//! One bounded run of consecutive, already-redacted log records.

use std::time::{Duration, Instant};

use crate::line::LogLine;

/// Continuing bursts remain visible without printing every duplicate.
const SUMMARY_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct ConsecutiveRecords {
    current: Option<RepeatRun>,
}

struct RepeatRun {
    line: LogLine,
    period_started: Instant,
    last_seen: Instant,
    repeats: u64,
}

impl RepeatRun {
    fn matches(&self, line: &LogLine) -> bool {
        self.line.level == line.level
            && self.line.target == line.target
            && self.line.message == line.message
    }

    fn summary(&self) -> Option<LogLine> {
        (self.repeats > 0).then(|| LogLine {
            ts: self.line.ts,
            level: self.line.level,
            target: self.line.target.clone(),
            message: format!(
                "[log] previous message repeated {} additional times over {} ms: {}",
                self.repeats,
                self.last_seen
                    .saturating_duration_since(self.period_started)
                    .as_millis(),
                self.line.message,
            ),
        })
    }
}

impl ConsecutiveRecords {
    /// Accept only visible, already-redacted records. The caller serializes this
    /// state transition AND its output to every sink under the same lock.
    /// Output is at most the previous summary followed by the new record.
    pub(crate) fn push(&mut self, line: LogLine, now: Instant) -> [Option<LogLine>; 2] {
        if let Some(run) = self.current.as_mut().filter(|run| run.matches(&line)) {
            run.line.ts = line.ts;
            run.last_seen = now;
            run.repeats = run.repeats.saturating_add(1);
            if now.saturating_duration_since(run.period_started) >= SUMMARY_INTERVAL {
                let summary = run.summary();
                run.repeats = 0;
                run.period_started = now;
                return [summary, None];
            }
            return [None, None];
        }

        let summary = self.flush();
        self.current = Some(RepeatRun {
            line: line.clone(),
            period_started: now,
            last_seen: now,
            repeats: 0,
        });
        [summary, Some(line)]
    }

    /// Flush ends the run, so the next record is always visible in full.
    pub(crate) fn flush(&mut self) -> Option<LogLine> {
        self.current.take().and_then(|run| run.summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;

    fn line(ts: i64, level: Level, target: &str, message: &str) -> LogLine {
        LogLine {
            ts,
            level,
            target: target.into(),
            message: message.into(),
        }
    }

    fn info(ts: i64, message: &str) -> LogLine {
        line(ts, Level::Info, "qbz", message)
    }

    fn messages(lines: [Option<LogLine>; 2]) -> Vec<String> {
        lines
            .into_iter()
            .flatten()
            .map(|line| line.message)
            .collect()
    }

    #[test]
    fn first_record_is_visible_and_consecutive_duplicates_are_counted() {
        let mut records = ConsecutiveRecords::default();
        let start = Instant::now();
        assert_eq!(
            messages(records.push(info(0, "loop mode"), start)),
            ["loop mode"]
        );
        for ms in [90, 180, 270] {
            assert!(messages(records.push(
                info(ms, "loop mode"),
                start + Duration::from_millis(ms as u64),
            ))
            .is_empty());
        }
        let output = records.push(
            info(300, "renderer changed"),
            start + Duration::from_millis(300),
        );
        assert_eq!(output[0].as_ref().unwrap().ts, 270);
        assert_eq!(
            messages(output),
            [
                "[log] previous message repeated 3 additional times over 270 ms: loop mode",
                "renderer changed",
            ]
        );
        assert_eq!(
            messages(records.push(info(400, "loop mode"), start + Duration::from_millis(400))),
            ["loop mode"]
        );
    }

    #[test]
    fn level_and_target_changes_end_the_run_even_with_identical_text() {
        for (level, target) in [(Level::Warn, "qbz"), (Level::Info, "qconnect")] {
            let mut records = ConsecutiveRecords::default();
            let start = Instant::now();
            records.push(info(0, "same"), start);
            records.push(info(1, "same"), start + Duration::from_millis(1));
            let output = records.push(
                line(2, level, target, "same"),
                start + Duration::from_millis(2),
            );
            let summary = output[0].as_ref().unwrap();
            assert_eq!(summary.level, Level::Info);
            assert_eq!(summary.target, "qbz");
            let changed = output[1].as_ref().unwrap();
            assert_eq!(changed.level, level);
            assert_eq!(changed.target, target);
            assert_eq!(changed.message, "same");
        }
    }

    #[test]
    fn warning_and_error_runs_preserve_severity_and_counts() {
        for level in [Level::Warn, Level::Error] {
            let mut records = ConsecutiveRecords::default();
            let start = Instant::now();
            let output = records.push(line(0, level, "qbz", "failed"), start);
            assert_eq!(output[1].as_ref().unwrap().level, level);
            records.push(
                line(10, level, "qbz", "failed"),
                start + Duration::from_millis(10),
            );
            let summary = records.flush().unwrap();
            assert_eq!(summary.level, level);
            assert_eq!(
                summary.message,
                "[log] previous message repeated 1 additional times over 10 ms: failed"
            );
        }
    }

    #[test]
    fn periodic_summaries_count_only_new_repetitions_and_retain_the_key() {
        let mut records = ConsecutiveRecords::default();
        let start = Instant::now();
        records.push(info(0, "busy"), start);
        for second in 1..=11 {
            let output = messages(records.push(
                info(second * 1000, "busy"),
                start + Duration::from_secs(second as u64),
            ));
            if second % 5 == 0 {
                assert_eq!(
                    output,
                    ["[log] previous message repeated 5 additional times over 5000 ms: busy"]
                );
            } else {
                assert!(output.is_empty());
            }
        }
        assert_eq!(
            records.flush().unwrap().message,
            "[log] previous message repeated 1 additional times over 1000 ms: busy"
        );
        assert!(records.flush().is_none());
    }

    #[test]
    fn flush_resets_the_run_and_does_not_invent_duplicates() {
        let mut records = ConsecutiveRecords::default();
        let start = Instant::now();
        assert!(records.flush().is_none());
        records.push(info(0, "a"), start);
        assert!(records.flush().is_none());
        assert_eq!(messages(records.push(info(1, "a"), start)), ["a"]);
        records.push(info(2, "a"), start);
        assert!(records.flush().is_some());
        assert_eq!(messages(records.push(info(3, "a"), start)), ["a"]);
        assert_eq!(messages(records.push(info(4, "b"), start)), ["b"]);
    }

    #[test]
    fn different_record_after_periodic_summary_does_not_repeat_the_count() {
        let mut records = ConsecutiveRecords::default();
        let start = Instant::now();
        records.push(info(0, "a"), start);
        let output = records.push(info(5000, "a"), start + Duration::from_secs(5));
        assert_eq!(messages(output).len(), 1);
        assert_eq!(
            messages(records.push(info(5001, "b"), start + Duration::from_millis(5001))),
            ["b"]
        );
        assert!(records.flush().is_none());
    }

    #[test]
    fn duration_is_monotonic_and_ends_at_last_duplicate_not_later_message() {
        let mut records = ConsecutiveRecords::default();
        let start = Instant::now();
        records.push(info(1000, "a"), start);
        // A wall-clock correction must not produce a negative or giant interval.
        records.push(info(500, "a"), start + Duration::from_millis(90));
        let output = records.push(info(9000, "b"), start + Duration::from_secs(10));
        let summary = output[0].as_ref().unwrap();
        assert_eq!(summary.ts, 500);
        assert_eq!(
            summary.message,
            "[log] previous message repeated 1 additional times over 90 ms: a"
        );
    }

    #[test]
    fn redacted_records_share_a_key_without_retaining_raw_secrets() {
        let mut records = ConsecutiveRecords::default();
        let start = Instant::now();
        let first = crate::redact("token=SEKRET_ONE");
        let second = crate::redact("token=SEKRET_TWO");
        assert_eq!(first, second);
        assert_eq!(messages(records.push(info(0, &first), start)), [first]);
        assert!(messages(records.push(info(1, &second), start)).is_empty());
        let summary = records.flush().unwrap();
        assert!(summary.message.contains("***REDACTED***"));
        assert!(!summary.message.contains("SEKRET"));
    }
}
