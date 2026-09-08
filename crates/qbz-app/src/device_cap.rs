//! Local output-device quality cap (#638 fix 3).
//!
//! Caches the local DAC's detected rate ceiling, mapped to a Qobuz tier, so
//! the request-time resolution (the frontend's `local_playback_quality`) can
//! clamp the streaming-quality preference without probing per call. Detection
//! is the proven read-only `qbz_audio::query_dac_capabilities` (reads
//! `/proc/asound` and shells out to `pw-dump`; never opens a stream), so a
//! refresh runs inside `spawn_blocking` on EXPLICIT triggers only — startup,
//! the Settings toggle, an output-device/backend change, reset-to-defaults,
//! the device refresh/release button — never on the playback hot path or the
//! poll tick. A stale cap after a hot-unplug (until the next device change),
//! and an uncapped first track when a session-restore play beats the startup
//! refresh, are the accepted trades — same class as the HiFi wizard's
//! behavior; both self-heal.
//!
//! PRECEDENCE (owner decision, #638): the cap of the device ACTUALLY PLAYING
//! governs. This cache is for LOCAL playback only — the cast path must never
//! read it (the local DAC is not in a cast's signal path).
//!
//! Frontend-agnostic by design: it lives here rather than in a binary crate
//! because that is exactly why the row never reached the Qt frontend.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use qbz_audio::backend::AudioBackendType;
use qbz_models::Quality;

/// The cached cap. `tier` is the coarse Qobuz-tier mapping of the detected
/// ceiling (`max_rate_hz`); `detected` false = the probe fell back to the
/// common rate set, so the Settings caveat must disclose that the cap may
/// not match the hardware (owner Decision B: it still applies).
#[derive(Clone)]
pub struct CapState {
    pub tier: Quality,
    pub detected: bool,
    pub max_rate_hz: u32,
    pub description: String,
}

/// None = the cap is disabled (toggle off) or not refreshed yet.
static CAP: RwLock<Option<CapState>> = RwLock::new(None);

/// Refresh generation, and it is load-bearing rather than defensive.
///
/// The triggers are independently spawned tasks and the probe is a
/// `spawn_blocking` that shells out to `pw-dump` — so two refreshes CAN be in
/// flight, and the slow one can land last. Without this counter the sequence
/// "toggle ON (slow probe) → user toggles OFF" ends with the OFF committing
/// first and the stale probe RESURRECTING a cap the settings say is disabled:
/// silently capped playback with the toggle visibly off, and nothing re-runs
/// until the next device change. Every write below commits only if it is still
/// the newest refresh.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// False until the first refresh of the process commits. It exists to stop a
/// catastrophe, not to be tidy: the cap cache starts empty, so at boot
/// `None -> Some(tier)` looks exactly like "the user just changed the cap",
/// and the invalidation that answer triggers is `clear_audio_cache()`, which
/// unlinks EVERY `<id>.audio` file on disk (hundreds of MB to GBs). Without
/// this flag, every single launch with the toggle on would silently wipe the
/// whole playback cache and re-download everything.
static SEEDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What a refresh did — the audio-cache invalidation decision, made where the
/// state lives instead of by a before/after comparison at the call site (which
/// cannot tell a boot from a change, and misattributes a concurrent refresh's
/// commit to itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapChange {
    /// Same tier as before, or this refresh was superseded by a newer one.
    /// Nothing to invalidate.
    Unchanged,
    /// The effective request tier MOVED. The audio cache is quality-blind, so
    /// bytes fetched under the old tier must not keep serving.
    Changed,
    /// The first refresh of the process learned the cap. It is not a change:
    /// nothing was fetched under a different tier *by this process*, and the
    /// previous session already invalidated on its own transitions. Known
    /// residual: bytes cached under a session whose DAC had a HIGHER ceiling
    /// survive a reboot onto a smaller DAC until any in-session trigger fires.
    /// That narrow hole is worth far less than deleting the disk cache on
    /// every launch.
    Seeded,
}

/// The classification, lifted out of the lock so it is testable.
fn classify(first_refresh: bool, before: Option<Quality>, after: Option<Quality>) -> CapChange {
    if first_refresh {
        CapChange::Seeded
    } else if before == after {
        CapChange::Unchanged
    } else {
        CapChange::Changed
    }
}

/// Commit a new cap under the write lock, refusing a superseded write.
///
/// The generation is re-checked WHILE HOLDING the lock. Checking it before
/// doing the work left a TOCTOU window wide enough to drive the exact bug the
/// counter exists for through: a slow probe passes the check, the user toggles
/// the cap off and commits `None`, then the stale probe writes its `Some`
/// back — a live cap with the toggle visibly off.
fn commit(next: Option<CapState>, generation: u64) -> CapChange {
    let mut guard = CAP.write().unwrap_or_else(|e| e.into_inner());
    if GENERATION.load(Ordering::SeqCst) != generation {
        log::info!("[qbz] device cap: refresh superseded by a newer one, discarding");
        return CapChange::Unchanged;
    }
    let before = guard.as_ref().map(|c| c.tier);
    let after = next.as_ref().map(|c| c.tier);
    *guard = next;
    classify(!SEEDED.swap(true, Ordering::SeqCst), before, after)
}

/// Map a detected max rate to the tier we may REQUEST. Coarse by design:
/// Qobuz sells four discrete tiers, and the mapping must never name a tier
/// the device cannot play, because the tier is a CEILING the service is free
/// to deliver at:
///
/// - Hi-Res+ can deliver up to 192 kHz, so it needs a 192 kHz device. A
///   176.4 kHz ceiling resolves to Hi-Res, not Hi-Res+.
/// - Hi-Res can deliver up to 96 kHz, so it needs a 96 kHz device. An
///   88.2 kHz ceiling resolves to CD, not Hi-Res.
/// - There is no 48 kHz tier, so anything below 96 kHz steps down to CD
///   16/44.1 — bit depth included; the Settings summary says it plainly
///   instead of letting the user discover it.
///
/// (Owner decision D4, 2026-08-17: these boundaries were the loose
/// `> 96_000` / `>= 88_200` pair, which handed an 88.2 kHz DAC a tier
/// requestable at 96 and a 176.4 kHz DAC one requestable at 192.)
fn tier_for_max_rate_hz(max_hz: u32) -> Quality {
    if max_hz >= 192_000 {
        Quality::UltraHiRes
    } else if max_hz >= 96_000 {
        Quality::HiRes
    } else {
        Quality::Lossless
    }
}

/// Cheap read for the request-time resolution: `(tier, detected)`.
/// None = no cap configured. Two uncontended lock reads — safe to call on
/// every play, which is what lets the cap live inside the play funnel.
pub fn cap() -> Option<(Quality, bool)> {
    CAP.read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|c| (c.tier, c.detected))
}

/// The Settings "Detected device limit" value line: `(summary, detected)`,
/// e.g. `("192 kHz · Hi-Res+", true)`. Untranslated data composition — the
/// same convention as the quality badge's "24-bit / 96 kHz" (tier names are
/// product names). `("", true)` when no cap is active: the row hides on the
/// empty summary, and `true` keeps the fallback caveat from flashing before
/// the first refresh lands.
pub fn summary() -> (String, bool) {
    match CAP.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some(c) => (
            format!(
                "{} · {}",
                rate_khz_label(c.max_rate_hz),
                tier_display(c.tier)
            ),
            c.detected,
        ),
        None => (String::new(), true),
    }
}

/// Product-name tier label for the summary line. The CD entry spells out the
/// bit-depth cost (no 48 kHz tier exists, so the step below Hi-Res loses
/// depth too — say it, don't let the user discover it).
fn tier_display(tier: Quality) -> &'static str {
    match tier {
        Quality::UltraHiRes => "Hi-Res+",
        Quality::HiRes => "Hi-Res",
        Quality::Lossless => "CD 16-bit / 44.1 kHz",
        // Unreachable from tier_for_max_rate_hz; total match for safety.
        Quality::Mp3 => "MP3 320",
    }
}

/// "192 kHz" / "44.1 kHz" from Hz — integer when whole, one decimal
/// otherwise (matches the quality badge's rate formatting).
fn rate_khz_label(hz: u32) -> String {
    let khz = hz as f64 / 1000.0;
    if khz.fract().abs() < f64::EPSILON {
        format!("{} kHz", khz as i64)
    } else {
        format!("{khz} kHz")
    }
}

/// Re-run detection and refresh the cache. `limit_enabled` off clears it
/// immediately (no probe). The probe runs in `spawn_blocking` (pw-dump
/// subprocess + /proc reads — never on the UI thread). Await-able so the
/// Settings controller can re-push the summary row right after.
///
/// `backend` is the audio backend ACTUALLY in use (owner decision D7), and its
/// reach is narrower than it looks: it selects the backend that ENUMERATES the
/// default sink when `output_device` is None ("System default"). The probe
/// itself is PipeWire-backed regardless (`query_dac_capabilities` resolves the
/// description through PipeWire, then falls through to `/proc/asound`). `None`
/// means "no configured backend" and enumerates through PipeWire, the
/// historical behavior.
///
/// Returns what changed, so the caller can decide about the audio cache
/// without re-deriving it from a racy before/after read.
pub async fn refresh(
    limit_enabled: bool,
    output_device: Option<String>,
    backend: Option<AudioBackendType>,
) -> CapChange {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if !limit_enabled {
        log::info!("[qbz] device cap: disabled");
        return commit(None, generation);
    }
    let probed = tokio::task::spawn_blocking(move || {
        // The configured device id, or the system-default sink when the
        // selection is "System default" (None). An unresolvable default
        // probes with an empty node name, which lands on the fallback set →
        // detected=false → Hi-Res+ no-op cap with the caveat disclosed.
        let node = output_device.unwrap_or_else(|| default_output_node(backend));
        qbz_audio::query_dac_capabilities(&node)
    })
    .await;
    let caps = match probed {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[qbz] device cap: probe task failed: {e}");
            return CapChange::Unchanged;
        }
    };
    let max_rate_hz = caps.sample_rates.iter().copied().max().unwrap_or(0);
    // assemble() always yields a non-empty rate list (fallback set), but
    // never store a 0 Hz cap if that invariant ever breaks.
    if max_rate_hz == 0 {
        return commit(None, generation);
    }
    let state = CapState {
        tier: tier_for_max_rate_hz(max_rate_hz),
        detected: caps.detected,
        max_rate_hz,
        description: caps.description.unwrap_or_else(|| caps.node_name.clone()),
    };
    log::info!(
        "[qbz] device cap: {} -> max {} Hz -> {:?} ({})",
        state.description,
        state.max_rate_hz,
        state.tier,
        if state.detected {
            "detected"
        } else {
            "fallback set"
        },
    );
    commit(Some(state), generation)
}

/// The default sink's node id for the "System default" device selection,
/// resolved through the backend that is actually playing (D7). Empty when
/// nothing enumerates — the probe then reports the fallback set honestly.
fn default_output_node(backend: Option<AudioBackendType>) -> String {
    qbz_audio::backend::BackendManager::create_backend(
        backend.unwrap_or(AudioBackendType::PipeWire),
    )
    .ok()
    .and_then(|b| b.enumerate_devices().ok())
    .and_then(|devs| devs.into_iter().find(|d| d.is_default))
    .map(|d| d.id)
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_mapping_never_names_a_tier_the_device_cannot_play() {
        // >= 192 kHz → Hi-Res+ (no effective cap).
        assert_eq!(tier_for_max_rate_hz(384_000), Quality::UltraHiRes);
        assert_eq!(tier_for_max_rate_hz(192_000), Quality::UltraHiRes);
        // 176.4 kHz → Hi-Res, NOT Hi-Res+ (D4): Hi-Res+ may be delivered at
        // 192 kHz, which this device cannot play.
        assert_eq!(tier_for_max_rate_hz(176_400), Quality::HiRes);
        // 96 kHz → Hi-Res (its exact ceiling).
        assert_eq!(tier_for_max_rate_hz(96_000), Quality::HiRes);
        // 88.2 kHz → CD, NOT Hi-Res (D4): Hi-Res may be delivered at 96 kHz.
        assert_eq!(tier_for_max_rate_hz(88_200), Quality::Lossless);
        // <= 48 kHz → CD 16/44.1 (bit depth lost too — no 48 kHz tier).
        assert_eq!(tier_for_max_rate_hz(48_000), Quality::Lossless);
        assert_eq!(tier_for_max_rate_hz(44_100), Quality::Lossless);
    }

    #[test]
    fn rate_label_formats_whole_and_fractional_khz() {
        assert_eq!(rate_khz_label(192_000), "192 kHz");
        assert_eq!(rate_khz_label(44_100), "44.1 kHz");
        assert_eq!(rate_khz_label(176_400), "176.4 kHz");
    }

    /// THE ONE THAT COST A DISK CACHE. `clear_audio_cache` unlinks every
    /// `<id>.audio` file on disk, and the cap cache starts empty — so the
    /// first refresh of a process ALWAYS looks like `None -> Some(tier)`.
    /// Classifying that as a change wiped the user's whole playback cache on
    /// every launch with the toggle on, and logged it as legitimate.
    #[test]
    fn the_first_refresh_of_a_process_is_never_a_cache_invalidating_change() {
        assert_eq!(
            classify(true, None, Some(Quality::Lossless)),
            CapChange::Seeded
        );
        // Even a first refresh that lands on a DIFFERENT tier than some past
        // session's is Seeded: this process fetched nothing under the old one.
        assert_eq!(
            classify(true, Some(Quality::UltraHiRes), Some(Quality::Lossless)),
            CapChange::Seeded
        );
    }

    /// ...but once seeded, `None -> Some` is the toggle being switched ON,
    /// which is precisely when uncapped bytes must stop serving. Collapsing
    /// this into the boot case would make the feature inert on every track
    /// played before the toggle.
    #[test]
    fn turning_the_cap_on_after_boot_does_invalidate() {
        assert_eq!(
            classify(false, None, Some(Quality::Lossless)),
            CapChange::Changed
        );
        // And off again — bytes fetched under the cap are below what the
        // preference now requests, so they must not keep serving either.
        assert_eq!(
            classify(false, Some(Quality::Lossless), None),
            CapChange::Changed
        );
    }

    #[test]
    fn a_refresh_that_lands_on_the_same_tier_invalidates_nothing() {
        assert_eq!(
            classify(false, Some(Quality::HiRes), Some(Quality::HiRes)),
            CapChange::Unchanged
        );
        assert_eq!(classify(false, None, None), CapChange::Unchanged);
    }

    /// The fallback rate set tops out at 192 kHz, so an UNDETECTED device is
    /// capped in name only (contract D6: accept and document). This test
    /// exists so the day someone tightens the fallback set, the consequence
    /// for the "The cap still applies" copy is visible instead of implicit.
    #[test]
    fn an_undetected_device_caps_at_hi_res_plus_which_is_no_clamp() {
        assert_eq!(tier_for_max_rate_hz(192_000), Quality::UltraHiRes);
    }
}
