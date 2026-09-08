//! Process-wide memory tuning for the player.
//!
//! `qbz-core`'s `system_capabilities` detects the host's memory profile
//! (Normal vs LowMemory), but `qbz-core` depends on `qbz-player` — the
//! player cannot read the profile back without a dependency cycle. So the
//! frontend binaries (`qbz`, `qbzd`) read the profile once at startup and
//! push the derived values down here, exactly like the pre-existing
//! [`super::streaming_source::set_max_initial_buffer_bytes`] cap.
//!
//! Every knob defaults to today's hardcoded Normal behavior, so a consumer
//! that never calls [`apply_memory_tuning`] (tests, auxiliary tools) is
//! byte-identical to before.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Whether the host profile is LowMemory (< 2 GB RAM, Raspberry Pi-class).
/// Default false = Normal — behavioral changes gated on this flag must keep
/// Normal hosts unchanged.
static LOW_MEMORY_CLASS: AtomicBool = AtomicBool::new(false);

/// L1 (in-memory) audio-cache budget in bytes. Defaults to the historical
/// hardcoded 400 MB; the LowMemory profile lowers it to 50 MB so the cache
/// cannot eat 40 % of a 1 GB host (issue #660).
static AUDIO_CACHE_L1_MAX_BYTES: AtomicUsize = AtomicUsize::new(400 * 1024 * 1024);

/// Push the host memory profile's derived values into the player. Called
/// once at process start by the frontend binaries; not designed for
/// mid-session mutation (the L1 cache reads its budget at construction).
pub fn apply_memory_tuning(
    low_memory: bool,
    l1_max_bytes: usize,
    max_initial_buffer_bytes: usize,
) {
    LOW_MEMORY_CLASS.store(low_memory, Ordering::Relaxed);
    AUDIO_CACHE_L1_MAX_BYTES.store(l1_max_bytes, Ordering::Relaxed);
    super::streaming_source::set_max_initial_buffer_bytes(max_initial_buffer_bytes);
}

/// True when the host was classified LowMemory at startup.
pub fn is_low_memory_class() -> bool {
    LOW_MEMORY_CLASS.load(Ordering::Relaxed)
}

/// The configured L1 audio-cache budget in bytes.
pub fn audio_cache_l1_max_bytes() -> usize {
    AUDIO_CACHE_L1_MAX_BYTES.load(Ordering::Relaxed)
}

/// Pure predicate: a completed track of `len` bytes is "oversized" for an
/// L1 budget of `l1_max_bytes` when it exceeds a quarter of that budget.
///
/// Rationale for the /4: the streaming buffer already holds one full copy
/// of the track in RAM for playback, so promoting an oversized track into
/// L1 (plus the promotion clone itself) transiently triples its footprint.
/// Anything beyond a quarter of the L1 budget would also evict every other
/// cached track on insert, so the cache stop pays for itself only for
/// smaller tracks. With the LowMemory 50 MB budget this flags tracks over
/// 12.5 MB; the predicate is only consulted on LowMemory hosts.
pub fn oversized_for_l1(len: usize, l1_max_bytes: usize) -> bool {
    len > l1_max_bytes / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOW_L1: usize = 50 * 1024 * 1024; // LowMemory profile budget

    #[test]
    fn oversized_boundary_is_quarter_of_budget() {
        let threshold = LOW_L1 / 4;
        assert!(!oversized_for_l1(threshold, LOW_L1)); // exactly at: still cached
        assert!(oversized_for_l1(threshold + 1, LOW_L1));
        assert!(!oversized_for_l1(0, LOW_L1));
    }

    #[test]
    fn normal_budget_flags_only_very_large_tracks() {
        // The predicate is gated on the LowMemory class at the call sites,
        // but with the Normal 400 MB budget the threshold would be 100 MB.
        let normal = 400 * 1024 * 1024;
        assert!(!oversized_for_l1(60 * 1024 * 1024, normal)); // typical HiRes
        assert!(oversized_for_l1(200 * 1024 * 1024, normal)); // issue #660 track
    }

    #[test]
    fn defaults_match_historical_hardcoded_behavior() {
        // A process that never applied tuning: Normal class, 400 MB L1 —
        // identical to the pre-wiring hardcoded values.
        assert!(!is_low_memory_class());
        assert_eq!(audio_cache_l1_max_bytes(), 400 * 1024 * 1024);
    }
}
