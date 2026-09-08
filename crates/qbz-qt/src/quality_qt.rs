//! The ONE quality-badge classifier for this frontend — `(tier, detail)` from
//! a container/codec name plus a (possibly unknown) bit depth and rate.
//!
//! Port of `crates/qbz/src/quality.rs` (`is_lossless_format`, `is_dsd_format`,
//! `dsd_multiple_label`, `tier`, `badge`). It lived inline in
//! `myqbz_builder_qt.rs`, whose own note called hoisting it here a named
//! follow-up that "must not be done in a domain commit" — this is that commit.
//!
//! Why one classifier matters: the earlier per-surface duplication is what
//! produced "96 kHz" on one surface and "96000 kHz" on another, and — until
//! this file existed — a DSD track badged as CD on every Local Library
//! surface while MyQBZ badged the same file `DSD64`.
//!
//! Tiers are exactly the five names the QML marks draw: `hires | cd | mp3 |
//! lossless | ""` (`controls/QualityBadgeFull.qml:28`). `local_rows::tier_of`
//! keeps its own extra `"max"` tier on purpose — `LocalLibraryView.qml:435`
//! filters hi-res on it and `QualityBadge.qml:66` folds it back to `hires`.

/// Lossless container/codec formats — the file IS lossless even when its exact
/// bit depth / sample rate isn't known yet (e.g. an un-hydrated Plex track).
pub(crate) fn is_lossless_format(format: &str) -> bool {
    matches!(
        format.trim().to_ascii_lowercase().as_str(),
        "flac" | "wav" | "wave" | "aiff" | "aif" | "alac" | "ape" | "dsd" | "dsf" | "dff"
    )
}

/// DSD container/format names. `AudioFormat::Dsd` prints `"DSD"`
/// (`qbz-library/src/models.rs:33`); the extensions cover the string-typed
/// callers.
pub(crate) fn is_dsd_format(format: &str) -> bool {
    matches!(
        format.trim().to_ascii_lowercase().as_str(),
        "dsd" | "dsf" | "dff"
    )
}

/// "DSD64" / "DSD128" / … from the DSD bit rate. Accepts Hz (2 822 400) or
/// kHz (2 822.4) — some surfaces normalize the stored rate to kHz upstream.
pub(crate) fn dsd_multiple_label(sample_rate: Option<f64>) -> String {
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

/// `(tier, detail)` for `controls/QualityBadgeFull.qml`.
///
/// DSD is answered FIRST and never reaches the depth match: its nominal depth
/// is 1 bit and its `sample_rate` is the DSD bit rate, so the generic detail
/// would read "1-bit / 2822.4 kHz" and the generic tier would read `"cd"`.
pub(crate) fn badge(
    format: &str,
    bit_depth: Option<u32>,
    sample_rate: Option<f64>,
) -> (String, String) {
    if is_dsd_format(format) {
        return ("dsd".to_string(), dsd_multiple_label(sample_rate));
    }
    let tier = if format.trim().eq_ignore_ascii_case("mp3") {
        "mp3"
    } else {
        match bit_depth {
            Some(b) if b >= 24 => "hires",
            Some(_) => "cd",
            None if is_lossless_format(format) => "lossless",
            None => "",
        }
    };
    match tier {
        "" | "mp3" => (tier.to_string(), String::new()),
        "lossless" => (tier.to_string(), format.trim().to_uppercase()),
        _ => (
            tier.to_string(),
            crate::home_qt::quality_detail_from_parts(bit_depth, sample_rate),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsd_is_hires_and_labelled_by_its_multiple() {
        // The bug this module exists for: `AudioFormat::Dsd` reaches the badge
        // as depth 1 / rate 2 822 400, which the generic arms read as CD.
        assert_eq!(
            badge("DSD", Some(1), Some(2_822_400.0)),
            ("dsd".to_string(), "DSD64".to_string())
        );
        assert_eq!(
            badge("dsf", Some(1), Some(5_644_800.0)),
            ("dsd".to_string(), "DSD128".to_string())
        );
    }

    #[test]
    fn a_rate_already_in_khz_is_not_re_divided() {
        assert_eq!(dsd_multiple_label(Some(2_822.4)), "DSD64");
        assert_eq!(dsd_multiple_label(None), "DSD");
    }

    #[test]
    fn the_non_dsd_arms_are_unchanged() {
        assert_eq!(badge("MP3", Some(16), Some(44_100.0)).0, "mp3");
        assert_eq!(
            badge("FLAC", Some(24), Some(96_000.0)),
            ("hires".to_string(), "24-bit / 96 kHz".to_string())
        );
        assert_eq!(
            badge("FLAC", Some(16), Some(44_100.0)),
            ("cd".to_string(), "16-bit / 44.1 kHz".to_string())
        );
        // Un-hydrated: depth unknown but the container is lossless.
        assert_eq!(
            badge("FLAC", None, None),
            ("lossless".to_string(), "FLAC".to_string())
        );
        assert_eq!(badge("Unknown", None, None), (String::new(), String::new()));
    }
}
