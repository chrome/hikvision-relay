use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use time::format_description::well_known::Iso8601;
use time::OffsetDateTime;

/// Stderr lock so concurrent log lines from multiple threads don't interleave.
static STDERR_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static VERBOSE_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(enabled: bool) {
    VERBOSE_ENABLED.store(enabled, Ordering::Relaxed);
}

fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Iso8601::DEFAULT)
        .unwrap_or_else(|_| String::from("0000-00-00T00:00:00Z"))
}

/// Logs a single step in the format used by the original JS app:
///   `[ISO-8601] [scope] step | details\n`
pub fn log_step(scope: &str, step: &str, details: &str) {
    if !VERBOSE_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let _g = match STDERR_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let suffix = if details.is_empty() {
        String::new()
    } else {
        format!(" | {}", details)
    };
    let line = format!("[{}] [{}] {}{}\n", timestamp(), scope, step, suffix);
    let _ = std::io::stderr().write_all(line.as_bytes());
}

/// Masks a secret string with `*` characters, capped at 8 chars (matches JS).
pub fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return String::from("<empty>");
    }
    let len = value.chars().count().min(8);
    "*".repeat(len)
}

#[macro_export]
macro_rules! log_step {
    ($scope:expr, $step:expr) => {
        $crate::logging::log_step($scope, $step, "")
    };
    ($scope:expr, $step:expr, $($arg:tt)*) => {
        $crate::logging::log_step($scope, $step, &format!($($arg)*))
    };
}
