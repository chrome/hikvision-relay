use std::sync::Arc;

use parking_lot::Mutex;
use tokio::time::Instant;

use crate::hikvision::client::RelayStats;
use crate::relay::pipeline_supervisor::{stop_pipeline, PipelineCtx};
use crate::relay::rtsp_server::EmbeddedRtspServer;

#[derive(Clone, Default)]
pub(crate) struct PipelineWatchdog {
    pub(crate) pipeline_started_at: Option<Instant>,
    pub(crate) first_stream_data: Option<Instant>,
    pub(crate) first_nal_at: Option<Instant>,
    pub(crate) last_stream_data: Option<Instant>,
}

pub(crate) fn restart_pipeline_after_watchdog(
    pipeline: &mut Option<PipelineCtx>,
    server: &EmbeddedRtspServer,
    stats: &Arc<Mutex<RelayStats>>,
    watch: &Arc<Mutex<PipelineWatchdog>>,
    reason: &str,
) {
    if let Some(mut ctx) = pipeline.take() {
        stop_pipeline(&mut ctx, server);
        let mut g = stats.lock();
        g.pipeline_stop_count += 1;
        g.last_sdk_error = Some(reason.to_string());
    }
    *watch.lock() = PipelineWatchdog::default();
}
