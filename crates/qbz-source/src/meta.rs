//! Row-level display fields, produced by the source that owns the item.
//!
//! `PlayOrigin` / `OriginKind` are deliberately ABSENT: `playback_qt::
//! derive_context` (playback_qt.rs:135-347) stays in charge of queue-level
//! origin, and both current queue builders leave `context_kind`/`context_id`
//! `None` on purpose (local_playback.rs:96-104 spells out why). A trait method
//! every implementor would return `None` from is a defect, not an abstraction.

use crate::art::ArtRef;

/// Everything a row needs to draw itself, without expanding the item into
/// tracks.
#[derive(Clone, Debug, Default)]
pub struct ItemMeta {
    pub title: String,
    /// Artist / album artist.
    pub subtitle: String,
    pub year: Option<u32>,
    pub track_count: Option<u32>,
    pub duration_secs: Option<u64>,
    pub quality: QualityHint,
    pub art: ArtRef,
    pub badge: SourceBadge,
    /// UNTRANSLATED kind key (`"album"` | `"track"` | `"playlist"` | `"ep"` |
    /// `"single"`). i18n stays in the frontend — this crate never links
    /// `qbz-i18n`.
    pub kind_label: &'static str,
}

/// The quality facts a badge is derived from.
#[derive(Clone, Copy, Debug, Default)]
pub struct QualityHint {
    pub bit_depth: Option<u32>,
    /// kHz, the `QueueTrack.sample_rate` convention (NOT Hz).
    pub sample_rate_khz: Option<f64>,
    /// The container/codec name as the source spells it (`"FLAC"`, `"MP3"`,
    /// `"flac"`…). Compared case-insensitively.
    pub format: &'static str,
}

impl QualityHint {
    /// Build from a raw Hz sample rate (the `LocalTrack` / Plex-cache
    /// convention) and a format name.
    ///
    /// Moved from the kHz normalisation duplicated at
    /// `local_playback.rs:49-53`, `local_rows::tier_of` (local_rows.rs:184-188)
    /// and the Plex queue mapper in `sources/plex.rs`.
    pub fn from_hz(bit_depth: Option<u32>, sample_rate_hz: f64, format: &'static str) -> Self {
        let khz = if sample_rate_hz >= 1000.0 {
            sample_rate_hz / 1000.0
        } else {
            sample_rate_hz
        };
        Self {
            bit_depth,
            sample_rate_khz: (khz > 0.0).then_some(khz),
            format,
        }
    }

    /// `""` | `"mp3"` | `"cd"` | `"hires"` | `"max"` | `"dsd"`.
    ///
    /// Moved from `local_rows::tier_of` (local_rows.rs:180-196) so a local card
    /// and a Qobuz card can never disagree — which was its whole reason for
    /// existing. The derivation ORDER (MP3 first, then max / hires / cd) is
    /// preserved verbatim.
    pub fn tier(&self) -> &'static str {
        if self.format.eq_ignore_ascii_case("mp3") {
            return "mp3";
        }
        // 1-bit is DSD and wears its own mark (2026-09-02), whatever the
        // container word the server used (dsf / dff / dsd).
        if self.bit_depth == Some(1)
            || ["dsf", "dff", "dsd"]
                .iter()
                .any(|w| self.format.eq_ignore_ascii_case(w))
        {
            return "dsd";
        }
        let khz = self.sample_rate_khz.unwrap_or(0.0);
        match self.bit_depth {
            Some(b) if b >= 24 && khz > 96.0 => "max",
            Some(b) if b >= 24 => "hires",
            Some(_) => "cd",
            None if khz >= 44.1 => "cd",
            None => "",
        }
    }
}

/// The row badge.
///
/// Moved from `local_rows::badge_source` (local_rows.rs:223-229): a Qobuz
/// DOWNLOAD is stored `qobuz_download` but badges as `offline`, and exactly one
/// place should know that.
///
/// The `Qobuz` arm is new. `badge_source` is only ever called with a
/// `qbz_library` `source` column, which never contains `"qobuz"`, so its `_ =>
/// local` fallback covered it by accident; here the Qobuz source needs a badge
/// of its own and the mapping is explicit.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SourceBadge {
    #[default]
    Local,
    Offline,
    Plex,
    Jellyfin,
    Subsonic,
    Qobuz,
}

impl SourceBadge {
    /// The badge for a raw source word (`LocalTrack.source`,
    /// `QueueTrack.source`). Absent / unknown badges as `Local`, matching
    /// `badge_source`'s `_` arm.
    pub fn from_word(raw: Option<&str>) -> SourceBadge {
        match raw.map(str::trim) {
            Some("plex") => SourceBadge::Plex,
            Some("jellyfin") => SourceBadge::Jellyfin,
            Some("subsonic") | Some("navidrome") | Some("gonic") | Some("airsonic")
            | Some("astiga") => SourceBadge::Subsonic,
            // `offline` is the ENCRYPTED CMAF download only qbz-core's offline
            // tier can open. A PURCHASE (`qobuz_purchase`) is a plain DRM-free
            // file the scanner found on disk; badging it `Offline` made the
            // local source refuse it as "a CMAF bundle" and the album the
            // user had just downloaded and added answered "File not
            // available — is the drive mounted?" (smoke 2026-09-02).
            Some("qobuz_download") | Some("offline") => SourceBadge::Offline,
            Some("qobuz_purchase") => SourceBadge::Local,
            Some("qobuz") | Some("qobuz_connect_remote") => SourceBadge::Qobuz,
            _ => SourceBadge::Local,
        }
    }

    /// The QML badge string (`SourceIcon.qml:43-50`'s `kind` vocabulary).
    pub fn as_str(self) -> &'static str {
        match self {
            SourceBadge::Local => "local",
            SourceBadge::Offline => "offline",
            SourceBadge::Plex => "plex",
            SourceBadge::Jellyfin => "jellyfin",
            SourceBadge::Subsonic => "subsonic",
            SourceBadge::Qobuz => "qobuz",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_matches_local_rows_tier_of() {
        // local_rows.rs:180-196 — MP3 wins over everything.
        assert_eq!(
            QualityHint::from_hz(Some(24), 192_000.0, "MP3").tier(),
            "mp3"
        );
        assert_eq!(
            QualityHint::from_hz(Some(24), 192_000.0, "FLAC").tier(),
            "max"
        );
        assert_eq!(QualityHint::from_hz(Some(24), 96_000.0, "FLAC").tier(), "hires");
        assert_eq!(QualityHint::from_hz(Some(16), 44_100.0, "FLAC").tier(), "cd");
        assert_eq!(QualityHint::from_hz(None, 44_100.0, "FLAC").tier(), "cd");
        assert_eq!(QualityHint::from_hz(None, 0.0, "").tier(), "");
        // kHz input passes through untouched (the QueueTrack convention).
        assert_eq!(QualityHint::from_hz(Some(24), 192.0, "FLAC").tier(), "max");
        assert_eq!(QualityHint::from_hz(Some(1), 2_822_400.0, "DSF").tier(), "dsd");
        assert_eq!(QualityHint::from_hz(None, 0.0, "dff").tier(), "dsd");
    }

    #[test]
    fn badge_matches_badge_source() {
        assert_eq!(SourceBadge::from_word(Some("plex")), SourceBadge::Plex);
        assert_eq!(
            SourceBadge::from_word(Some("qobuz_download")),
            SourceBadge::Offline
        );
        // A purchase is a plain file: LOCAL, never the encrypted-bundle badge.
        assert_eq!(
            SourceBadge::from_word(Some("qobuz_purchase")),
            SourceBadge::Local
        );
        assert_eq!(SourceBadge::from_word(Some("user")), SourceBadge::Local);
        assert_eq!(SourceBadge::from_word(None), SourceBadge::Local);
        assert_eq!(SourceBadge::from_word(Some("qobuz")), SourceBadge::Qobuz);
    }
}
