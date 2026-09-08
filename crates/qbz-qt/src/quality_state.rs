//! Delivered-vs-catalog quality arithmetic — the Qt port of the #590/#638
//! logic that lives in `crates/qbz/src/playback.rs` (`stream_downgraded`,
//! `classify_limit_cause`, `delivered_tier_str`) plus the formatters from
//! `crates/qbz/src/quality.rs` (`detail`, `dsd_multiple_label`).
//!
//! Pure module: no Qt, no bridge. `now_playing.rs` owns the publishing; this
//! owns the numbers, so the arithmetic is unit-testable without a Qt event
//! loop and cannot drift into the UI layer.
//!
//! Two facts about the current track are cached in atomics here, seeded ONCE
//! per track change (never per poll tick — the Slint side is explicit that
//! `ui_prefs::load()` is a whole-file read + JSON parse):
//!   - the CATALOG max (`TRACK_MAX_*`), compared against the delivered stream;
//!   - the REQUESTED tier + its request-time cause (`REQUESTED_*`), which
//!     names WHY the delivered stream is below that max.
//!
//! The detected-device ceiling (#638 fix 3) IS wired: the request tier and its
//! cause both come from `playback_qt::local_playback_quality`, which reconciles
//! the streaming preference against `qbz_app::device_cap::cap()`. So
//! `LocalDeviceCap` is a cause this module really reports, and the badge
//! tooltip names the output device rather than the preference.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use qbz_models::{Quality, QualityLimit};

// ---------------------------------------------------------------------------
// Per-track cache (seeded on the track-change edge)
// ---------------------------------------------------------------------------

/// Catalog max of the current track. Rate in Hz (0 = unknown); bits 0 =
/// unknown, 1 = DSD (nominal 1-bit — exempt from the comparison).
static TRACK_MAX_RATE_HZ: AtomicU32 = AtomicU32::new(0);
static TRACK_MAX_BITS: AtomicU32 = AtomicU32::new(0);

/// REQUESTED tier of the current track's stream (Qobuz format id; 0 = the
/// track is not governed by the streaming-quality preference — local, Plex
/// and ephemeral sources) plus the request-time cause (`QualityLimit`).
static REQUESTED_QUALITY_ID: AtomicU32 = AtomicU32::new(0);
static REQUESTED_CAUSE: AtomicI32 = AtomicI32::new(0);

/// Normalize a catalog sample rate to Hz. Qobuz reports kHz (96.0); local /
/// Plex rows report raw Hz (96000). Same `>= 1000` guard the Slint uses so a
/// kHz value is never multiplied twice.
pub fn rate_to_hz(sample_rate: Option<f64>) -> u32 {
    sample_rate.map_or(0, |sr| {
        if sr >= 1000.0 {
            sr as u32
        } else {
            (sr * 1000.0) as u32
        }
    })
}

/// Seed the per-track cache on a track change (`playback.rs`
/// `refresh_now_playing_meta`: the TRACK_MAX stores + the governed branch).
/// `governed` = the streaming-quality preference shapes the request for this
/// track (Qobuz-sourced, not local); local / Plex / ephemeral store 0, which
/// keeps the cause line off for them.
pub fn seed_track(bit_depth: Option<u32>, sample_rate: Option<f64>, governed: bool) {
    TRACK_MAX_RATE_HZ.store(rate_to_hz(sample_rate), Ordering::Relaxed);
    TRACK_MAX_BITS.store(bit_depth.unwrap_or(0), Ordering::Relaxed);
    if governed {
        let (requested, cause) = local_playback_quality();
        REQUESTED_QUALITY_ID.store(requested.id(), Ordering::Relaxed);
        REQUESTED_CAUSE.store(cause as i32, Ordering::Relaxed);
    } else {
        REQUESTED_QUALITY_ID.store(0, Ordering::Relaxed);
        REQUESTED_CAUSE.store(QualityLimit::None as i32, Ordering::Relaxed);
    }
}

/// The request tier + the cause that shaped it, for the current settings.
///
/// It is the SAME function the play funnel resolves with
/// (`playback_qt::local_playback_quality`), not a parallel copy — a badge that
/// reports a tier the play path did not request is worse than no badge. This
/// module used to keep its own cap-free copy, which is exactly how it drifted:
/// the seed said `Preference` while the request was governed by the device.
use crate::playback_qt::local_playback_quality;

// ---------------------------------------------------------------------------
// The arithmetic (ported verbatim from playback.rs)
// ---------------------------------------------------------------------------

/// The #590 downgrade arithmetic. PRESERVED EXACTLY: the 0.9 rate guard
/// avoids flagging 44.1-vs-48 kHz family mismatches, and DSD (nominal 1-bit,
/// on either side) is exempt ENTIRELY — a bit-perfect DSD stream must never
/// show the amber arrow.
pub fn stream_downgraded(eff_rate_hz: u32, eff_bits: u32, max_rate_hz: u32, max_bits: u32) -> bool {
    let dsd = max_bits == 1 || eff_bits == 1;
    !dsd && ((eff_rate_hz > 0
        && max_rate_hz > 0
        && (eff_rate_hz as f64) < max_rate_hz as f64 * 0.9)
        || (eff_bits > 0 && max_bits > 0 && eff_bits < max_bits))
}

/// Why the delivered stream is below the catalog max (#638 fix 1). Returns a
/// `QualityLimit` discriminant: 0 none · 1 preference · 2 local device cap ·
/// 3 renderer (cast) cap · 4 no higher tier offered.
pub fn classify_limit_cause(
    downgraded: bool,
    requested_id: u32,
    request_cause: i32,
    eff_bits: u32,
) -> i32 {
    if !downgraded {
        return QualityLimit::None as i32;
    }
    let Some(requested) = Quality::from_id(requested_id) else {
        return QualityLimit::None as i32;
    };
    let promise_met = match requested {
        Quality::UltraHiRes => return QualityLimit::CatalogAvailability as i32,
        Quality::HiRes => eff_bits >= 24,
        Quality::Lossless | Quality::Mp3 => true,
    };
    if promise_met {
        request_cause
    } else {
        QualityLimit::CatalogAvailability as i32
    }
}

/// Delivered-tier string for the badge's main line while downgraded
/// ("hires" | "cd" | "mp3"; "" = not downgraded → the badge keeps the catalog
/// tier). Uses the requested tier too, so an MP3-capped stream (which decodes
/// to 16-bit PCM) is labeled MP3, not mislabeled CD.
pub fn delivered_tier_str(downgraded: bool, requested_id: u32, eff_bits: u32) -> &'static str {
    if !downgraded {
        return "";
    }
    if requested_id == Quality::Mp3.id() {
        return "mp3";
    }
    if eff_bits == 1 {
        return "dsd";
    }
    if eff_bits >= 24 {
        "hires"
    } else {
        "cd"
    }
}

// ---------------------------------------------------------------------------
// Formatters (crates/qbz/src/quality.rs)
// ---------------------------------------------------------------------------

/// The exact `"24-bit / 96 kHz"` line. Accepts Hz OR kHz (the `>= 1000` guard
/// normalizes), so a track row and the now-playing stamp can never disagree.
pub fn detail(bit_depth: Option<u32>, sample_rate: Option<f64>) -> String {
    let hi_res = matches!(bit_depth, Some(depth) if depth >= 24);
    let depth = bit_depth.unwrap_or(if hi_res { 24 } else { 16 });
    let rate = sample_rate.unwrap_or(if hi_res { 96.0 } else { 44.1 });
    let rate = if rate >= 1000.0 { rate / 1000.0 } else { rate };
    format!("{depth}-bit / {} kHz", crate::home_qt::format_rate(rate))
}

/// "DSD64" / "DSD128" / … from the DSD bit rate. Accepts Hz (2 822 400) or
/// kHz (2 822.4).
pub fn dsd_multiple_label(sample_rate: Option<f64>) -> String {
    let hz = match sample_rate {
        Some(r) if r >= 1_000_000.0 => r,
        Some(r) if r >= 1_000.0 => r * 1000.0,
        _ => return "DSD".to_string(),
    };
    format!(
        "DSD{}",
        ((hz / 2_822_400.0).round() as u32).saturating_mul(64)
    )
}

// ---------------------------------------------------------------------------
// The per-tick evaluation
// ---------------------------------------------------------------------------

/// What the delivered stream is, measured against the cached catalog max.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delivered {
    pub downgraded: bool,
    /// The DELIVERED detail line ("16-bit / 44.1 kHz" / "DSD64"); empty when
    /// the engine has not reported stream params yet.
    pub true_detail: String,
    /// `QualityLimit` discriminant — see `classify_limit_cause`.
    pub limit_cause: i32,
    /// Delivered tier while downgraded ("hires" | "cd" | "mp3"), else "".
    pub effective_tier: String,
}

/// Evaluate the engine's delivered stream params against the cached catalog
/// max + requested tier. Cheap (four relaxed atomic loads) — safe per tick.
pub fn evaluate(eff_rate_hz: u32, eff_bits: u32) -> Delivered {
    let max_rate_hz = TRACK_MAX_RATE_HZ.load(Ordering::Relaxed);
    let max_bits = TRACK_MAX_BITS.load(Ordering::Relaxed);
    let downgraded = stream_downgraded(eff_rate_hz, eff_bits, max_rate_hz, max_bits);
    let requested_id = REQUESTED_QUALITY_ID.load(Ordering::Relaxed);
    let request_cause = REQUESTED_CAUSE.load(Ordering::Relaxed);
    // Native DSD (1-bit) goes through the DSD label — the generic detail
    // would read "1-bit / 2822.4 kHz".
    let true_detail = if eff_bits == 1 {
        dsd_multiple_label((eff_rate_hz > 0).then_some(eff_rate_hz as f64))
    } else if eff_rate_hz > 0 || eff_bits > 0 {
        detail(
            (eff_bits > 0).then_some(eff_bits),
            (eff_rate_hz > 0).then_some(eff_rate_hz as f64),
        )
    } else {
        String::new()
    };
    Delivered {
        downgraded,
        true_detail,
        limit_cause: classify_limit_cause(downgraded, requested_id, request_cause, eff_bits),
        effective_tier: delivered_tier_str(downgraded, requested_id, eff_bits).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_guard_ignores_44_vs_48_family() {
        // 44.1 delivered against a 48 kHz max is NOT a downgrade (0.9 guard).
        assert!(!stream_downgraded(44_100, 16, 48_000, 16));
        // 44.1 against 96 kHz is.
        assert!(stream_downgraded(44_100, 16, 96_000, 24));
    }

    #[test]
    fn dsd_is_exempt_on_either_side() {
        assert!(!stream_downgraded(2_822_400, 1, 96_000, 24));
        assert!(!stream_downgraded(96_000, 24, 2_822_400, 1));
    }

    #[test]
    fn unknown_params_never_flag() {
        assert!(!stream_downgraded(0, 0, 96_000, 24));
        assert!(!stream_downgraded(44_100, 16, 0, 0));
    }

    #[test]
    fn limit_cause_rules() {
        // Not downgraded → none, whatever the request was.
        assert_eq!(
            classify_limit_cause(false, Quality::HiRes.id(), 1, 16),
            QualityLimit::None as i32
        );
        // Ungoverned source (id 0) → none.
        assert_eq!(
            classify_limit_cause(true, 0, 1, 16),
            QualityLimit::None as i32
        );
        // Requested Hi-Res+ and still short → Qobuz had no more.
        assert_eq!(
            classify_limit_cause(true, Quality::UltraHiRes.id(), 0, 24),
            QualityLimit::CatalogAvailability as i32
        );
        // Requested Hi-Res, delivered 24-bit → the promise held, so the
        // request-time cause did this.
        assert_eq!(
            classify_limit_cause(
                true,
                Quality::HiRes.id(),
                QualityLimit::Preference as i32,
                24
            ),
            QualityLimit::Preference as i32
        );
        // Requested Hi-Res, delivered 16-bit → the promise broke: catalog.
        assert_eq!(
            classify_limit_cause(
                true,
                Quality::HiRes.id(),
                QualityLimit::Preference as i32,
                16
            ),
            QualityLimit::CatalogAvailability as i32
        );
    }

    #[test]
    fn delivered_tier_labels_mp3_by_request() {
        assert_eq!(delivered_tier_str(false, Quality::Mp3.id(), 16), "");
        assert_eq!(delivered_tier_str(true, Quality::Mp3.id(), 16), "mp3");
        assert_eq!(delivered_tier_str(true, Quality::HiRes.id(), 16), "cd");
        assert_eq!(delivered_tier_str(true, Quality::HiRes.id(), 24), "hires");
        assert_eq!(delivered_tier_str(true, Quality::HiRes.id(), 1), "dsd");
    }

    #[test]
    fn detail_normalizes_hz_and_khz_identically() {
        assert_eq!(detail(Some(24), Some(96.0)), "24-bit / 96 kHz");
        assert_eq!(detail(Some(24), Some(96_000.0)), "24-bit / 96 kHz");
        assert_eq!(detail(Some(16), Some(44.1)), "16-bit / 44.1 kHz");
    }

    #[test]
    fn dsd_labels() {
        assert_eq!(dsd_multiple_label(Some(2_822_400.0)), "DSD64");
        assert_eq!(dsd_multiple_label(Some(5_644_800.0)), "DSD128");
        assert_eq!(dsd_multiple_label(None), "DSD");
    }

    #[test]
    fn rate_to_hz_guard() {
        assert_eq!(rate_to_hz(Some(96.0)), 96_000);
        assert_eq!(rate_to_hz(Some(96_000.0)), 96_000);
        assert_eq!(rate_to_hz(None), 0);
    }
}
