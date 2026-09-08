//! The ring/file element: a single captured log record.

use log::Level;

/// One captured log line. `message` is **already redacted** by the time it lands
/// in the ring or the file (redaction happens at the single write choke point in
/// [`crate::tee::TeeLogger`]).
#[derive(Clone)]
pub struct LogLine {
    /// Capture timestamp, epoch milliseconds (local-clock derived).
    pub ts: i64,
    /// Severity.
    pub level: Level,
    /// `log` target (usually the module path).
    pub target: String,
    /// Redacted message text.
    pub message: String,
}

impl LogLine {
    /// Fixed-width uppercase level label (`ERROR`/`WARN`/`INFO`/`DEBUG`/`TRACE`).
    pub fn level_str(&self) -> &'static str {
        match self.level {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }

    /// Human-readable local timestamp (`YYYY-MM-DD HH:MM:SS.mmm`) from the epoch ms.
    /// Falls back to the raw epoch value if the timestamp is somehow out of range.
    pub fn format_ts(&self) -> String {
        use chrono::{Local, TimeZone};
        match Local.timestamp_millis_opt(self.ts).single() {
            Some(dt) => dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            None => self.ts.to_string(),
        }
    }
}
