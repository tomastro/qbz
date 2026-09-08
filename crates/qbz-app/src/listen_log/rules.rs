//! Pure reading rules over listen events.
//!
//! The store applies NO threshold when it writes (LMS/Astra): a skip is a row
//! with little `played_ms`, never an absent row. Whatever wants to count
//! "plays" or "skips" applies its bar here, so two consumers can never
//! disagree about what a play is because one of them wrote and the other
//! read (the Amperfy/Retro trap).

/// Why a listen event closed. `Open` is the in-flight state (NULL on disk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// The track played to its end.
    Natural,
    /// Replaced by another track before its end (next / previous / play X).
    Skip,
    /// Playback stopped with nothing following.
    Stop,
    /// The stream failed.
    Error,
    /// The app went away with the row open (closed on the next start, or on
    /// an orderly exit).
    Shutdown,
    /// Playback moved to another device (cast / peer). Reserved — v1 never
    /// records those tracks, but the word is fixed so readers can rely on it.
    Handoff,
}

/// ONE list, read by both directions: `as_str` and `parse` must never drift
/// (the `AudioFormat::Dsd` → `Unknown` lesson: a writer and a reader that each
/// spell the enum are two lists).
const END_REASONS: &[(EndReason, &str)] = &[
    (EndReason::Natural, "natural"),
    (EndReason::Skip, "skip"),
    (EndReason::Stop, "stop"),
    (EndReason::Error, "error"),
    (EndReason::Shutdown, "shutdown"),
    (EndReason::Handoff, "handoff"),
];

impl EndReason {
    pub fn as_str(self) -> &'static str {
        END_REASONS
            .iter()
            .find(|(r, _)| *r == self)
            .map(|(_, s)| *s)
            .expect("every EndReason is in END_REASONS")
    }

    pub fn parse(s: &str) -> Option<Self> {
        END_REASONS.iter().find(|(_, t)| *t == s).map(|(r, _)| *r)
    }
}

/// Last.fm / MusicBee bar: the track is longer than 30 s and at least half of
/// it (capped at 4 minutes) was audible.
pub fn is_play(played_ms: u64, duration_ms: u64) -> bool {
    duration_ms > 30_000 && played_ms >= (duration_ms / 2).min(240_000)
}

/// beets bar: it was replaced before 85 % of it was heard. A track that was
/// replaced at 90 % is a play that happened to end early, not a skip.
pub fn is_skip(end_reason: Option<EndReason>, played_ms: u64, duration_ms: u64) -> bool {
    end_reason == Some(EndReason::Skip) && (played_ms as f64) < 0.85 * duration_ms as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_reason_round_trips_every_variant() {
        for (reason, word) in END_REASONS {
            assert_eq!(reason.as_str(), *word);
            assert_eq!(EndReason::parse(word), Some(*reason));
        }
        assert_eq!(EndReason::parse("nope"), None);
        assert_eq!(END_REASONS.len(), 6);
    }

    #[test]
    fn is_play_thirty_second_floor() {
        // 30 s exactly is NOT longer than 30 s.
        assert!(!is_play(30_000, 30_000));
        // 30.001 s: half is 15 s (integer ms).
        assert!(is_play(15_000, 30_001));
        assert!(!is_play(14_999, 30_001));
    }

    #[test]
    fn is_play_half_or_four_minutes() {
        // 100 s track: half = 50 s.
        assert!(is_play(50_000, 100_000));
        assert!(!is_play(49_999, 100_000));
        // 10 min track: half = 300 s, capped at 240 s.
        assert!(is_play(240_000, 600_000));
        assert!(!is_play(239_999, 600_000));
        // Exactly 480 s: half == cap.
        assert!(is_play(240_000, 480_000));
    }

    #[test]
    fn is_play_unknown_duration_never_counts() {
        assert!(!is_play(1_000_000, 0));
    }

    #[test]
    fn is_skip_needs_skip_reason_and_under_85_percent() {
        let d = 200_000;
        assert!(is_skip(Some(EndReason::Skip), 10_000, d));
        assert!(is_skip(Some(EndReason::Skip), 169_999, d));
        // 85 % exactly is not a skip.
        assert!(!is_skip(Some(EndReason::Skip), 170_000, d));
        assert!(!is_skip(Some(EndReason::Natural), 10_000, d));
        assert!(!is_skip(Some(EndReason::Stop), 10_000, d));
        assert!(!is_skip(None, 10_000, d));
    }

    #[test]
    fn is_skip_zero_duration_is_never_a_skip() {
        // 0 < 0.85 * 0 is false: an unknown-duration row cannot be a skip.
        assert!(!is_skip(Some(EndReason::Skip), 0, 0));
    }
}
