use clap::Parser;

use crate::error::AppResult;
use crate::logging::mask_secret;

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub ip: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct RelayConfig {
    pub listen_host: String,
    pub listen_port: u16,
    pub listen_path: String,
    pub probe_timeout_ms: u64,
    pub stale_after_ms: u64,
    pub idle_grace_ms: u64,
    pub restart_on_fail: bool,
    pub max_restarts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    Main,
    Sub,
}

impl StreamType {
    pub fn as_str(self) -> &'static str {
        match self {
            StreamType::Main => "main",
            StreamType::Sub => "sub",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub connection: ConnectionConfig,
    pub sdk_root_path: Option<String>,
    pub include_raw_config: bool,
}

#[derive(Debug, Clone, Copy)]
struct Defaults;

impl Defaults {
    const HOST: &'static str = "127.0.0.1";
    const PORT: u16 = 8000;
    const USER: &'static str = "admin";
    const PASSWORD: &'static str = "";

    const RELAY_LISTEN_HOST: &'static str = "127.0.0.1";
    const RELAY_LISTEN_PORT: u16 = 8554;
    const RELAY_LISTEN_PATH: &'static str = "/live";
    const RELAY_PROBE_TIMEOUT_MS: u64 = 15_000;
    const RELAY_STALE_AFTER_MS: u64 = 10_000;
    const RELAY_IDLE_GRACE_MS: u64 = 5_000;
    const RELAY_MAX_RESTARTS: u32 = 5;
    const RELAY_RESTART_ON_FAIL: bool = false;

    const INCLUDE_RAW_CONFIG: bool = false;
}

#[derive(Debug, Parser)]
#[command(
    name = "hikvision",
    disable_help_subcommand = true,
    after_help = "Examples:\n  hikvision\n  hikvision --json\n  hikvision --host 192.168.1.64 --port 8000 --user admin --password \"secret\" --sdk-path \"D:\\sdk\\EN-HCNetSDKV6.1.9.4_build20220412_win64\"\n  hikvision --rtsp-relay --host 192.168.1.64 --port 8000 --user admin --password \"secret\" --sdk-path \"/opt/hikvision/EN-HCNetSDKV6.1.9.4_build20220412_linux64\""
)]
struct Cli {
    #[arg(long = "host", default_value = Defaults::HOST, help = "DVR/NVR IP address")]
    host: String,
    #[arg(long = "port", default_value_t = Defaults::PORT, help = "DVR/NVR SDK port")]
    port: u16,
    #[arg(long = "user", default_value = Defaults::USER, help = "DVR/NVR username")]
    user: String,
    #[arg(long = "password", default_value = Defaults::PASSWORD, help = "DVR/NVR password")]
    password: String,
    #[arg(long = "rtsp-relay", help = "Run embedded RTSP relay mode")]
    rtsp_relay: bool,
    #[arg(long = "listen-host", default_value = Defaults::RELAY_LISTEN_HOST, help = "Relay bind host")]
    listen_host: String,
    #[arg(long = "listen-port", default_value_t = Defaults::RELAY_LISTEN_PORT, help = "Relay bind port")]
    listen_port: u16,
    #[arg(long = "listen-path", default_value = Defaults::RELAY_LISTEN_PATH, help = "Relay route prefix")]
    listen_path: String,
    #[arg(long = "probe-timeout-ms", default_value_t = Defaults::RELAY_PROBE_TIMEOUT_MS, help = "Time to wait for first data before watchdog restart (ms)")]
    probe_timeout_ms: u64,
    #[arg(long = "stale-after-ms", default_value_t = Defaults::RELAY_STALE_AFTER_MS, help = "No-stream-data timeout before watchdog restart (ms)")]
    stale_after_ms: u64,
    #[arg(long = "idle-grace-ms", default_value_t = Defaults::RELAY_IDLE_GRACE_MS, help = "Delay before stopping idle route pipeline (ms)")]
    idle_grace_ms: u64,
    #[arg(long = "restart-on-fail", default_value_t = Defaults::RELAY_RESTART_ON_FAIL, help = "Enable automatic pipeline restarts on recoverable failures")]
    restart_on_fail: bool,
    #[arg(long = "max-restarts", default_value_t = Defaults::RELAY_MAX_RESTARTS, help = "Maximum automatic restarts per route")]
    max_restarts: u32,
    #[arg(long = "include-raw-config", default_value_t = Defaults::INCLUDE_RAW_CONFIG, help = "Include raw device config blob in list output")]
    include_raw_config: bool,
    #[arg(
        long = "sdk-path",
        help = "Path to Hikvision SDK root (must contain incEn/ and lib/). Can also be set using environment variable HIKVISION_SDK_PATH"
    )]
    sdk_path: Option<String>,
    #[arg(long = "verbose", help = "Enable detailed diagnostic logs")]
    verbose: bool,
    #[arg(long = "json", help = "Output list mode in JSON format (default is human-readable text)")]
    json: bool,
}

#[derive(Debug, Clone)]
pub enum Mode {
    List,
    Relay(RelayConfig),
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub runtime: RuntimeConfig,
    pub mode: Mode,
    pub verbose: bool,
    pub json: bool,
}

fn parse_relay_mode(cli: &Cli) -> Mode {
    if !cli.rtsp_relay {
        return Mode::List;
    }
    let listen_host = cli.listen_host.clone();
    let listen_port = cli.listen_port;
    let mut listen_path = cli.listen_path.clone();
    if !listen_path.starts_with('/') {
        listen_path = format!("/{listen_path}");
    }
    Mode::Relay(RelayConfig {
        listen_host,
        listen_port,
        listen_path,
        probe_timeout_ms: cli.probe_timeout_ms,
        stale_after_ms: cli.stale_after_ms,
        idle_grace_ms: cli.idle_grace_ms,
        restart_on_fail: cli.restart_on_fail,
        max_restarts: cli.max_restarts,
    })
}

pub fn load_config() -> AppResult<AppConfig> {
    let cli = Cli::parse();

    let ip = cli.host.clone();
    let port = cli.port;
    let user = cli.user.clone();
    let password = cli.password.clone();
    crate::log_step!(
        "cli",
        "env_validation_ok",
        "ip={} port={} user={} password={}",
        ip,
        port,
        user,
        mask_secret(&password)
    );

    let runtime = RuntimeConfig {
        connection: ConnectionConfig {
            ip,
            port,
            user,
            password,
        },
        sdk_root_path: cli
            .sdk_path
            .clone()
            .or_else(|| std::env::var("HIKVISION_SDK_PATH").ok())
            .filter(|v| !v.is_empty()),
        include_raw_config: cli.include_raw_config,
    };
    let mode = parse_relay_mode(&cli);

    Ok(AppConfig {
        runtime,
        mode,
        verbose: cli.verbose,
        json: cli.json,
    })
}
