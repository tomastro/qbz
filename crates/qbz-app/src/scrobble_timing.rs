//! Pure scrobble arming rules (no UI / network).
//!
//! Shared so the Slint shell and unit tests agree on when a delayed scrobble
//! may fire. Unknown duration (`0`) must never arm an immediate scrobble.

/// `min(50% of duration, 240s)` in seconds, per the Last.fm scrobbling rules
/// (track longer than 30 seconds, play half or 4 minutes), applied to both
/// Last.fm and ListenBrainz.
///
/// Returns `None` when duration is unknown (`0`) or 30 seconds or less:
/// unknown durations would pollute history for local/Plex metadata holes, and
/// the Last.fm spec says such short tracks must not be scrobbled at all.
pub fn scrobble_delay_secs(duration_secs: u64) -> Option<u64> {
    if duration_secs <= 30 {
        return None;
    }
    Some((duration_secs / 2).min(240))
}

#[cfg(test)]
mod tests {
    use super::scrobble_delay_secs;

    #[test]
    fn unknown_duration_skips_scrobble() {
        assert_eq!(scrobble_delay_secs(0), None);
    }

    #[test]
    fn short_tracks_skip_scrobble() {
        // Last.fm rule: the track must be longer than 30 seconds.
        assert_eq!(scrobble_delay_secs(1), None);
        assert_eq!(scrobble_delay_secs(29), None);
        assert_eq!(scrobble_delay_secs(30), None);
        assert_eq!(scrobble_delay_secs(31), Some(15));
    }

    #[test]
    fn half_duration_capped_at_240() {
        assert_eq!(scrobble_delay_secs(100), Some(50));
        assert_eq!(scrobble_delay_secs(600), Some(240));
    }
}
