mod config;
mod error;
mod hikvision;
mod list_streams;
mod logging;
mod relay;

use std::process::ExitCode;

use serde_json::json;

use crate::config::{load_config, Mode};
use crate::error::AppResult;
use crate::list_streams::ListStreamsResult;

fn success_list_json(result: &ListStreamsResult) -> serde_json::Value {
    json!({
        "ok": true,
        "count": result.streams.len(),
        "streams": result.streams,
        "deviceSettings": result.device_settings,
    })
}

fn error_json(details: &str) -> serde_json::Value {
    json!({
        "ok": false,
        "error": "Hikvision CLI execution failed.",
        "details": details,
    })
}

fn emit_json(value: serde_json::Value, to_stderr: bool) {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| String::from("{\"ok\":false}"));
    if to_stderr {
        eprintln!("{text}");
    } else {
        println!("{text}");
    }
}

async fn run() -> AppResult<()> {
    let _ = dotenvy::dotenv();
    let cfg = load_config()?;
    crate::logging::set_verbose(cfg.verbose);
    crate::log_step!("cli", "startup");

    match cfg.mode {
        Mode::List => {
            crate::log_step!("cli", "starting_stream_listing");
            let result = list_streams::list_streams(
                &cfg.runtime.connection,
                cfg.runtime.sdk_root_path.as_deref(),
                cfg.runtime.include_raw_config,
            )
            .await?;
            crate::log_step!("cli", "stream_listing_complete", "count={}", result.streams.len());
            if cfg.json {
                emit_json(success_list_json(&result), false);
            } else {
                emit_list_human(&result);
            }
            Ok(())
        }
        Mode::Relay(relay_cfg) => {
            relay::run_relay(cfg.runtime, relay_cfg).await?;
            Ok(())
        }
    }
}

fn render_list_human(result: &ListStreamsResult) -> String {
    let mut lines = vec![
        String::from("Hikvision device info"),
        format!("Streams found: {}", result.streams.len()),
        format!(
            "Config version: {} ({} bytes)",
            result.device_settings.config_read.version, result.device_settings.config_read.bytes_returned
        ),
        format!(
            "Enabled devices/channels: {}/{}",
            result.device_settings.enabled_ip_devices.len(),
            result.device_settings.enabled_ip_channels.len()
        ),
    ];
    if result.streams.is_empty() {
        lines.push(String::from("No enabled digital streams found."));
        return lines.join("\n");
    }
    lines.push(String::new());
    lines.push(String::from("Available streams:"));
    for s in &result.streams {
        lines.push(format!(
            "- channel {} -> sdkChannel {} ({}, sourceDeviceId={}, sourceChannel={}, transport={})",
            s.channel_no,
            s.sdk_channel,
            s.stream_source_type,
            s.source.ip_device_id,
            s.source.source_channel,
            s.source.transport_protocol
        ));
    }
    lines.join("\n")
}

fn emit_list_human(result: &ListStreamsResult) {
    println!("{}", render_list_human(result));
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            crate::log_step!("cli", "execution_failed", "{err}");
            emit_json(error_json(&err.to_string()), true);
            ExitCode::from(1)
        }
    }
}
