use std::sync::Arc;

use parking_lot::Mutex;
use tokio::time::{sleep, Instant};

use crate::config::{RelayConfig, RuntimeConfig, StreamType};
use crate::error::{AppError, AppResult};
use crate::hikvision::client::{CallbackFn, HikvisionClient, RelayStats};
use crate::hikvision::ffi::{NET_DVR_STREAMDATA, NET_DVR_SYSHEAD};
use crate::relay::depacketizer::{RtpVideoDepacketizer, VideoCodec};
use crate::relay::rtsp::protocol::RouteKey;
use crate::relay::rtsp_server::EmbeddedRtspServer;
use crate::relay::watchdog_policy::PipelineWatchdog;

const ANNEXB: [u8; 4] = [0, 0, 0, 1];

pub(crate) struct PipelineCtx {
    pub(crate) sdk: HikvisionClient,
}

fn preview_error_is_permanent(code: Option<u32>) -> bool {
    matches!(code, Some(4 | 17 | 23 | 90 | 91))
}

fn should_fallback_sub_to_main(code: Option<u32>, stream: StreamType, tried_sub_to_main: bool) -> bool {
    code == Some(91) && stream == StreamType::Sub && !tried_sub_to_main
}

fn retry_delay_ms_for_code(code: Option<u32>) -> u64 {
    if code == Some(5) { 2500 } else { 750 }
}

fn should_retry_preview_attempt(code: Option<u32>, attempt: u32) -> bool {
    !preview_error_is_permanent(code) && attempt < 4
}

pub(crate) async fn maybe_restart_pipeline(
    pipeline: &mut Option<PipelineCtx>,
    runtime: &RuntimeConfig,
    relay_cfg: &RelayConfig,
    sdk_channel: i32,
    stream_type: StreamType,
    route: RouteKey,
    rtsp_server: &EmbeddedRtspServer,
    stats: &Arc<Mutex<RelayStats>>,
    watch: &Arc<Mutex<PipelineWatchdog>>,
    restart_count: &mut u32,
) -> AppResult<()> {
    if !relay_cfg.restart_on_fail || *restart_count >= relay_cfg.max_restarts || rtsp_server.stats().clients == 0 {
        return Ok(());
    }
    *restart_count += 1;
    crate::log_step!("relay", "pipeline_restart_scheduled", "count={}", restart_count);
    match start_pipeline_with_retries(
        runtime,
        relay_cfg,
        sdk_channel,
        stream_type,
        route,
        rtsp_server,
        stats.clone(),
        watch.clone(),
    )
    .await
    {
        Ok(ctx) => {
            watch.lock().pipeline_started_at = Some(Instant::now());
            *pipeline = Some(ctx);
        }
        Err(e) => {
            stats.lock().last_sdk_error = Some(e.to_string());
            return Err(e);
        }
    }
    Ok(())
}

pub(crate) async fn start_pipeline_with_retries(
    runtime: &RuntimeConfig,
    _relay_cfg: &RelayConfig,
    sdk_channel: i32,
    stream_type: StreamType,
    route: RouteKey,
    rtsp_server: &EmbeddedRtspServer,
    stats: Arc<Mutex<RelayStats>>,
    watch: Arc<Mutex<PipelineWatchdog>>,
) -> AppResult<PipelineCtx> {
    let mut stream = stream_type;
    let mut tried_sub_to_main = false;
    let mut last_err: Option<AppError> = None;
    crate::log_step!(
        "relay",
        "pipeline_starting",
        "sdkChannel={} stream={}",
        sdk_channel,
        stream.as_str()
    );

    for attempt in 1..=4u32 {
        crate::log_step!(
            "relay",
            "pipeline_start_attempt",
            "attempt={}/4 stream={}",
            attempt,
            stream.as_str()
        );
        *watch.lock() = PipelineWatchdog::default();
        rtsp_server.enable_audio_for_route(route, "PCMA", 8, 8000, 1);
        let dep = Arc::new(Mutex::new(RtpVideoDepacketizer::new()));
        let dep_cb = dep.clone();
        let server_cb = rtsp_server.clone();
        let stats_cb = stats.clone();
        let watch_cb = watch.clone();
        let cb: CallbackFn = Arc::new(move |data_type, chunk| {
            if stats_cb.lock().sdk_chunks == 0 {
                crate::log_step!(
                    "callback",
                    "first_callback_received",
                    "dataType={} bytes={}",
                    data_type,
                    chunk.len()
                );
            }
            let is_syshead = data_type == NET_DVR_SYSHEAD;
            let is_stream = data_type == NET_DVR_STREAMDATA;
            if !is_syshead && !is_stream {
                crate::log_step!(
                    "callback",
                    "non_stream_data",
                    "dataType={} bytes={}",
                    data_type,
                    chunk.len()
                );
                return;
            }
            if is_stream {
                let mut w = watch_cb.lock();
                let now = Instant::now();
                w.last_stream_data = Some(now);
                if w.first_stream_data.is_none() {
                    w.first_stream_data = Some(now);
                }
                drop(w);
            }

            let mut stats_guard = stats_cb.lock();
            if is_stream {
                stats_guard.sdk_chunks += 1;
                stats_guard.sdk_bytes += chunk.len();
            }
            let mut dep_guard = dep_cb.lock();
            let (nals, audio) = dep_guard.feed_sdk_chunk(&chunk);
            if let Some(codec) = dep_guard.video_codec {
                match codec {
                    VideoCodec::H264 => server_cb.set_video_codec_for_route(route, "H264"),
                    VideoCodec::H265 => server_cb.set_video_codec_for_route(route, "H265"),
                }
            }
            if !is_syshead {
                stats_guard.rtp_packets += 1;
            }
            drop(dep_guard);
            drop(stats_guard);
            if let Some(audio_packet) = audio {
                server_cb.feed_audio_rtp_for_route(route, &audio_packet.payload, audio_packet.timestamp, audio_packet.marker);
            }
            if !nals.is_empty() {
                let mut dep_guard = dep_cb.lock();
                if dep_guard.video_codec.is_none() {
                    if let Some(first) = nals.first() {
                        dep_guard.video_codec = first.first().copied().and_then(crate::relay::depacketizer::detect_codec);
                    }
                }
                if let Some(codec) = dep_guard.video_codec {
                    match codec {
                        VideoCodec::H264 => server_cb.set_video_codec_for_route(route, "H264"),
                        VideoCodec::H265 => server_cb.set_video_codec_for_route(route, "H265"),
                    }
                }
                drop(dep_guard);
                let mut w2 = watch_cb.lock();
                if w2.first_nal_at.is_none() {
                    w2.first_nal_at = Some(Instant::now());
                }
                drop(w2);
                let mut annex_chunk = Vec::new();
                for nal in nals {
                    stats_cb.lock().nals_emitted += 1;
                    annex_chunk.extend_from_slice(&ANNEXB);
                    annex_chunk.extend_from_slice(&nal);
                }
                server_cb.feed_video_annexb_for_route(route, &annex_chunk);
            }
        });

        let mut sdk = HikvisionClient::new(runtime.sdk_root_path.as_deref())?;
        crate::log_step!("relay", "pipeline_sdk_init_begin");
        sdk.init()?;
        crate::log_step!("relay", "pipeline_sdk_init_ok");
        crate::log_step!("relay", "pipeline_sdk_login_begin");
        sdk.login(&runtime.connection)?;
        crate::log_step!("relay", "pipeline_sdk_login_ok");

        crate::log_step!(
            "relay",
            "pipeline_preview_start_begin",
            "channel={} stream={}",
            sdk_channel,
            stream.as_str()
        );
        match sdk.start_preview_and_attach_callback(sdk_channel, stream, cb) {
            Ok(()) => {
                stats.lock().pipeline_start_count += 1;
                crate::log_step!("relay", "pipeline_preview_started");
                return Ok(PipelineCtx { sdk });
            }
            Err(e) => {
                let msg = e.to_string();
                stats.lock().last_sdk_error = Some(msg.clone());
                let code = e.sdk_code();
                crate::log_step!(
                    "relay",
                    "preview_attempt_failed",
                    "attempt={}/4 code={:?} stream={} channel={} message={}",
                    attempt,
                    code,
                    stream.as_str(),
                    sdk_channel,
                    msg
                );
                drop(sdk);
                if should_fallback_sub_to_main(code, stream, tried_sub_to_main) {
                    tried_sub_to_main = true;
                    stream = StreamType::Main;
                    crate::log_step!(
                        "relay",
                        "preview_fallback_sub_to_main",
                        "channel={} reason=NET_DVR_CHAN_NOTSUPPORT",
                        sdk_channel
                    );
                    last_err = Some(e);
                    continue;
                }
                if !should_retry_preview_attempt(code, attempt) {
                    return Err(e);
                }
                last_err = Some(e);
                let delay_ms = retry_delay_ms_for_code(code);
                sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| AppError::Sdk(String::from("start preview exhausted retries"))))
}

pub(crate) fn stop_pipeline(ctx: &mut PipelineCtx, _rtsp_server: &EmbeddedRtspServer) {
    crate::log_step!("relay", "pipeline_stopping", "reason=idle_or_shutdown");
    crate::log_step!("sdk", "stopping_preview");
    ctx.sdk.stop_preview();
    crate::log_step!("sdk", "preview_stopped");
    crate::log_step!("sdk", "logging_out");
    ctx.sdk.logout();
    crate::log_step!("sdk", "logged_out");
    crate::log_step!("sdk", "cleaning_up");
    ctx.sdk.cleanup();
    crate::log_step!("sdk", "cleaned_up");
    crate::log_step!("relay", "pipeline_stopped");
}
