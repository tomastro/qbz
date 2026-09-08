//! The composite [`log::Log`] that fans every record to stderr, the ring, and the file.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::Mutex;
use std::time::Instant;

use log::{Log, Metadata, Record};

use crate::line::LogLine;
use crate::repeat::ConsecutiveRecords;
use crate::{redact, ring};

/// Wraps `env_logger`'s built `Logger` and tees every record to the in-memory ring and
/// (optionally) the on-disk log file, with secret redaction applied once at this single
/// write choke point. **All** sinks (ring, file, and stderr) receive the redacted text.
/// Consecutive duplicate records are summarized with their original severity,
/// target, redacted message, repetition count and monotonic elapsed time.
pub struct TeeLogger {
    inner: env_logger::Logger,
    // One lock covers grouping AND all sink writes: concurrent threads cannot
    // interleave a summary and its next record or reorder the file vs. the ring.
    output: Mutex<Output>,
}

struct Output {
    file: Option<BufWriter<File>>,
    consecutive: ConsecutiveRecords,
}

impl TeeLogger {
    pub(crate) fn new(inner: env_logger::Logger, file: Option<BufWriter<File>>) -> Self {
        Self {
            inner,
            output: Mutex::new(Output {
                file,
                consecutive: ConsecutiveRecords::default(),
            }),
        }
    }
}

impl Output {
    fn write(&mut self, line: LogLine) {
        let formatted = format_line(&line);
        ring::push(line);
        if let Some(writer) = &mut self.file {
            let _ = writeln!(writer, "{formatted}");
        }
        // Never delegate to inner.log(record): it would bypass redaction.
        let _ = writeln!(std::io::stderr(), "{formatted}");
    }
}

fn now_epoch_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// Format a redacted log line the same way the file sink does (stable, greppable).
fn format_line(line: &LogLine) -> String {
    format!(
        "{} {:5} {} {}",
        line.format_ts(),
        line.level_str(),
        line.target,
        line.message
    )
}

impl Log for TeeLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        // Honor the inner logger's filter so the ring matches what stderr would show.
        if !self.inner.enabled(record.metadata()) {
            return;
        }

        // Redact ONCE; every downstream sink gets the cleaned text.
        let msg = redact::redact(&record.args().to_string());
        let mut output = self.output.lock().unwrap_or_else(|p| p.into_inner());
        let line = LogLine {
            ts: now_epoch_ms(),
            level: record.level(),
            target: record.target().to_owned(),
            message: msg,
        };
        let [summary, first] = output.consecutive.push(line, Instant::now());
        let summarized = summary.is_some();
        for line in [summary, first].into_iter().flatten() {
            output.write(line);
        }
        // Compaction may no longer fill BufWriter for minutes. Make periodic
        // summaries visible to a live file tail as well as stderr/the ring.
        if summarized {
            if let Some(writer) = &mut output.file {
                let _ = writer.flush();
            }
        }
    }

    fn flush(&self) {
        let mut output = self.output.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(summary) = output.consecutive.flush() {
            output.write(summary);
        }
        self.inner.flush();
        if let Some(writer) = &mut output.file {
            let _ = writer.flush();
        }
        let _ = std::io::stderr().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;

    #[test]
    fn format_line_includes_redacted_message_not_raw() {
        let line = LogLine {
            ts: 0,
            level: Level::Info,
            target: "qbz".into(),
            message: "token=***REDACTED***".into(),
        };
        let s = format_line(&line);
        assert!(s.contains("***REDACTED***"));
        assert!(!s.contains("SEKRET"));
    }
}
