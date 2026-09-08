//! Source-aware playback types.
//!
//! Playable tracks reach the queue from multiple origins: Qobuz streaming,
//! the offline cache (downloaded Qobuz), local files, and Plex. These types
//! let every frontend reason about a track's origin and resolve its cover
//! art uniformly, instead of branching on stringly-typed `source` values at
//! each call site.
//!
//! This is the frontend-agnostic contract behind the source-aware playback
//! context: the now-playing bar, the queue, and the artwork pipeline consume
//! `PlaybackSource` + [`ArtworkRef`] and never special-case a source themselves.
//! The same contract drives the Qobuz Connect queue gate (only castable tracks
//! may be cast — see [`PlaybackSource::is_castable_to_qconnect`]).

use serde::{Deserialize, Serialize};

use crate::playback::QueueTrack;

/// Where a playable track comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackSource {
    /// The Qobuz streaming catalog.
    Qobuz,
    /// A Qobuz track downloaded into the offline cache (`qobuz_download`).
    OfflineCache,
    /// A local file indexed in the user's library.
    Local,
    /// A track served by a Plex Media Server.
    Plex,
}

impl PlaybackSource {
    /// Parse the stringly-typed `source` value carried by `QueueTrack` /
    /// `LocalTrack`. Unknown or absent values default to [`Qobuz`] — every
    /// pre-existing queue track was Qobuz, so this preserves history.
    ///
    /// [`Qobuz`]: PlaybackSource::Qobuz
    pub fn from_source_str(s: Option<&str>) -> Self {
        match s {
            Some("local") => Self::Local,
            Some("plex") => Self::Plex,
            Some("qobuz_download") => Self::OfflineCache,
            _ => Self::Qobuz,
        }
    }

    /// The canonical string written to `source` fields.
    pub fn as_source_str(self) -> &'static str {
        match self {
            Self::Qobuz => "qobuz",
            Self::OfflineCache => "qobuz_download",
            Self::Local => "local",
            Self::Plex => "plex",
        }
    }

    /// Whether this source streams live from the Qobuz catalog. NOTE: NOT the
    /// cast gate — offline-cache also carries a valid Qobuz id and IS castable.
    /// Use is_castable_to_qconnect for the Qobuz Connect gate.
    pub fn is_qobuz_streamable(self) -> bool {
        matches!(self, Self::Qobuz)
    }

    /// The admission-side cast predicate. Offline-cache maps to castable (the
    /// offline copy carries a valid Qobuz track id). This is the method the
    /// QConnect gate consults; is_qobuz_streamable stays "streams live from Qobuz".
    ///
    /// Shared QConnect-admission gate primitive: this is the single predicate
    /// both the Tauri and the upcoming Slint frontends call to gate casting.
    pub fn is_castable_to_qconnect(self) -> bool {
        matches!(self, Self::Qobuz | Self::OfflineCache)
    }
}

/// Admission-only origin tag. Unlike PlaybackSource, this has ExternalUnknown
/// so the Qobuz Connect gate can default unknown/absent to *blocked* not *Qobuz*.
///
/// Shared QConnect-admission gate primitive consumed by the Slint port (its
/// strict-parse companion for the cast gate); kept intentionally for that use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackOriginTag {
    Qobuz,
    OfflineCache,
    Local,
    Plex,
    ExternalUnknown,
}

impl PlaybackSource {
    /// Strict parse for the admission path: unknown/absent → ExternalUnknown (blocked).
    ///
    /// Shared QConnect-admission gate primitive consumed by the Slint port (it
    /// feeds the cast gate, where unknown origins must block, not default to Qobuz).
    pub fn from_source_str_strict(s: Option<&str>) -> TrackOriginTag {
        match s {
            Some("qobuz") => TrackOriginTag::Qobuz,
            Some("qobuz_download") => TrackOriginTag::OfflineCache,
            Some("local") => TrackOriginTag::Local,
            Some("plex") => TrackOriginTag::Plex,
            _ => TrackOriginTag::ExternalUnknown,
        }
    }
}

impl TrackOriginTag {
    pub fn is_castable_to_qconnect(self) -> bool {
        matches!(self, Self::Qobuz | Self::OfflineCache)
    }
}

/// A reference to a piece of cover art, resolvable regardless of origin.
///
/// The artwork loaders historically handled only remote HTTP URLs, which is
/// why local-file and Plex artwork failed to reach the UI. This enum is the
/// uniform contract: a frontend's artwork pipeline matches on it and fetches
/// the bytes the right way for each variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtworkRef {
    /// An HTTP(S) URL (Qobuz covers; Plex thumbs that already carry a token).
    Remote(String),
    /// A path to a cover image on the local filesystem.
    LocalFile(String),
    /// A Plex thumbnail fetched with an auth token appended. `path` is the
    /// server-relative thumb path (e.g. `/library/metadata/42/thumb/1`).
    ///
    /// `size` selects the URL form (see [`plex_thumb_url`]): `Some(n)` builds
    /// a server-side transcode URL downscaled to `n`×`n` (so we download what
    /// we render, not the full-res original); `None` keeps the raw full-res
    /// tokenized path (album-detail full covers / fallbacks).
    PlexThumb {
        base_url: String,
        token: String,
        path: String,
        size: Option<u32>,
    },
    /// Cover bytes already in memory (e.g. embedded tags).
    Embedded(Vec<u8>),
    /// No artwork available.
    None,
}

impl ArtworkRef {
    /// True when there is effectively nothing to load (explicit `None` or an
    /// empty Remote/LocalFile string).
    pub fn is_empty(&self) -> bool {
        match self {
            ArtworkRef::None => true,
            ArtworkRef::Remote(s) | ArtworkRef::LocalFile(s) => s.is_empty(),
            ArtworkRef::Embedded(b) => b.is_empty(),
            ArtworkRef::PlexThumb { path, .. } => path.is_empty(),
        }
    }

    /// A URL suitable for the MPRIS `mpris:artUrl` property (and other OS media
    /// controls). Mirrors the Tauri frontend's `normalizeCoverUrlForMetadata`:
    /// - **Remote** HTTP(S) covers (Qobuz) pass through — clients fetch them.
    /// - **LocalFile** bare paths become a proper percent-encoded `file://`
    ///   URI (MPRIS clients cannot read a bare path or an `asset://` URL).
    /// - **PlexThumb** becomes the tokenized HTTP URL the client can fetch.
    /// - **Embedded** bytes / **None** have no URL (`None`).
    pub fn to_mpris_url(&self) -> Option<String> {
        match self {
            ArtworkRef::Remote(s) if !s.is_empty() => Some(s.clone()),
            ArtworkRef::LocalFile(p) if !p.is_empty() => {
                // Already a file URL? keep it. Otherwise build one (absolute
                // paths only — `from_file_path` rejects relative, → None).
                if p.starts_with("file://") {
                    Some(p.clone())
                } else {
                    url::Url::from_file_path(p).ok().map(|u| u.to_string())
                }
            }
            ArtworkRef::PlexThumb {
                base_url,
                token,
                path,
                size,
            } if !path.is_empty() => Some(plex_thumb_url(base_url, token, path, *size)),
            _ => None,
        }
    }
}

/// Build the fetchable Plex artwork URL.
///
/// `Some(size)` → a server-side **transcode** URL downscaled to `size`×`size`
/// (so the client downloads roughly what it renders instead of the full-res
/// original). `None` (or `Some(0)`) → the raw full-res tokenized path.
///
/// Mirrors the Tauri frontend's `buildPlexArtworkUrl` exactly: the base is
/// trailing-slash-stripped, the inner `url=` (the original `/library` path)
/// and the token are percent-encoded, and `minSize=1` is appended. The raw
/// fallback keeps the non-percent-encoded token to match the historical
/// behavior (and the existing regression tests).
pub fn plex_thumb_url(base_url: &str, token: &str, path: &str, size: Option<u32>) -> String {
    let base = base_url.trim_end_matches('/');
    match size {
        Some(n) if n > 0 => format!(
            "{base}/photo/:/transcode?url={}&width={n}&height={n}&minSize=1&X-Plex-Token={}",
            urlencoding::encode(path),
            urlencoding::encode(token),
        ),
        _ => {
            let sep = if path.contains('?') { '&' } else { '?' };
            format!("{base}{path}{sep}X-Plex-Token={token}")
        }
    }
}

impl QueueTrack {
    /// The track's playback source, parsed from its `source` field.
    pub fn source_kind(&self) -> PlaybackSource {
        PlaybackSource::from_source_str(self.source.as_deref())
    }

    /// A uniform reference to this track's cover art.
    ///
    /// The heuristic is source-agnostic (it does not trust `source` to be
    /// set): an `http(s)://` value is [`ArtworkRef::Remote`]; a `file://`
    /// value or a bare filesystem path is [`ArtworkRef::LocalFile`] (local
    /// library + offline-cache covers live on disk). Plex thumbnails that
    /// need a token are produced by the Plex queue builder via
    /// [`ArtworkRef::PlexThumb`] directly, not here.
    pub fn artwork_ref(&self) -> ArtworkRef {
        let raw = self.artwork_url.as_deref().unwrap_or("");
        if raw.is_empty() {
            return ArtworkRef::None;
        }
        if raw.starts_with("http://") || raw.starts_with("https://") {
            ArtworkRef::Remote(raw.to_string())
        } else if raw.starts_with("file://") {
            // Queue builders hand this layer a URI, not a filesystem path.
            // Decode it before `to_mpris_url` canonicalises it again; merely
            // stripping the scheme turns `%23` into a literal filename and
            // the second encode publishes `%2523` to MPRIS.
            let path = url::Url::parse(raw)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| raw.trim_start_matches("file://").to_string());
            ArtworkRef::LocalFile(path)
        } else {
            ArtworkRef::LocalFile(raw.to_string())
        }
    }

    /// A uniform reference to this track's cover art, Plex-aware.
    ///
    /// Same value-driven classification as [`artwork_ref`](Self::artwork_ref),
    /// except a bare server-relative Plex thumb path (`/library/...` or
    /// `/photo/...`) is resolved to [`ArtworkRef::PlexThumb`] using the supplied
    /// current credentials — so the now-playing bar, queue panel, and MPRIS get
    /// the tokenized cover instead of a `file://`-poisoned local-read miss.
    ///
    /// `qbz-models` stays credential-free: the caller fetches the live
    /// `{base_url, token}` (from the Slint Plex settings store) and threads them
    /// in. When creds are missing, or the path is not a Plex thumb path, this
    /// falls back to [`artwork_ref`](Self::artwork_ref).
    ///
    /// `size` is threaded into the resulting [`ArtworkRef::PlexThumb`] so the
    /// caller picks the render-target size: `Some(n)` → server-side transcode
    /// at `n`×`n`; `None` → raw full-res (e.g. for MPRIS art that wants a
    /// larger image). See [`plex_thumb_url`].
    pub fn artwork_ref_with_plex(
        &self,
        base_url: &str,
        token: &str,
        size: Option<u32>,
    ) -> ArtworkRef {
        let raw = self.artwork_url.as_deref().unwrap_or("");
        let is_plex_path = raw.starts_with("/library/") || raw.starts_with("/photo/");
        if is_plex_path && !base_url.is_empty() && !token.is_empty() {
            return ArtworkRef::PlexThumb {
                base_url: base_url.to_string(),
                token: token.to_string(),
                path: raw.to_string(),
                size,
            };
        }
        self.artwork_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_str_roundtrip_and_default() {
        assert_eq!(PlaybackSource::from_source_str(Some("local")), PlaybackSource::Local);
        assert_eq!(PlaybackSource::from_source_str(Some("plex")), PlaybackSource::Plex);
        assert_eq!(
            PlaybackSource::from_source_str(Some("qobuz_download")),
            PlaybackSource::OfflineCache
        );
        assert_eq!(PlaybackSource::from_source_str(Some("qobuz")), PlaybackSource::Qobuz);
        // Unknown / absent -> Qobuz (historical default).
        assert_eq!(PlaybackSource::from_source_str(None), PlaybackSource::Qobuz);
        assert_eq!(PlaybackSource::from_source_str(Some("???")), PlaybackSource::Qobuz);
        for s in [
            PlaybackSource::Qobuz,
            PlaybackSource::OfflineCache,
            PlaybackSource::Local,
            PlaybackSource::Plex,
        ] {
            assert_eq!(PlaybackSource::from_source_str(Some(s.as_source_str())), s);
        }
    }

    #[test]
    fn offline_cache_is_castable() {
        assert!(PlaybackSource::OfflineCache.is_castable_to_qconnect());
        assert!(TrackOriginTag::OfflineCache.is_castable_to_qconnect());
    }

    #[test]
    fn strict_parse_blocks_unknown_and_absent() {
        use TrackOriginTag::*;
        assert_eq!(PlaybackSource::from_source_str_strict(Some("qobuz")), Qobuz);
        assert_eq!(PlaybackSource::from_source_str_strict(Some("local")), Local);
        assert_eq!(PlaybackSource::from_source_str_strict(Some("plex")), Plex);
        assert_eq!(PlaybackSource::from_source_str_strict(Some("qobuz_download")), OfflineCache);
        assert_eq!(PlaybackSource::from_source_str_strict(None), ExternalUnknown);
        assert_eq!(PlaybackSource::from_source_str_strict(Some("???")), ExternalUnknown);
        // Lenient parser still defaults to Qobuz (playback compatibility).
        assert_eq!(PlaybackSource::from_source_str(None), PlaybackSource::Qobuz);
    }

    #[test]
    fn only_qobuz_is_castable() {
        assert!(PlaybackSource::Qobuz.is_qobuz_streamable());
        assert!(!PlaybackSource::OfflineCache.is_qobuz_streamable());
        assert!(!PlaybackSource::Local.is_qobuz_streamable());
        assert!(!PlaybackSource::Plex.is_qobuz_streamable());
    }

    fn track_with(source: Option<&str>, artwork: Option<&str>) -> QueueTrack {
        QueueTrack {
            id: 1,
            title: "t".into(),
            version: None,
            artist: "a".into(),
            album: "al".into(),
            album_version: None,
            duration_secs: 0,
            artwork_url: artwork.map(|s| s.to_string()),
            hires: false,
            bit_depth: None,
            sample_rate: None,
            is_local: false,
            album_id: None,
            artist_id: None,
            streamable: true,
            source: source.map(|s| s.to_string()),
            parental_warning: false,
            source_item_id_hint: None,
            context_kind: None,
            context_id: None,
            isrc: None,
            recording_mbid: None,
        }
    }

    #[test]
    fn artwork_ref_classifies_by_value() {
        assert_eq!(track_with(None, None).artwork_ref(), ArtworkRef::None);
        assert_eq!(
            track_with(Some("qobuz"), Some("https://x/cover.jpg")).artwork_ref(),
            ArtworkRef::Remote("https://x/cover.jpg".into())
        );
        assert_eq!(
            track_with(Some("local"), Some("/home/u/cover.jpg")).artwork_ref(),
            ArtworkRef::LocalFile("/home/u/cover.jpg".into())
        );
        assert_eq!(
            track_with(Some("local"), Some("file:///home/u/cover.jpg")).artwork_ref(),
            ArtworkRef::LocalFile("/home/u/cover.jpg".into())
        );
        // A raw Plex thumb path stays LocalFile here (no creds at the model
        // layer); PlexThumb is produced only by `artwork_ref_with_plex`.
        assert_eq!(
            track_with(Some("plex"), Some("/library/metadata/42/thumb/1")).artwork_ref(),
            ArtworkRef::LocalFile("/library/metadata/42/thumb/1".into())
        );
    }

    #[test]
    fn local_file_uri_round_trips_special_path_chars_for_mpris() {
        let path = "/mnt/music/Disc 01 - Soundtrack #01/100%? cover.jpg";
        let queue_uri = "file:///mnt/music/Disc 01 - Soundtrack %2301/100%25%3F cover.jpg";
        let artwork = track_with(Some("local"), Some(queue_uri)).artwork_ref();
        assert_eq!(artwork, ArtworkRef::LocalFile(path.into()));
        assert_eq!(
            artwork.to_mpris_url(),
            url::Url::from_file_path(path)
                .ok()
                .map(|url| url.to_string())
        );
    }

    #[test]
    fn artwork_ref_with_plex_resolves_library_path() {
        // Plex thumb path + creds → PlexThumb (the now-playing/queue/MPRIS path).
        assert_eq!(
            track_with(Some("plex"), Some("/library/metadata/42/thumb/1"))
                .artwork_ref_with_plex("http://plex.local:32400", "tok", None),
            ArtworkRef::PlexThumb {
                base_url: "http://plex.local:32400".into(),
                token: "tok".into(),
                path: "/library/metadata/42/thumb/1".into(),
                size: None,
            }
        );
        // `/photo/...` transcode paths classify the same way.
        assert_eq!(
            track_with(Some("plex"), Some("/photo/:/transcode?url=x"))
                .artwork_ref_with_plex("http://plex.local:32400", "tok", None),
            ArtworkRef::PlexThumb {
                base_url: "http://plex.local:32400".into(),
                token: "tok".into(),
                path: "/photo/:/transcode?url=x".into(),
                size: None,
            }
        );
        // A size is threaded straight into the ref (URL form decided later).
        assert_eq!(
            track_with(Some("plex"), Some("/library/metadata/42/thumb/1"))
                .artwork_ref_with_plex("http://plex.local:32400", "tok", Some(264)),
            ArtworkRef::PlexThumb {
                base_url: "http://plex.local:32400".into(),
                token: "tok".into(),
                path: "/library/metadata/42/thumb/1".into(),
                size: Some(264),
            }
        );
    }

    #[test]
    fn artwork_ref_with_plex_falls_back_without_creds_or_non_plex() {
        // Missing creds → no PlexThumb; falls back to LocalFile (the raw path).
        assert_eq!(
            track_with(Some("plex"), Some("/library/metadata/42/thumb/1"))
                .artwork_ref_with_plex("", "", Some(264)),
            ArtworkRef::LocalFile("/library/metadata/42/thumb/1".into())
        );
        // Non-Plex value is untouched: http stays Remote even with creds.
        assert_eq!(
            track_with(Some("qobuz"), Some("https://x/cover.jpg"))
                .artwork_ref_with_plex("http://plex.local:32400", "tok", Some(264)),
            ArtworkRef::Remote("https://x/cover.jpg".into())
        );
        // A local file:// path stays LocalFile, never mistaken for Plex.
        assert_eq!(
            track_with(Some("local"), Some("file:///home/u/cover.jpg"))
                .artwork_ref_with_plex("http://plex.local:32400", "tok", Some(264)),
            ArtworkRef::LocalFile("/home/u/cover.jpg".into())
        );
    }

    #[test]
    fn plex_thumb_to_mpris_url_tokenizes() {
        // size: None → raw full-res tokenized path (historical behavior).
        let no_query = ArtworkRef::PlexThumb {
            base_url: "http://plex.local:32400".into(),
            token: "tok".into(),
            path: "/library/metadata/42/thumb/1".into(),
            size: None,
        };
        assert_eq!(
            no_query.to_mpris_url().as_deref(),
            Some("http://plex.local:32400/library/metadata/42/thumb/1?X-Plex-Token=tok")
        );
        let with_query = ArtworkRef::PlexThumb {
            base_url: "http://plex.local:32400".into(),
            token: "tok".into(),
            path: "/photo/:/transcode?url=x".into(),
            size: None,
        };
        assert_eq!(
            with_query.to_mpris_url().as_deref(),
            Some("http://plex.local:32400/photo/:/transcode?url=x&X-Plex-Token=tok")
        );
    }

    #[test]
    fn plex_thumb_url_size_emits_transcode_form() {
        // Some(size) → server-side transcode URL, percent-encoded url + token,
        // trailing-slash-stripped base, minSize=1 (1:1 with Tauri's
        // buildPlexArtworkUrl). The `/` in the path is encoded to %2F.
        assert_eq!(
            plex_thumb_url(
                "http://plex.local:32400",
                "tok",
                "/library/metadata/42/thumb/1",
                Some(264),
            ),
            "http://plex.local:32400/photo/:/transcode?url=%2Flibrary%2Fmetadata%2F42%2Fthumb%2F1&width=264&height=264&minSize=1&X-Plex-Token=tok"
        );
        // Trailing slashes on the base are stripped before composing.
        assert_eq!(
            plex_thumb_url(
                "http://plex.local:32400/",
                "t/k",
                "/library/metadata/7/thumb/9",
                Some(96),
            ),
            "http://plex.local:32400/photo/:/transcode?url=%2Flibrary%2Fmetadata%2F7%2Fthumb%2F9&width=96&height=96&minSize=1&X-Plex-Token=t%2Fk"
        );
        // None / Some(0) → raw tokenized path (no transcode).
        assert_eq!(
            plex_thumb_url("http://plex.local:32400", "tok", "/library/metadata/42/thumb/1", None),
            "http://plex.local:32400/library/metadata/42/thumb/1?X-Plex-Token=tok"
        );
        assert_eq!(
            plex_thumb_url("http://plex.local:32400", "tok", "/library/metadata/42/thumb/1", Some(0)),
            "http://plex.local:32400/library/metadata/42/thumb/1?X-Plex-Token=tok"
        );
    }
}
