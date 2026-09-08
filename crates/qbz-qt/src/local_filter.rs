//! Allowlisted Local Library quality/format/source filter descriptor.
//!
//! QML owns the presentation JSON; every backend path consumes this parsed
//! value so native and compatibility readers cannot disagree about a chip.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaFilter {
    pub formats: Vec<String>,
    pub other_formats: bool,
    pub qualities: Vec<String>,
    pub sources: Vec<String>,
}

impl MediaFilter {
    pub fn from_json(json: &str) -> Self {
        let value: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
        let on = |key: &str| value.get(key).and_then(|value| value.as_bool()) == Some(true);
        Self {
            formats: ["flac", "alac", "ape", "wav", "mp3", "aac"]
                .into_iter()
                .filter(|key| on(key))
                .map(str::to_string)
                .collect(),
            other_formats: on("other"),
            qualities: ["dsd", "hires", "cd", "lossy"]
                .into_iter()
                .filter(|key| on(key))
                .map(str::to_string)
                .collect(),
            sources: ["local", "offline", "plex", "jellyfin", "subsonic"]
                .into_iter()
                .filter(|key| on(key))
                .map(str::to_string)
                .collect(),
        }
    }

    pub fn source_enabled(&self, source: &str) -> bool {
        self.sources.is_empty() || self.sources.iter().any(|value| value == source)
    }

    /// Apply the same source/format/quality funnel to one physical copy.
    /// Logical album cards may match through any represented source; version
    /// pickers and their actions must not reintroduce copies the active funnel
    /// excluded.
    pub fn track_enabled(&self, track: &qbz_library::LocalTrack) -> bool {
        let source = match track.source.as_deref().unwrap_or("local") {
            // Scanned files are stored as `user` in library.db. The UI and
            // every album projection expose that source as `local`; letting
            // the raw storage word reach the allowlist made physical local
            // copies disappear as soon as any source chip was active.
            "" | "user" | "local" => "local",
            "qobuz_purchase" | "qobuz_download" => "offline",
            "navidrome" | "gonic" | "airsonic" | "astiga" => "subsonic",
            source => source,
        };
        if !self.source_enabled(source) {
            return false;
        }

        let format = track.format.to_string().to_ascii_lowercase();
        if !self.formats.is_empty() || self.other_formats {
            let known = ["flac", "alac", "ape", "wav", "mp3", "aac"];
            let format_matches = self.formats.iter().any(|value| value == &format)
                || (self.other_formats && !known.contains(&format.as_str()));
            if !format_matches {
                return false;
            }
        }

        if !self.qualities.is_empty() {
            let tier =
                crate::local_rows::tier_of(&track.format, track.bit_depth, track.sample_rate);
            // DSD is its own chip (the badge already wears its own tier);
            // Hi-Res is 24-bit PCM only.
            let quality = match tier {
                "dsd" => "dsd",
                "hires" | "max" => "hires",
                "cd" => "cd",
                _ => "lossy",
            };
            if !self.qualities.iter().any(|value| value == quality) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::MediaFilter;
    use qbz_library::{AudioFormat, LocalTrack};

    #[test]
    fn parser_ignores_unknown_keys_and_normalizes_source_gate() {
        let filter = MediaFilter::from_json(
            r#"{"flac":true,"other":true,"hires":true,"plex":true,"bogus":true}"#,
        );
        assert_eq!(filter.formats, ["flac"]);
        assert!(filter.other_formats);
        assert_eq!(filter.qualities, ["hires"]);
        assert!(filter.source_enabled("plex"));
        assert!(!filter.source_enabled("local"));
    }

    #[test]
    fn physical_versions_use_folded_source_and_media_facets() {
        let filter = MediaFilter::from_json(r#"{"subsonic":true,"flac":true,"hires":true}"#);
        let track = LocalTrack {
            source: Some("navidrome".into()),
            format: AudioFormat::Flac,
            bit_depth: Some(24),
            sample_rate: 96_000.0,
            ..Default::default()
        };
        assert!(filter.track_enabled(&track));

        let mut local = track.clone();
        // `user` is the authoritative library.db spelling for a scanned
        // physical file; presentation folds it to the Local source chip.
        local.source = Some("user".into());
        assert!(!filter.track_enabled(&local));

        let mut mp3 = track;
        mp3.format = AudioFormat::Mp3;
        mp3.bit_depth = None;
        mp3.sample_rate = 44_100.0;
        assert!(!filter.track_enabled(&mp3));
    }

    #[test]
    fn source_filter_removes_other_physical_versions_before_picker_grouping() {
        let filter = MediaFilter::from_json(r#"{"jellyfin":true,"subsonic":true}"#);
        let copy = |id: i64, key: &str, source: &str| LocalTrack {
            id,
            file_path: format!("{key}/01.flac"),
            title: "Track 1".into(),
            artist: "Seiji Yokoyama".into(),
            album: "Saint Seiya Eternal CD-Box".into(),
            album_group_key: key.into(),
            album_group_title: "Saint Seiya Eternal CD-Box".into(),
            track_number: Some(1),
            disc_number: Some(1),
            format: AudioFormat::Flac,
            source: Some(source.into()),
            ..Default::default()
        };
        let tracks = vec![
            copy(1, "local-copy", "user"),
            copy(2, "plex-copy", "plex"),
            copy(3, "jellyfin-copy", "jellyfin"),
            copy(4, "navidrome-copy", "navidrome"),
        ];
        let filtered = tracks
            .into_iter()
            .filter(|track| filter.track_enabled(track))
            .collect();
        let versions = crate::local_album_actions::split_versions(filtered);
        assert_eq!(versions.len(), 2);
        let sources: Vec<&str> = versions
            .iter()
            .map(|(_, rows)| rows[0].source.as_deref().unwrap())
            .collect();
        assert!(sources.contains(&"jellyfin"));
        assert!(sources.contains(&"navidrome"));
        assert!(!sources.contains(&"local"));
        assert!(!sources.contains(&"plex"));
    }

    #[test]
    fn dsd_chip_is_its_own_tier_and_hires_stays_pcm() {
        let dsd = LocalTrack {
            source: Some("user".into()),
            format: AudioFormat::Dsd,
            bit_depth: Some(1),
            sample_rate: 2_822_400.0,
            ..Default::default()
        };
        let hires = LocalTrack {
            source: Some("user".into()),
            format: AudioFormat::Flac,
            bit_depth: Some(24),
            sample_rate: 96_000.0,
            ..Default::default()
        };
        let only_dsd = MediaFilter::from_json(r#"{"dsd":true}"#);
        assert_eq!(only_dsd.qualities, ["dsd"]);
        assert!(only_dsd.track_enabled(&dsd));
        assert!(!only_dsd.track_enabled(&hires));

        let only_hires = MediaFilter::from_json(r#"{"hires":true}"#);
        assert!(only_hires.track_enabled(&hires));
        assert!(!only_hires.track_enabled(&dsd), "DSD no longer hides under Hi-Res");
    }

    #[test]
    fn local_source_chip_accepts_scanner_storage_word() {
        let filter = MediaFilter::from_json(r#"{"local":true}"#);
        let scanned = LocalTrack {
            source: Some("user".into()),
            format: AudioFormat::Flac,
            ..Default::default()
        };
        assert!(filter.track_enabled(&scanned));

        let all_sources = MediaFilter::from_json(
            r#"{"local":true,"offline":true,"plex":true,"jellyfin":true,"subsonic":true}"#,
        );
        assert!(all_sources.track_enabled(&scanned));
    }
}
