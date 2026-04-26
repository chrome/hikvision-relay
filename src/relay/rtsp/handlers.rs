use base64::Engine;
use tokio::time::{timeout, Duration};

use crate::relay::rtsp::protocol::{play_track_url, rtsp_response, RtspRequest};
use crate::relay::rtsp_server::{
    RtspEvent, SdpSnapshot, SetupOutcome, Shared, CONTROL_SEND_TIMEOUT_MS, RTP_CLOCK_HZ,
    RTP_PAYLOAD_TYPE_VIDEO_DEFAULT,
};
use crate::relay::rtsp_transport::parse_tcp_interleaved_transport;

pub(crate) async fn build_rtsp_response(
    shared: &Shared,
    client_id: &str,
    req: &RtspRequest,
    cseq: &str,
) -> (Vec<u8>, bool) {
    let method = req.method.as_str();
    let is_teardown = method == "TEARDOWN";
    let response = match method {
        "OPTIONS" => rtsp_response(
            "200 OK",
            cseq,
            vec![("Public", "OPTIONS, DESCRIBE, SETUP, PLAY, PAUSE, TEARDOWN, GET_PARAMETER")],
            None,
        ),
        "DESCRIBE" => {
            let route = shared.ensure_client_route(client_id, &req.url);
            if route.is_none() {
                rtsp_response("404 Not Found", cseq, vec![], None)
            } else if !wait_parameter_sets(shared, route.expect("route exists"), 8_000).await {
                rtsp_response("503 Service Unavailable", cseq, vec![("Retry-After", "1")], None)
            } else {
                let sdp = build_sdp(shared, route.expect("route exists"), &req.url);
                let content_base = if req.url.ends_with('/') {
                    req.url.clone()
                } else {
                    format!("{}/", req.url)
                };
                rtsp_response(
                    "200 OK",
                    cseq,
                    vec![("Content-Type", "application/sdp"), ("Content-Base", content_base.as_str())],
                    Some(&sdp),
                )
            }
        }
        "SETUP" => {
            let transport = req.headers.get("transport").cloned().unwrap_or_default();
            let transport_spec = parse_tcp_interleaved_transport(&transport);
            if transport_spec.is_none() {
                rtsp_response(
                    "461 Unsupported Transport",
                    cseq,
                    vec![(
                        "Server-Note",
                        "Only RTSP/RTP over interleaved TCP is supported. Add ?rtsp_transport=tcp or use VLC/ffplay with TCP option.",
                    )],
                    None,
                )
            } else {
                match shared.apply_setup(client_id, &req.url, transport_spec) {
                    SetupOutcome::InternalError => rtsp_response("500 Internal Server Error", cseq, vec![], None),
                    SetupOutcome::NotFound => rtsp_response("404 Not Found", cseq, vec![], None),
                    SetupOutcome::Ok {
                        rtp_channel,
                        rtcp_channel,
                        session_id,
                    } => {
                        let transport_line =
                            format!("RTP/AVP/TCP;unicast;interleaved={rtp_channel}-{rtcp_channel}");
                        let session_line = format!("{session_id};timeout=60");
                        rtsp_response(
                            "200 OK",
                            cseq,
                            vec![("Transport", transport_line.as_str()), ("Session", session_line.as_str())],
                            None,
                        )
                    }
                }
            }
        }
        "PLAY" => {
            if let Some(resp) = validate_session_header(shared, client_id, req, cseq) {
                resp
            } else {
                let (video_ready, audio_ready, session_id) = shared.playback_state(client_id);
                if !video_ready && !audio_ready {
                    rtsp_response("455 Method Not Valid in This State", cseq, vec![], None)
                } else {
                    let _ = shared.set_client_playing(client_id, true);
                    if let Some(route) = shared.route_for_client(client_id) {
                        shared.send_event(RtspEvent::ClientPlaying { route });
                    }
                    let route = shared.route_for_client(client_id);
                    let (vseq, vts) = route.map(|r| shared.video_rtp_info(r)).unwrap_or((0, 0));
                    let mut rtp_info_parts: Vec<String> = Vec::new();
                    if video_ready {
                        rtp_info_parts.push(format!(
                            "url={};seq={};rtptime={}",
                            play_track_url(&req.url, 0),
                            vseq,
                            vts
                        ));
                    }
                    if audio_ready {
                        let (a_seq, a_ts) = route.map(|r| shared.audio_rtp_info(r)).unwrap_or((0, 0));
                        rtp_info_parts.push(format!(
                            "url={};seq={};rtptime={}",
                            play_track_url(&req.url, 1),
                            a_seq,
                            a_ts
                        ));
                    }
                    let rtp_info = rtp_info_parts.join(",");
                    let session_line = format!("{session_id};timeout=60");
                    rtsp_response(
                        "200 OK",
                        cseq,
                        vec![
                            ("Session", session_line.as_str()),
                            ("Range", "npt=0.000-"),
                            ("RTP-Info", rtp_info.as_str()),
                        ],
                        None,
                    )
                }
            }
        }
        "PAUSE" => {
            if let Some(resp) = validate_session_header(shared, client_id, req, cseq) {
                resp
            } else {
                let no_playing = shared.set_client_playing(client_id, false);
                if no_playing {
                    crate::log_step!("rtsp-server", "no_playing_clients");
                    if let Some(route) = shared.route_for_client(client_id) {
                        shared.send_event(RtspEvent::LastClientGone { route });
                    }
                }
                let session_id = shared.session_id(client_id);
                rtsp_response(
                    "200 OK",
                    cseq,
                    vec![("Session", &format!("{session_id};timeout=60"))],
                    None,
                )
            }
        }
        "GET_PARAMETER" => {
            let session_id = shared.session_id(client_id);
            rtsp_response(
                "200 OK",
                cseq,
                vec![("Session", &format!("{session_id};timeout=60"))],
                None,
            )
        }
        "TEARDOWN" => {
            if let Some(resp) = validate_session_header(shared, client_id, req, cseq) {
                resp
            } else {
                let no_playing = shared.set_client_playing(client_id, false);
                if no_playing {
                    crate::log_step!("rtsp-server", "no_playing_clients");
                    if let Some(route) = shared.route_for_client(client_id) {
                        shared.send_event(RtspEvent::LastClientGone { route });
                    }
                }
                let session_id = shared.session_id(client_id);
                rtsp_response(
                    "200 OK",
                    cseq,
                    vec![("Session", &format!("{session_id};timeout=60"))],
                    None,
                )
            }
        }
        _ => rtsp_response("501 Not Implemented", cseq, vec![], None),
    };
    (response, is_teardown)
}

pub(crate) async fn send_control_response(shared: &Shared, client_id: &str, response: Vec<u8>) {
    let tx = shared.control_tx(client_id);
    let Some(tx) = tx else {
        return;
    };
    let send = tx.send(response);
    if timeout(Duration::from_millis(CONTROL_SEND_TIMEOUT_MS), send)
        .await
        .is_err()
    {
        crate::log_step!("rtsp-server", "control_send_timeout", "id={}", client_id);
        shared.remove_client(client_id);
    }
}

fn validate_session_header(shared: &Shared, client_id: &str, req: &RtspRequest, cseq: &str) -> Option<Vec<u8>> {
    let provided = req
        .headers
        .get("session")
        .map(|v| v.split(';').next().unwrap_or("").trim().to_string());
    let expected = shared.expected_session(client_id);
    if expected.is_empty() {
        return Some(rtsp_response("454 Session Not Found", cseq, vec![], None));
    }
    match provided {
        Some(p) if p == expected => None,
        _ => {
            let session_line = format!("{expected};timeout=60");
            Some(rtsp_response(
                "454 Session Not Found",
                cseq,
                vec![("Session", session_line.as_str())],
                None,
            ))
        }
    }
}

pub(crate) async fn wait_parameter_sets(shared: &Shared, route: crate::relay::rtsp::protocol::RouteKey, timeout_ms: u64) -> bool {
    if shared.parameter_ready(route) {
        return true;
    }
    let mut rx = shared.parameter_ready_rx(route);
    let wait = async move {
        loop {
            if *rx.borrow() {
                return true;
            }
            if rx.changed().await.is_err() {
                return false;
            }
        }
    };
    (timeout(Duration::from_millis(timeout_ms), wait).await).unwrap_or_default()
}

fn trim_rbsp_emulation_prevention_3byte(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len());
    let mut i = 0usize;
    while i < rbsp.len() {
        if i + 2 < rbsp.len() && rbsp[i] == 0 && rbsp[i + 1] == 0 && rbsp[i + 2] == 3 {
            out.push(0);
            out.push(0);
            i += 3;
            continue;
        }
        out.push(rbsp[i]);
        i += 1;
    }
    out
}

fn profile_level_id_hex_from_sps(sps: &[u8]) -> String {
    if sps.len() < 4 {
        return String::from("42E01F");
    }
    let no_prev = trim_rbsp_emulation_prevention_3byte(sps);
    let profile_idc = *no_prev.get(1).unwrap_or(&0x42);
    let profile_iop = *no_prev.get(2).unwrap_or(&0xe0);
    let level_idc = *no_prev.get(3).unwrap_or(&0x1f);
    format!("{profile_idc:02X}{profile_iop:02X}{level_idc:02X}")
}

pub(crate) fn build_sdp(shared: &Shared, route: crate::relay::rtsp::protocol::RouteKey, url: &str) -> String {
    let SdpSnapshot {
        video_codec: codec,
        vps,
        sps,
        pps,
        audio_cfg,
    } = shared.sdp_snapshot(route);
    let mut lines = vec![
        String::from("v=0"),
        String::from("o=- 0 0 IN IP4 0.0.0.0"),
        String::from("s=Hikvision Live"),
        String::from("c=IN IP4 0.0.0.0"),
        String::from("t=0 0"),
        String::from("a=tool:HikvisionRtspBridge"),
        format!("m=video 0 RTP/AVP {}", RTP_PAYLOAD_TYPE_VIDEO_DEFAULT),
    ];
    if codec == "H265" {
        lines.push(format!(
            "a=rtpmap:{} H265/{}",
            RTP_PAYLOAD_TYPE_VIDEO_DEFAULT, RTP_CLOCK_HZ
        ));
        if let (Some(vps), Some(sps), Some(pps)) = (vps, sps, pps) {
            let vps = base64::engine::general_purpose::STANDARD.encode(vps);
            let sps = base64::engine::general_purpose::STANDARD.encode(sps);
            let pps = base64::engine::general_purpose::STANDARD.encode(pps);
            lines.push(format!(
                "a=fmtp:{} sprop-vps={}; sprop-sps={}; sprop-pps={}",
                RTP_PAYLOAD_TYPE_VIDEO_DEFAULT, vps, sps, pps
            ));
        }
    } else {
        lines.push(format!(
            "a=rtpmap:{} H264/{}",
            RTP_PAYLOAD_TYPE_VIDEO_DEFAULT, RTP_CLOCK_HZ
        ));
        if let (Some(sps), Some(pps)) = (sps, pps) {
            let pli = profile_level_id_hex_from_sps(&sps);
            let sps = base64::engine::general_purpose::STANDARD.encode(sps);
            let pps = base64::engine::general_purpose::STANDARD.encode(pps);
            lines.push(format!(
                "a=fmtp:{} packetization-mode=1; profile-level-id={pli}; sprop-parameter-sets={sps},{pps}",
                RTP_PAYLOAD_TYPE_VIDEO_DEFAULT
            ));
        }
    }
    lines.push(String::from("a=control:trackID=0"));
    if let Some(a) = audio_cfg {
        lines.push(format!("m=audio 0 RTP/AVP {}", a.payload_type()));
        let ch = if a.channels() > 1 {
            format!("/{}", a.channels())
        } else {
            String::new()
        };
        lines.push(format!(
            "a=rtpmap:{} {}/{}{}",
            a.payload_type(),
            a.codec().to_uppercase(),
            a.clock_hz(),
            ch
        ));
        lines.push(String::from("a=control:trackID=1"));
    }
    let mut sdp = lines.join("\r\n");
    sdp.push_str("\r\n");
    let _ = url;
    sdp
}
