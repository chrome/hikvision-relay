use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use rand::Rng;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, watch};
use crate::error::{AppError, AppResult};
use crate::relay::rtsp::handlers::{build_rtsp_response, send_control_response};
use crate::relay::rtsp::protocol::{parse_route_key, parse_rtsp_request, parse_track_id_from_url, rtsp_response, RouteKey};
use crate::relay::rtsp_transport::TransportSpec;

pub(crate) const RTP_PAYLOAD_TYPE_VIDEO_DEFAULT: u8 = 96;
pub(crate) const RTP_CLOCK_HZ: u32 = 90_000;
const MAX_RTP_PAYLOAD: usize = 1400;
const CLIENT_QUEUE_CAPACITY: usize = 512;
const DEFAULT_VIDEO_FPS: u32 = 25;
const MAX_RTSP_HEADER_BYTES: usize = 32 * 1024;
pub(crate) const CONTROL_SEND_TIMEOUT_MS: u64 = 1000;

#[derive(Debug, Clone)]
pub enum RtspEvent {
    FirstClientConnected { route: RouteKey },
    LastClientGone { route: RouteKey },
    ParameterSetsReady { route: RouteKey },
    ClientPlaying { route: RouteKey },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct RtspStats {
    pub clients: usize,
}

#[derive(Clone)]
pub(crate) struct Shared {
    clients: Arc<Mutex<HashMap<String, ClientState>>>,
    total_rtp_packets_sent: Arc<Mutex<usize>>,
    bytes_sent: Arc<Mutex<usize>>,
    route_state: Arc<Mutex<HashMap<RouteKey, RouteMediaState>>>,
    event_tx: broadcast::Sender<RtspEvent>,
    base_path: String,
}

#[derive(Clone)]
pub(crate) struct RouteMediaState {
    video_codec: String,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    vps: Option<Vec<u8>>,
    parameter_sets_announced: bool,
    pending_access_unit: Vec<Vec<u8>>,
    audio_cfg: Option<AudioCfg>,
    video_rtp: VideoRtpState,
    parameter_ready_tx: watch::Sender<bool>,
    parameter_ready_rx: watch::Receiver<bool>,
}

#[derive(Clone)]
pub(crate) struct AudioCfg {
    codec: String,
    payload_type: u8,
    clock_hz: u32,
    channels: u8,
    ssrc: u32,
    seq: u16,
    ts_base: u32,
    first_src_ts: Option<u32>,
}

impl AudioCfg {
    pub(crate) fn payload_type(&self) -> u8 {
        self.payload_type
    }
    pub(crate) fn clock_hz(&self) -> u32 {
        self.clock_hz
    }
    pub(crate) fn channels(&self) -> u8 {
        self.channels
    }
    pub(crate) fn codec(&self) -> &str {
        &self.codec
    }
}

#[derive(Clone)]
pub(crate) struct VideoRtpState {
    seq: u16,
    ts: u32,
    /// Stable per-session SSRC (matches Node `EmbeddedRtspServer` constructor).
    ssrc: u32,
}

#[derive(Clone)]
pub(crate) struct ClientState {
    tx: mpsc::Sender<Vec<u8>>,
    playing: bool,
    video_synced: bool,
    video_rtp_channel: Option<u8>,
    audio_rtp_channel: Option<u8>,
    session_id: String,
    route: Option<RouteKey>,
}

pub(crate) enum SetupOutcome {
    Ok {
        rtp_channel: u8,
        rtcp_channel: u8,
        session_id: String,
    },
    NotFound,
    InternalError,
}

pub(crate) struct SdpSnapshot {
    pub(crate) video_codec: String,
    pub(crate) vps: Option<Vec<u8>>,
    pub(crate) sps: Option<Vec<u8>>,
    pub(crate) pps: Option<Vec<u8>>,
    pub(crate) audio_cfg: Option<AudioCfg>,
}

impl RouteMediaState {
    fn new() -> Self {
        let mut rng = rand::thread_rng();
        let (parameter_ready_tx, parameter_ready_rx) = watch::channel(false);
        Self {
            video_codec: String::from("H264"),
            sps: None,
            pps: None,
            vps: None,
            parameter_sets_announced: false,
            pending_access_unit: Vec::new(),
            audio_cfg: Some(AudioCfg {
                codec: String::from("PCMA"),
                payload_type: 8,
                clock_hz: 8000,
                channels: 1,
                ssrc: rng.r#gen::<u32>(),
                seq: rng.r#gen::<u16>(),
                ts_base: rng.r#gen::<u32>(),
                first_src_ts: None,
            }),
            video_rtp: VideoRtpState {
                seq: rng.r#gen::<u16>(),
                ts: rng.r#gen::<u32>(),
                ssrc: rng.r#gen::<u32>(),
            },
            parameter_ready_tx,
            parameter_ready_rx,
        }
    }
}

pub(crate) fn has_playing_clients(clients: &std::collections::HashMap<String, ClientState>) -> bool {
    clients.values().any(|c| c.playing)
}

impl Shared {
    pub(crate) fn ensure_client_route(&self, client_id: &str, req_url: &str) -> Option<RouteKey> {
        let route = parse_route_key(req_url, &self.base_path)?;
        let mut clients = self.clients.lock();
        let (should_emit_first, prev_route, prev_was_playing) = clients
            .get(client_id)
            .map(|c| (c.route != Some(route), c.route, c.playing))
            .unwrap_or((false, None, false));
        let client = clients.get_mut(client_id)?;
        if prev_route != Some(route) {
            client.playing = false;
            client.video_synced = false;
            client.video_rtp_channel = None;
            client.audio_rtp_channel = None;
            client.session_id = format!("{:x}", rand::random::<u64>());
        }
        client.route = Some(route);
        if let Some(prev) = prev_route {
            if prev != route && prev_was_playing {
                let prev_has_playing = clients
                    .values()
                    .any(|cl| cl.route == Some(prev) && cl.playing);
                if !prev_has_playing {
                    let _ = self.event_tx.send(RtspEvent::LastClientGone { route: prev });
                }
            }
        }
        if should_emit_first {
            let first = clients.values().filter(|cl| cl.route == Some(route)).count() == 1;
            if first {
                let _ = self.event_tx.send(RtspEvent::FirstClientConnected { route });
            }
        }
        Some(route)
    }

    pub(crate) fn apply_setup(
        &self,
        client_id: &str,
        req_url: &str,
        transport_spec: Option<TransportSpec>,
    ) -> SetupOutcome {
        let Some(route) = self.ensure_client_route(client_id, req_url) else {
            return SetupOutcome::NotFound;
        };
        let mut clients = self.clients.lock();
        let (track_id, session_id) = {
            let Some(c) = clients.get_mut(client_id) else {
                return SetupOutcome::InternalError;
            };
            let track_id = parse_track_id_from_url(req_url).unwrap_or_else(|| {
                if c.video_rtp_channel.is_none() {
                    0
                } else if c.audio_rtp_channel.is_none() {
                    1
                } else {
                    0
                }
            });
            if track_id != 0 && track_id != 1 {
                return SetupOutcome::NotFound;
            }
            (track_id, c.session_id.clone())
        };
        let TransportSpec {
            rtp_channel,
            rtcp_channel,
        } = transport_spec.unwrap_or_else(|| {
            if track_id == 0 {
                TransportSpec {
                    rtp_channel: 0,
                    rtcp_channel: 1,
                }
            } else {
                TransportSpec {
                    rtp_channel: 2,
                    rtcp_channel: 3,
                }
            }
        });
        if let Some(c) = clients.get_mut(client_id) {
            if track_id == 0 {
                c.video_rtp_channel = Some(rtp_channel);
            } else {
                c.audio_rtp_channel = Some(rtp_channel);
            }
        }
        let _ = route;
        SetupOutcome::Ok {
            rtp_channel,
            rtcp_channel,
            session_id,
        }
    }

    pub(crate) fn playback_state(&self, client_id: &str) -> (bool, bool, String) {
        let clients = self.clients.lock();
        match clients.get(client_id) {
            Some(c) => (
                c.video_rtp_channel.is_some(),
                c.audio_rtp_channel.is_some(),
                c.session_id.clone(),
            ),
            None => (false, false, String::new()),
        }
    }

    pub(crate) fn set_client_playing(&self, client_id: &str, playing: bool) -> bool {
        let mut clients = self.clients.lock();
        let route = clients.get(client_id).and_then(|c| c.route);
        if let Some(c) = clients.get_mut(client_id) {
            c.playing = playing;
            if playing {
                c.video_synced = false;
            }
        }
        if let Some(route) = route {
            !clients
                .values()
                .any(|c| c.playing && c.route == Some(route))
        } else {
            !has_playing_clients(&clients)
        }
    }

    pub(crate) fn send_event(&self, event: RtspEvent) {
        let _ = self.event_tx.send(event);
    }

    pub(crate) fn video_rtp_info(&self, route: RouteKey) -> (u16, u32) {
        self.with_route_state(route, |s| (s.video_rtp.seq, s.video_rtp.ts))
            .unwrap_or((0, 0))
    }

    pub(crate) fn audio_rtp_info(&self, route: RouteKey) -> (u16, u32) {
        self.with_route_state(route, |s| {
            s.audio_cfg
            .as_ref()
            .map(|a| (a.seq, a.ts_base))
            .unwrap_or((0, 0))
        })
        .unwrap_or((0, 0))
    }

    pub(crate) fn session_id(&self, client_id: &str) -> String {
        self.clients
            .lock()
            .get(client_id)
            .map(|c| c.session_id.clone())
            .unwrap_or_default()
    }

    pub(crate) fn control_tx(&self, client_id: &str) -> Option<mpsc::Sender<Vec<u8>>> {
        self.clients.lock().get(client_id).map(|c| c.tx.clone())
    }

    pub(crate) fn client_count(&self) -> usize {
        self.clients.lock().len()
    }

    pub(crate) fn remove_client(&self, client_id: &str) {
        let mut clients = self.clients.lock();
        let removed_route = clients.remove(client_id).and_then(|c| c.route);
        if let Some(route) = removed_route {
            let left = clients.values().any(|c| c.route == Some(route));
            if !left {
                let _ = self.event_tx.send(RtspEvent::LastClientGone { route });
            }
        }
    }

    pub(crate) fn expected_session(&self, client_id: &str) -> String {
        self.session_id(client_id)
    }

    pub(crate) fn parameter_ready_rx(&self, route: RouteKey) -> watch::Receiver<bool> {
        self.with_route_state_mut(route, |s| s.parameter_ready_rx.clone())
    }

    pub(crate) fn parameter_ready(&self, route: RouteKey) -> bool {
        self.with_route_state(route, |s| *s.parameter_ready_rx.borrow())
            .unwrap_or(false)
    }

    pub(crate) fn route_for_client(&self, client_id: &str) -> Option<RouteKey> {
        self.clients.lock().get(client_id).and_then(|c| c.route)
    }

    fn with_route_state_mut<R>(&self, route: RouteKey, f: impl FnOnce(&mut RouteMediaState) -> R) -> R {
        let mut all = self.route_state.lock();
        let state = all.entry(route).or_insert_with(RouteMediaState::new);
        f(state)
    }

    fn with_route_state<R>(&self, route: RouteKey, f: impl FnOnce(&RouteMediaState) -> R) -> Option<R> {
        let all = self.route_state.lock();
        all.get(&route).map(f)
    }

    pub(crate) fn sdp_snapshot(&self, route: RouteKey) -> SdpSnapshot {
        self.with_route_state_mut(route, |s| SdpSnapshot {
            video_codec: s.video_codec.clone(),
            vps: s.vps.clone(),
            sps: s.sps.clone(),
            pps: s.pps.clone(),
            audio_cfg: s.audio_cfg.clone(),
        })
    }
}

#[derive(Clone)]
pub struct EmbeddedRtspServer {
    host: String,
    port: u16,
    path: String,
    shared: Shared,
    stop_tx: Option<broadcast::Sender<()>>,
}

impl EmbeddedRtspServer {
    pub fn new(host: String, port: u16, path: String) -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self {
            host,
            port,
            path: path.clone(),
            shared: Shared {
                clients: Arc::new(Mutex::new(HashMap::new())),
                total_rtp_packets_sent: Arc::new(Mutex::new(0)),
                bytes_sent: Arc::new(Mutex::new(0)),
                route_state: Arc::new(Mutex::new(HashMap::new())),
                event_tx,
                base_path: path.clone(),
            },
            stop_tx: None,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RtspEvent> {
        self.shared.event_tx.subscribe()
    }

    pub fn build_bind_url(&self) -> String {
        format!("rtsp://{}:{}{}", self.host, self.port, self.path)
    }

    pub async fn start(&mut self) -> AppResult<()> {
        let listener = TcpListener::bind((self.host.as_str(), self.port))
            .await
            .map_err(|e| AppError::Rtsp(format!("bind failed: {e}")))?;
        let (stop_tx, mut stop_rx) = broadcast::channel(1);
        self.stop_tx = Some(stop_tx.clone());
        let shared = self.shared.clone();
        let path = self.path.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => { break; }
                    accept = listener.accept() => {
                        match accept {
                            Ok((socket, _)) => {
                                let shared2 = shared.clone();
                                let path2 = path.clone();
                                tokio::spawn(async move {
                                let _ = handle_connection(socket, path2, shared2).await;
                                });
                            }
                            Err(err) => {
                                let _ = shared.event_tx.send(RtspEvent::Error(err.to_string()));
                                break;
                            }
                        }
                    }
                }
            }
        });
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
    }

    pub fn set_video_codec_for_route(&self, route: RouteKey, codec: &str) {
        let normalized = codec.to_uppercase();
        self.shared.with_route_state_mut(route, |s| {
            if s.video_codec != normalized {
                s.video_codec = normalized;
                s.vps = None;
                s.sps = None;
                s.pps = None;
                s.parameter_sets_announced = false;
                s.pending_access_unit.clear();
                let _ = s.parameter_ready_tx.send(false);
            }
        });
    }

    pub fn enable_audio_for_route(&self, route: RouteKey, codec: &str, payload_type: u8, clock_hz: u32, channels: u8) {
        self.shared.with_route_state_mut(route, |s| {
        if let Some(existing) = s.audio_cfg.as_ref() {
            if existing.codec.eq_ignore_ascii_case(codec)
                && existing.payload_type == payload_type
                && existing.clock_hz == clock_hz
                && existing.channels == channels
            {
                return;
            }
        }
        let mut rng = rand::thread_rng();
        s.audio_cfg = Some(AudioCfg {
            codec: codec.to_string(),
            payload_type,
            clock_hz,
            channels,
            ssrc: rng.r#gen::<u32>(),
            seq: rng.r#gen::<u16>(),
            ts_base: rng.r#gen::<u32>(),
            first_src_ts: None,
        });
        });
    }

    pub fn feed_video_annexb_for_route(&self, route: RouteKey, chunk: &[u8]) {
        let nals = split_annexb(chunk);
        for nal in &nals {
            if nal.is_empty() {
                continue;
            }
            let codec = self.shared.with_route_state_mut(route, |s| s.video_codec.clone());
            if codec == "H265" {
                let t = (nal[0] >> 1) & 0x3f;
                if t == 35 {
                    self.flush_access_unit(route);
                    continue;
                }
                if t == 32 {
                    self.shared.with_route_state_mut(route, |s| s.vps = Some(nal.clone()));
                    continue;
                } else if t == 33 {
                    self.shared.with_route_state_mut(route, |s| s.sps = Some(nal.clone()));
                    continue;
                } else if t == 34 {
                    self.shared.with_route_state_mut(route, |s| s.pps = Some(nal.clone()));
                    continue;
                }
                let pending_len = self.shared.with_route_state_mut(route, |s| {
                    s.pending_access_unit.push(nal.clone());
                    s.pending_access_unit.len()
                });
                if t <= 31 || pending_len > 64 {
                    self.flush_access_unit(route);
                }
            } else {
                let t = nal[0] & 0x1f;
                if t == 9 {
                    self.flush_access_unit(route);
                    continue;
                }
                if t == 7 {
                    self.shared.with_route_state_mut(route, |s| s.sps = Some(nal.clone()));
                    continue;
                } else if t == 8 {
                    self.shared.with_route_state_mut(route, |s| s.pps = Some(nal.clone()));
                    continue;
                }
                let pending_len = self.shared.with_route_state_mut(route, |s| {
                    s.pending_access_unit.push(nal.clone());
                    s.pending_access_unit.len()
                });
                if t == 1 || t == 5 || pending_len > 64 {
                    self.flush_access_unit(route);
                }
            }
        }
        let ready_now = self.parameter_sets_ready(route);
        if ready_now {
            let announced = self.shared.with_route_state_mut(route, |s| {
                if s.parameter_sets_announced {
                    true
                } else {
                    s.parameter_sets_announced = true;
                    let _ = s.parameter_ready_tx.send(true);
                    false
                }
            });
            if !announced {
                let _ = self.shared.event_tx.send(RtspEvent::ParameterSetsReady { route });
            }
        }
        // leftover non-VCL pending is intentionally kept until AU boundary.
    }

    pub fn feed_audio_rtp_for_route(&self, route: RouteKey, payload: &[u8], src_ts: u32, marker: bool) {
        let maybe_pkt = self.shared.with_route_state_mut(route, |s| {
            let Some(cfg) = s.audio_cfg.as_mut() else { return None; };
            if cfg.first_src_ts.is_none() {
                cfg.first_src_ts = Some(src_ts);
            }
            let delta = src_ts.wrapping_sub(cfg.first_src_ts.unwrap_or(src_ts));
            let ts = cfg.ts_base.wrapping_add(delta);
            cfg.seq = cfg.seq.wrapping_add(1);
            Some(make_rtp_packet(marker, cfg.payload_type, cfg.seq, ts, cfg.ssrc, payload))
        });
        if let Some(pkt) = maybe_pkt {
            self.broadcast_interleaved(route, 2, &pkt, true, false);
        }
    }

    pub fn stats(&self) -> RtspStats {
        RtspStats {
            clients: self.shared.clients.lock().len(),
        }
    }

    fn parameter_sets_ready(&self, route: RouteKey) -> bool {
        let codec = self.shared.with_route_state(route, |s| s.video_codec.clone());
        let Some(codec) = codec else {
            return false;
        };
        if codec == "H265" {
            self.shared.with_route_state(route, |s| s.vps.is_some() && s.sps.is_some() && s.pps.is_some()).unwrap_or(false)
        } else {
            self.shared.with_route_state(route, |s| s.sps.is_some() && s.pps.is_some()).unwrap_or(false)
        }
    }

    fn send_video_nals(&self, route: RouteKey, nals: Vec<Vec<u8>>) {
        let Some((mut seq, ts, codec, ssrc)) = self.shared.with_route_state(route, |s| {
            (s.video_rtp.seq, s.video_rtp.ts, s.video_codec.clone(), s.video_rtp.ssrc)
        }) else {
            return;
        };
        let sync_access_unit = is_sync_access_unit(&codec, &nals);
        for (idx, nal) in nals.iter().enumerate() {
            let is_last = idx + 1 == nals.len();
            let packets = if codec == "H265" {
                packetize_h265_nal(nal, seq, ts, is_last, ssrc)
            } else {
                packetize_h264_nal(nal, seq, ts, is_last, ssrc)
            };
            if let Some(last_seq) = packets.last().map(|p| u16::from_be_bytes([p[2], p[3]])) {
                seq = last_seq.wrapping_add(1);
            }
            for pkt in packets {
                self.broadcast_interleaved(route, 0, &pkt, false, sync_access_unit);
            }
        }
        self.shared.with_route_state_mut(route, |s| {
            s.video_rtp.seq = seq;
            let ts_step = RTP_CLOCK_HZ / DEFAULT_VIDEO_FPS;
            s.video_rtp.ts = s.video_rtp.ts.wrapping_add(ts_step.max(1));
        });
    }

    fn flush_access_unit(&self, route: RouteKey) {
        let nals = self.shared.with_route_state_mut(route, |s| {
            if s.pending_access_unit.is_empty() {
                None
            } else {
                Some(std::mem::take(&mut s.pending_access_unit))
            }
        });
        let Some(nals) = nals else {
            return;
        };
        self.send_video_nals(route, nals);
    }

    fn broadcast_interleaved(
        &self,
        route: RouteKey,
        default_channel: u8,
        rtp_packet: &[u8],
        audio: bool,
        sync_access_unit: bool,
    ) {
        let mut dead = Vec::new();
        let targets: Vec<(String, mpsc::Sender<Vec<u8>>, u8)> = {
            let mut clients = self.shared.clients.lock();
            clients
                .iter_mut()
                .filter_map(|(id, c)| {
                    if !c.playing {
                        return None;
                    }
                    if c.route != Some(route) {
                        return None;
                    }
                    if !audio && !c.video_synced {
                        if !sync_access_unit {
                            return None;
                        }
                        c.video_synced = true;
                    }
                    let channel = if audio {
                        c.audio_rtp_channel.unwrap_or(default_channel)
                    } else {
                        c.video_rtp_channel.unwrap_or(default_channel)
                    };
                    Some((id.clone(), c.tx.clone(), channel))
                })
                .collect()
        };
        let mut sent_packets = 0usize;
        let mut sent_bytes = 0usize;
        for (id, tx, channel) in targets {
            let frame = interleave(channel, rtp_packet);
            match tx.try_send(frame) {
                Ok(()) => {
                    sent_packets += 1;
                    sent_bytes += rtp_packet.len() + 4;
                }
                Err(_) => dead.push(id),
            }
        }
        if sent_packets > 0 {
            *self.shared.total_rtp_packets_sent.lock() += sent_packets;
            *self.shared.bytes_sent.lock() += sent_bytes;
        }
        if dead.is_empty() {
            return;
        }
        let mut clients = self.shared.clients.lock();
        for id in dead {
            clients.remove(&id);
        }
    }
}

async fn handle_connection(socket: TcpStream, _path: String, shared: Shared) -> AppResult<()> {
    let client_id = format!("{:08x}", rand::random::<u32>());
    let (mut rd, mut wr) = socket.into_split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(CLIENT_QUEUE_CAPACITY);

    {
        let mut clients = shared.clients.lock();
        clients.insert(
            client_id.clone(),
            ClientState {
                tx,
                playing: false,
                video_synced: false,
                video_rtp_channel: None,
                audio_rtp_channel: None,
                session_id: format!("{:x}", rand::random::<u64>()),
                route: None,
            },
        );
        crate::log_step!(
            "rtsp-server",
            "client_connected",
            "id={} totalClients={}",
            client_id,
            clients.len()
        );
    }

    let writer_shared = shared.clone();
    let writer_client_id = client_id.clone();
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if wr.write_all(&frame).await.is_err() {
                writer_shared.remove_client(&writer_client_id);
                crate::log_step!(
                    "rtsp-server",
                    "client_disconnected",
                    "id={} totalClients={} reason=write_error",
                    writer_client_id,
                    writer_shared.client_count()
                );
                break;
            }
        }
    });

    let mut buf = Vec::<u8>::new();
    let mut read_pos = 0usize;
    let mut temp = [0_u8; 8192];
    'read: loop {
        let n = rd.read(&mut temp).await?;
        if n == 0 {
            crate::log_step!("rtsp-server", "socket_eof", "id={}", client_id);
            break;
        }
        buf.extend_from_slice(&temp[..n]);
        if buf.len() > MAX_RTSP_HEADER_BYTES {
            crate::log_step!("rtsp-server", "request_too_large", "id={} bytes={}", client_id, buf.len());
            let response = rtsp_response("400 Bad Request", "0", vec![], None);
            send_control_response(&shared, &client_id, response).await;
            break;
        }
        loop {
            let Some(slice) = buf.get(read_pos..) else { break; };
            if slice.is_empty() {
                break;
            }
            // Interleaved RTP/RTCP over RTSP/TCP starts with '$' + channel + length(2 bytes).
            // These are binary frames and must not be parsed as RTSP text requests.
            if slice[0] == 0x24 {
                if slice.len() < 4 {
                    break;
                }
                let payload_len = u16::from_be_bytes([slice[2], slice[3]]) as usize;
                let frame_len = 4 + payload_len;
                if slice.len() < frame_len {
                    break;
                }
                read_pos += frame_len;
                continue;
            }
            let Some((req, used)) = parse_rtsp_request(slice) else { break; };
            read_pos += used;
            let cseq = req.headers.get("cseq").cloned().unwrap_or_else(|| String::from("0"));
            let method = req.method.as_str();
            crate::log_step!(
                "rtsp-server",
                "rtsp_request",
                "id={} method={} cseq={}",
                client_id,
                method,
                cseq
            );
            let (response, is_teardown) = build_rtsp_response(&shared, &client_id, &req, &cseq).await;
            if writer.is_finished() {
                break 'read;
            }
            send_control_response(&shared, &client_id, response).await;
            if is_teardown {
                break 'read;
            }
        }
        if read_pos > 0 {
            buf.drain(..read_pos);
            read_pos = 0;
        }
    }

    shared.remove_client(&client_id);
    crate::log_step!(
        "rtsp-server",
        "client_disconnected",
        "id={} totalClients={} reason=read_loop_end",
        client_id,
        shared.client_count()
    );
    Ok(())
}


fn split_annexb(chunk: &[u8]) -> Vec<Vec<u8>> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 < chunk.len() {
        if chunk[i] == 0 && chunk[i + 1] == 0 && chunk[i + 2] == 1 {
            starts.push((i, 3usize));
            i += 3;
        } else if i + 4 < chunk.len() && chunk[i] == 0 && chunk[i + 1] == 0 && chunk[i + 2] == 0 && chunk[i + 3] == 1 {
            starts.push((i, 4usize));
            i += 4;
        } else {
            i += 1;
        }
    }
    if starts.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for idx in 0..starts.len() {
        let (s, len) = starts[idx];
        let start = s + len;
        let end = if idx + 1 < starts.len() {
            starts[idx + 1].0
        } else {
            chunk.len()
        };
        if start < end {
            out.push(chunk[start..end].to_vec());
        }
    }
    out
}

fn interleave(channel: u8, rtp: &[u8]) -> Vec<u8> {
    let mut out = vec![0x24, channel, 0, 0];
    out[2..4].copy_from_slice(&(rtp.len() as u16).to_be_bytes());
    out.extend_from_slice(rtp);
    out
}

fn make_rtp_packet(marker: bool, payload_type: u8, sequence: u16, timestamp: u32, ssrc: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0_u8; 12];
    out[0] = 2 << 6;
    out[1] = ((marker as u8) << 7) | (payload_type & 0x7f);
    out[2..4].copy_from_slice(&sequence.to_be_bytes());
    out[4..8].copy_from_slice(&timestamp.to_be_bytes());
    out[8..12].copy_from_slice(&ssrc.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn is_sync_access_unit(codec: &str, nals: &[Vec<u8>]) -> bool {
    if codec == "H265" {
        nals.iter().any(|nal| {
            if nal.is_empty() {
                return false;
            }
            let nal_type = (nal[0] >> 1) & 0x3f;
            matches!(nal_type, 19 | 20 | 21)
        })
    } else {
        nals.iter().any(|nal| {
            if nal.is_empty() {
                return false;
            }
            (nal[0] & 0x1f) == 5
        })
    }
}

fn packetize_h264_nal(nal: &[u8], seq_start: u16, ts: u32, is_last_nal: bool, ssrc: u32) -> Vec<Vec<u8>> {
    if nal.len() <= MAX_RTP_PAYLOAD {
        return vec![make_rtp_packet(
            is_last_nal,
            RTP_PAYLOAD_TYPE_VIDEO_DEFAULT,
            seq_start,
            ts,
            ssrc,
            nal,
        )];
    }
    let nal_header = nal[0];
    let nri = (nal_header >> 5) & 0x03;
    let ntype = nal_header & 0x1f;
    let fu_indicator = (nri << 5) | 28;
    let payload = &nal[1..];
    let mut out = Vec::new();
    let mut seq = seq_start;
    let mut off = 0usize;
    while off < payload.len() {
        let take = (MAX_RTP_PAYLOAD - 2).min(payload.len() - off);
        let start = off == 0;
        let end = off + take >= payload.len();
        let mut fu_header = ntype;
        if start {
            fu_header |= 0x80;
        }
        if end {
            fu_header |= 0x40;
        }
        let mut data = vec![fu_indicator, fu_header];
        data.extend_from_slice(&payload[off..off + take]);
        out.push(make_rtp_packet(
            end && is_last_nal,
            RTP_PAYLOAD_TYPE_VIDEO_DEFAULT,
            seq,
            ts,
            ssrc,
            &data,
        ));
        off += take;
        seq = seq.wrapping_add(1);
    }
    out
}

fn packetize_h265_nal(nal: &[u8], seq_start: u16, ts: u32, is_last_nal: bool, ssrc: u32) -> Vec<Vec<u8>> {
    if nal.len() <= MAX_RTP_PAYLOAD {
        return vec![make_rtp_packet(
            is_last_nal,
            RTP_PAYLOAD_TYPE_VIDEO_DEFAULT,
            seq_start,
            ts,
            ssrc,
            nal,
        )];
    }
    if nal.len() < 2 {
        return Vec::new();
    }
    let orig_b0 = nal[0];
    let orig_b1 = nal[1];
    let orig_type = (orig_b0 >> 1) & 0x3f;
    let outer_b0 = (orig_b0 & 0x81) | (49 << 1);
    let payload = &nal[2..];
    let mut out = Vec::new();
    let mut seq = seq_start;
    let mut off = 0usize;
    while off < payload.len() {
        let take = (MAX_RTP_PAYLOAD - 3).min(payload.len() - off);
        let start = off == 0;
        let end = off + take >= payload.len();
        let mut fu_header = orig_type;
        if start {
            fu_header |= 0x80;
        }
        if end {
            fu_header |= 0x40;
        }
        let mut data = vec![outer_b0, orig_b1, fu_header];
        data.extend_from_slice(&payload[off..off + take]);
        out.push(make_rtp_packet(
            end && is_last_nal,
            RTP_PAYLOAD_TYPE_VIDEO_DEFAULT,
            seq,
            ts,
            ssrc,
            &data,
        ));
        off += take;
        seq = seq.wrapping_add(1);
    }
    out
}

