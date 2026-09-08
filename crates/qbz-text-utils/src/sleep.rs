//! Sleep-timer countdown formatting (1:1 with the Tauri `formatSleepTimerRemaining`).

/// Format remaining seconds like the Tauri sleep timer.
///
/// - `h > 0`  -> `"{h}h {m}m"`
/// - `m > 0`  -> `"{m}m {s}s"` only when under 5 minutes and there are stray seconds, else `"{m}m"`
/// - else     -> `"{s}s"`
/// - `<= 0`   -> `"0s"`
///
/// Seconds are shown only under 5 minutes, matching `sleepTimerStore.ts`.
pub fn format_sleep_remaining(secs: i64) -> String {
    if secs <= 0 {
        return "0s".to_string();
    }
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        if s > 0 && m < 5 {
            format!("{m}m {s}s")
        } else {
            format!("{m}m")
        }
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_sleep_remaining as f;

    #[test]
    fn under_a_minute() {
        assert_eq!(f(45), "45s");
        assert_eq!(f(1), "1s");
    }

    #[test]
    fn at_or_below_zero() {
        assert_eq!(f(0), "0s");
        assert_eq!(f(-3), "0s");
    }

    #[test]
    fn under_five_min_shows_secs() {
        assert_eq!(f(125), "2m 5s");
        assert_eq!(f(61), "1m 1s");
    }

    #[test]
    fn exact_minute_no_stray_secs() {
        assert_eq!(f(120), "2m");
    }

    #[test]
    fn five_min_or_more_minutes_only() {
        assert_eq!(f(305), "5m");
        assert_eq!(f(300), "5m");
        assert_eq!(f(599), "9m");
    }

    #[test]
    fn hours() {
        assert_eq!(f(3725), "1h 2m");
        assert_eq!(f(3600), "1h 0m");
    }
}
