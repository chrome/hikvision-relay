pub mod depacketizer;
pub mod pipeline_supervisor;
pub mod rtsp;
pub mod rtsp_server;
pub mod rtsp_transport;
pub mod watchdog_policy;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::signal;
use tokio::time::{interval, Duration, Instant, MissedTickBehavior};

use crate::config::{RelayConfig, RuntimeConfig};
use crate::error::{AppError, AppResult};
use crate::hikvision::client::RelayStats;
use crate::list_streams::list_streams;
use crate::relay::pipeline_supervisor::{
    maybe_restart_pipeline, start_pipeline_with_retries, stop_pipeline, PipelineCtx,
};
use crate::relay::rtsp_server::{EmbeddedRtspServer, RtspEvent};
use crate::relay::rtsp::protocol::{RouteKey, RouteStream};
use crate::relay::watchdog_policy::{restart_pipeline_after_watchdog, PipelineWatchdog};

async fn wait_shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = signal::ctrl_c().await;
                return "signal:SIGINT";
            }
        };
        tokio::select! {
            _ = signal::ctrl_c() => "signal:SIGINT",
            _ = sigterm.recv() => "signal:SIGTERM",
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
        "signal:SIGINT"
    }
}

fn stream_type_for_route(route: RouteKey) -> crate::config::StreamType {
    match route.stream {
        RouteStream::Main => crate::config::StreamType::Main,
        RouteStream::Sub => crate::config::StreamType::Sub,
    }
}

fn sdk_channel_for_route(channel_map: &HashMap<u32, i32>, route: RouteKey) -> Option<i32> {
    channel_map.get(&route.channel).copied()
}

fn idle_deadline_from(now: Instant, idle_grace_ms: u64) -> Instant {
    now + Duration::from_millis(idle_grace_ms)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogAction {
    None,
    RestartNoCallbackData,
    RestartNoH264Nals,
    RestartStaleStream,
}

fn classify_watchdog_action(w: &PipelineWatchdog, now: Instant, probe_ms: u64, stale_ms: u64) -> WatchdogAction {
    let Some(started) = w.pipeline_started_at else {
        return WatchdogAction::None;
    };
    if w.first_stream_data.is_none() && now.duration_since(started) > Duration::from_millis(probe_ms) {
        return WatchdogAction::RestartNoCallbackData;
    }
    if let Some(fd) = w.first_stream_data {
        if w.first_nal_at.is_none() && now.duration_since(fd) > Duration::from_millis(probe_ms) {
            return WatchdogAction::RestartNoH264Nals;
        }
    }
    if let Some(last) = w.last_stream_data {
        if now.duration_since(last) > Duration::from_millis(stale_ms) {
            return WatchdogAction::RestartStaleStream;
        }
    }
    WatchdogAction::None
}

pub async fn run_relay(runtime: RuntimeConfig, relay_cfg: RelayConfig) -> AppResult<()> {
    crate::log_step!(
        "cli",
        "preparing_relay_sdk",
        "dynamicRoute={} listen={}:{}{}",
        "/live/<channel>/<main|sub>",
        relay_cfg.listen_host,
        relay_cfg.listen_port,
        relay_cfg.listen_path
    );
    let list = list_streams(
        &runtime.connection,
        runtime.sdk_root_path.as_deref(),
        runtime.include_raw_config,
    )
    .await?;
    let channel_map: HashMap<u32, i32> = list
        .streams
        .iter()
        .map(|s| (s.channel_no as u32, s.sdk_channel as i32))
        .collect();

    let mut server = EmbeddedRtspServer::new(
        relay_cfg.listen_host.clone(),
        relay_cfg.listen_port,
        relay_cfg.listen_path.clone(),
    );
    server.start().await?;
    print_relay_start_banner(&relay_cfg, &channel_map, server.build_bind_url());

    struct RouteRuntime {
        pipeline: Option<PipelineCtx>,
        stats: Arc<Mutex<RelayStats>>,
        watch: Arc<Mutex<PipelineWatchdog>>,
        idle_deadline: Option<Instant>,
        restart_count: u32,
    }

    impl RouteRuntime {
        fn new() -> Self {
            Self {
                pipeline: None,
                stats: Arc::new(Mutex::new(RelayStats {
                    pipeline_start_count: 0,
                    pipeline_stop_count: 0,
                    sdk_chunks: 0,
                    sdk_bytes: 0,
                    rtp_packets: 0,
                    nals_emitted: 0,
                    last_sdk_error: None,
                })),
                watch: Arc::new(Mutex::new(PipelineWatchdog::default())),
                idle_deadline: None,
                restart_count: 0,
            }
        }
    }

    let mut event_rx = server.subscribe();
    let mut routes: HashMap<RouteKey, RouteRuntime> = HashMap::new();
    let mut idle_poll = interval(Duration::from_millis(250));
    idle_poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut watchdog = interval(Duration::from_secs(2));
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let _ = watchdog.tick().await;
    let _ = idle_poll.tick().await;

    let shutdown_signal = wait_shutdown_signal();
    tokio::pin!(shutdown_signal);
    let shutdown_reason = loop {
        tokio::select! {
            reason = &mut shutdown_signal => {
                crate::log_step!("cli", "received_signal", "{reason}");
                break reason;
            }
            ev = event_rx.recv() => {
                match ev {
                    Ok(RtspEvent::FirstClientConnected { route }) => {
                        let sdk_ch = match sdk_channel_for_route(&channel_map, route) {
                            Some(ch) => ch,
                            None => continue,
                        };
                        let entry = routes.entry(route).or_insert_with(RouteRuntime::new);
                        entry.idle_deadline = None;
                        if entry.pipeline.is_none() {
                            let stream = stream_type_for_route(route);
                            match start_pipeline_with_retries(
                                &runtime,
                                &relay_cfg,
                                sdk_ch,
                                stream,
                                route,
                                &server,
                                entry.stats.clone(),
                                entry.watch.clone(),
                            ).await {
                                Ok(ctx) => {
                                    entry.watch.lock().pipeline_started_at = Some(Instant::now());
                                    entry.pipeline = Some(ctx);
                                }
                                Err(e) => {
                                    entry.stats.lock().last_sdk_error = Some(e.to_string());
                                    return Err(e);
                                }
                            }
                        }
                    }
                    Ok(RtspEvent::LastClientGone { route }) => {
                        crate::log_step!(
                            "relay",
                            "last_client_gone",
                            "idleGraceMs={} route={}/{}",
                            relay_cfg.idle_grace_ms
                            ,route.channel, route.stream.as_str()
                        );
                        if let Some(entry) = routes.get_mut(&route) {
                            entry.idle_deadline = Some(idle_deadline_from(Instant::now(), relay_cfg.idle_grace_ms));
                        }
                    }
                    Ok(RtspEvent::ClientPlaying { route }) => {
                        crate::log_step!(
                            "relay",
                            "rtsp_client_playing",
                            "route={}/{}",
                            route.channel,
                            route.stream.as_str()
                        );
                    }
                    Ok(RtspEvent::ParameterSetsReady { route }) => {
                        crate::log_step!(
                            "relay",
                            "ready",
                            "bindUrl={} route={}/{}",
                            server.build_bind_url(),
                            route.channel,
                            route.stream.as_str()
                        );
                    }
                    Ok(RtspEvent::Error(err)) => {
                        return Err(AppError::Rtsp(err));
                    }
                    Err(_) => {}
                }
            }
            _ = idle_poll.tick() => {
                let now = Instant::now();
                for entry in routes.values_mut() {
                    if let Some(deadline) = entry.idle_deadline {
                        if now >= deadline {
                            if let Some(mut ctx) = entry.pipeline.take() {
                                crate::log_step!("relay", "idle_timeout_reached_stopping_pipeline");
                                stop_pipeline(&mut ctx, &server);
                                let mut g = entry.stats.lock();
                                g.pipeline_stop_count += 1;
                            }
                            entry.idle_deadline = None;
                        }
                    }
                }
            }
            _ = watchdog.tick() => {
                let now = Instant::now();
                let probe = relay_cfg.probe_timeout_ms;
                let stale = relay_cfg.stale_after_ms;
                for (route, entry) in &mut routes {
                    if entry.pipeline.is_none() {
                        continue;
                    }
                    let w = entry.watch.lock();
                    let action = classify_watchdog_action(&w, now, probe, stale);
                    drop(w);
                    match action {
                        WatchdogAction::None => {}
                        WatchdogAction::RestartNoCallbackData => {
                            crate::log_step!("watchdog", "no_callback_data_within_probe_timeout");
                            restart_pipeline_after_watchdog(
                                &mut entry.pipeline,
                                &server,
                                &entry.stats,
                                &entry.watch,
                                "no_callback_data",
                            );
                            let sdk_ch = match sdk_channel_for_route(&channel_map, *route) {
                                Some(ch) => ch,
                                None => continue,
                            };
                            let stream = stream_type_for_route(*route);
                            maybe_restart_pipeline(
                                &mut entry.pipeline,
                                &runtime,
                                &relay_cfg,
                                sdk_ch,
                                stream,
                                *route,
                                &server,
                                &entry.stats,
                                &entry.watch,
                                &mut entry.restart_count,
                            )
                            .await?;
                            continue;
                        }
                        WatchdogAction::RestartNoH264Nals => {
                            crate::log_step!("watchdog", "no_h264_nals_extracted_within_probe_timeout");
                            restart_pipeline_after_watchdog(
                                &mut entry.pipeline,
                                &server,
                                &entry.stats,
                                &entry.watch,
                                "no_h264_nals_extracted",
                            );
                            continue;
                        }
                        WatchdogAction::RestartStaleStream => {
                            crate::log_step!("watchdog", "stale_sdk_stream");
                            restart_pipeline_after_watchdog(
                                &mut entry.pipeline,
                                &server,
                                &entry.stats,
                                &entry.watch,
                                "stale_sdk_stream",
                            );
                        }
                    }
                }
            }
        }
    };

    for entry in routes.values_mut() {
        if let Some(mut ctx) = entry.pipeline.take() {
            stop_pipeline(&mut ctx, &server);
        }
    }
    server.stop().await;
    println!();
    println!("Relay stopped: {shutdown_reason}");
    Ok(())
}

fn print_relay_start_banner(relay_cfg: &RelayConfig, channel_map: &HashMap<u32, i32>, bind_url: String) {
    let host = advertised_host(relay_cfg);
    let mut channels: Vec<u32> = channel_map.keys().copied().collect();
    channels.sort_unstable();

    println!("RTSP relay is running");
    println!("Bind address: {bind_url}");
    println!("Route pattern: /live/<channel>/<main|sub>");
    println!("Idle stop timeout: {} ms", relay_cfg.idle_grace_ms);
    println!();
    if channels.is_empty() {
        println!("No available channels found.");
        return;
    }

    println!("Available stream URLs:");
    for channel in channels {
        let main = format!(
            "rtsp://{}:{}{}/{}/main",
            host, relay_cfg.listen_port, relay_cfg.listen_path, channel
        );
        let sub = format!(
            "rtsp://{}:{}{}/{}/sub",
            host, relay_cfg.listen_port, relay_cfg.listen_path, channel
        );
        println!("- channel {channel}");
        println!("  main: {main}");
        println!("  sub : {sub}");
    }
}

fn advertised_host(relay_cfg: &RelayConfig) -> &str {
    if relay_cfg.listen_host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        relay_cfg.listen_host.as_str()
    }
}
