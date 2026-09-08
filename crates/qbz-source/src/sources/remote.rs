//! What Jellyfin and Subsonic have in common.
//!
//! Both are REMOTE CACHED sources: their library is swept into
//! `qbz-media-cache` and every read the UI makes is answered from that mirror,
//! not from the server. Only [`Source::playback`] and the artwork FETCH touch
//! the network, and both do it by handing out a URL rather than a body.
//!
//! Plex is the same shape and is not folded in here yet — it reads
//! `plex_cache.db` through `qbz-plex`'s own global and carries a five-id
//! identity problem this pair does not have. It joins once the shared cache has
//! proved itself with these two (see `qbz-media-cache`'s header).
//!
//! # Why the connection is a `Mutex<Option<Connection>>`
//!
//! `rusqlite::Connection` is `!Sync`, so it cannot simply live behind a shared
//! reference. `LocalSource` has a real pool (`DbPool`) because `library.db` is
//! read from 48 call sites at scroll speed; this cache is read by the grid in
//! batches and by playback once per track, so one guarded connection is enough
//! and a pool would be machinery without a measurement behind it. If that ever
//! stops being true, the fix is to reuse `DbPool`, not to hand-roll a second
//! one.
//!
//! The guard is `std::sync::Mutex` and is NEVER held across an `.await` — the
//! guard is `!Send`, so doing it inside an `#[async_trait]` method is a compile
//! error rather than a review rule. Same discipline as `PlexSource::server`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use qbz_media_cache::{CachedTrack, RemoteSource};
use qbz_models::QueueTrack;

use crate::art::ArtRef;
use crate::error::SourceError;
use crate::id::{ItemKind, MediaRef, SourceId};
use crate::meta::{ItemMeta, QualityHint, SourceBadge};

/// The per-user cache handle a remote source owns.
/// PUBLIC because the frontend's SYNC writes through it: `qbz-source` owns
/// the handle (so it is bound and torn down with the rest of the registry),
/// while the sweep that fills it lives in the frontend, where the progress UI
/// and the tokio runtime are.
pub struct CacheHandle {
    which: RemoteSource,
    id: SourceId,
    path: Mutex<Option<PathBuf>>,
    conn: Mutex<Option<rusqlite::Connection>>,
}

impl CacheHandle {
    pub(crate) fn new(which: RemoteSource, id: SourceId) -> Self {
        Self {
            which,
            id,
            path: Mutex::new(None),
            conn: Mutex::new(None),
        }
    }

    /// The cache file for the active user. ONE file for every remote source —
    /// the table's `source` column is what separates them, which is the whole
    /// point of the shared schema.
    pub fn bind(&self, dir: &Path) {
        let p = dir.join("remote_cache.db");
        if let Ok(mut slot) = self.path.lock() {
            *slot = Some(p);
        }
        // Drop the previous user's handle rather than reopening eagerly: a user
        // who never opens Local Library should not pay for a database open, and
        // the first read will do it.
        if let Ok(mut c) = self.conn.lock() {
            *c = None;
        }
    }

    pub fn teardown(&self) {
        if let Ok(mut slot) = self.path.lock() {
            *slot = None;
        }
        if let Ok(mut c) = self.conn.lock() {
            *c = None;
        }
    }

    /// Run `f` against the cache, opening it on first use.
    ///
    /// `None` means "no user bound, or the cache could not be opened" — the
    /// same shape `local_state::with_db` has, and the callers treat it the same
    /// way: an empty result, never a panic.
    pub fn with<R>(&self, f: impl FnOnce(&rusqlite::Connection) -> R) -> Option<R> {
        let mut guard = self.conn.lock().ok()?;
        if guard.is_none() {
            let path = self.path.lock().ok()?.clone()?;
            match qbz_media_cache::open(&path) {
                Ok(c) => *guard = Some(c),
                Err(e) => {
                    log::warn!("[qbz-source] {} cache open failed: {e}", self.id);
                    return None;
                }
            }
        }
        guard.as_ref().map(f)
    }

    /// Run `f` against the cache with WRITE access. Same handle — SQLite
    /// serialises writers itself and this one is behind a mutex anyway.
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut rusqlite::Connection) -> R) -> Option<R> {
        let mut guard = self.conn.lock().ok()?;
        if guard.is_none() {
            let path = self.path.lock().ok()?.clone()?;
            match qbz_media_cache::open(&path) {
                Ok(c) => *guard = Some(c),
                Err(e) => {
                    log::warn!("[qbz-source] {} cache open failed: {e}", self.id);
                    return None;
                }
            }
        }
        guard.as_mut().map(f)
    }

    pub fn which(&self) -> RemoteSource {
        self.which
    }
}

/// The one place a cached row becomes a `QueueTrack`.
///
/// Shared rather than written per source because every field decision below is
/// a decision about QBZ, not about the vendor — and two copies would drift the
/// way the three `play_audible`s did.
pub(crate) fn cached_to_queue_track(t: &CachedTrack, source: SourceId) -> QueueTrack {
    QueueTrack {
        // The NAMESPACED id. It is the one the Local Library grid publishes and
        // the one `claim` recognises, so the queue must carry the same number
        // or "play from the queue" and "play from the grid" disagree — which is
        // bug 2, in a new source.
        id: t.id as u64,
        title: t.title.clone(),
        version: None,
        artist: t.artist.clone(),
        album: t.album.clone(),
        album_version: None,
        duration_secs: t.duration_ms / 1000,
        // A source-tagged RAW token, not a URL. The vendor token is stable,
        // while credentials and requested size are resolved only when a
        // consumer needs bytes. The source tag is load-bearing: recent-track
        // playback reaches this mapper directly, with no LocalTrack adapter
        // available to add it later, and an untagged opaque token would be
        // misclassified as a local filesystem path by NPB/MPRIS artwork.
        artwork_url: t
            .artwork_token
            .clone()
            .or_else(|| t.collection_artwork_token.clone())
            .map(|token| format!("{}:{token}", source.as_str())),
        hires: t.bit_depth.map(|d| d > 16).unwrap_or(false),
        bit_depth: t.bit_depth,
        // kHz — the `QueueTrack` convention, NOT Hz.
        sample_rate: t.sample_rate_hz.map(|hz| hz as f64 / 1000.0),
        // TRUE, and it is not a stub: `is_local` gates the frontend's
        // "this row does not go through the Qobuz tier walk" behaviour, which
        // is exactly right for a row served by someone else's server.
        is_local: true,
        album_id: Some(t.album_id.clone()),
        artist_id: None,
        // A remote server's rights are not Qobuz's. `qbz-core` never asks about
        // this row, and marking it unstreamable would drop it from the queue.
        streamable: true,
        source: Some(source.as_str().to_string()),
        parental_warning: false,
        // The SERVER's own id. `claim` prefers it over the numeric id, so a
        // queue row survives a cache rebuild that renumbers rowids — the one
        // way the namespaced id can legitimately change.
        source_item_id_hint: Some(t.item_id.clone()),
        context_kind: None,
        context_id: None,
        // Whatever the server exposed (Jellyfin ProviderIds / OpenSubsonic).
        isrc: t.isrc.clone(),
        recording_mbid: t.recording_mbid.clone(),
    }
}

/// Row-level display fields for one album or track.
pub(crate) fn meta_of_rows(
    rows: &[CachedTrack],
    item: &MediaRef,
    badge: SourceBadge,
    art: ArtRef,
) -> ItemMeta {
    let first = rows.first();
    ItemMeta {
        title: match (item.kind(), first) {
            (ItemKind::Track, Some(f)) => f.title.clone(),
            (_, Some(f)) => f.album.clone(),
            (_, None) => String::new(),
        },
        subtitle: first
            .map(|f| {
                if f.album_artist.is_empty() {
                    f.artist.clone()
                } else {
                    f.album_artist.clone()
                }
            })
            .unwrap_or_default(),
        year: first.and_then(|f| f.year),
        track_count: Some(rows.len() as u32),
        duration_secs: Some(rows.iter().map(|t| t.duration_ms / 1000).sum()),
        quality: first
            .map(|f| {
                QualityHint::from_hz(
                    f.bit_depth,
                    f.sample_rate_hz.unwrap_or(0) as f64,
                    // The tier only needs to know whether it is lossy; the
                    // container name itself is display data.
                    if f.container.eq_ignore_ascii_case("mp3")
                        || f.codec
                            .as_deref()
                            .is_some_and(|c| c.eq_ignore_ascii_case("mp3"))
                    {
                        "MP3"
                    } else {
                        "FLAC"
                    },
                )
            })
            .unwrap_or_default(),
        art,
        badge,
        kind_label: item.kind().label(),
    }
}

/// `NotFound` for an item this source owns but cannot resolve.
pub(crate) fn not_found(by: SourceId, item: &MediaRef) -> SourceError {
    SourceError::NotFound {
        by,
        kind: item.kind(),
        id: item.id().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> CachedTrack {
        CachedTrack {
            id: qbz_media_cache::RemoteSource::Jellyfin.namespace(7),
            source: "jellyfin".into(),
            item_id: "srv-abc".into(),
            title: "Harvester Of Sorrow".into(),
            artist: "Metallica".into(),
            album_artist: "Metallica".into(),
            album: "...And Justice For All".into(),
            album_id: "alb-1".into(),
            duration_ms: 348_480,
            bit_depth: Some(24),
            sample_rate_hz: Some(96_000),
            container: "flac".into(),
            artwork_token: Some("748607df".into()),
            year: Some(1988),
            ..Default::default()
        }
    }

    /// Every field the queue reads, and the two conversions that are easy to
    /// get backwards: ms -> s and Hz -> kHz.
    #[test]
    fn a_cached_row_becomes_a_queue_track_in_qbz_units() {
        let qt = cached_to_queue_track(&row(), SourceId::JELLYFIN);
        assert_eq!(qt.duration_secs, 348, "ms were not converted to seconds");
        assert_eq!(qt.sample_rate, Some(96.0), "Hz were not converted to kHz");
        assert_eq!(qt.bit_depth, Some(24));
        assert!(qt.hires);
        assert_eq!(qt.source.as_deref(), Some("jellyfin"));
        assert!(qt.is_local, "a remote row must skip the Qobuz tier walk");
        assert!(
            qt.streamable,
            "an unstreamable row is dropped from the queue"
        );
    }

    /// The queue carries the NAMESPACED id and the SERVER's id, not one or the
    /// other. The numeric id is what the grid published; the hint is what
    /// survives a cache rebuild that renumbers rowids.
    #[test]
    fn the_queue_row_carries_both_identities() {
        let r = row();
        let qt = cached_to_queue_track(&r, SourceId::JELLYFIN);
        assert_eq!(qt.id, r.id as u64);
        assert_eq!(qt.source_item_id_hint.as_deref(), Some("srv-abc"));
    }

    /// The source-tagged RAW token, never a URL: a URL embeds credentials and
    /// a size, and a queue row outlives both. The tag keeps an opaque server
    /// id from being mistaken for a local path by consumers outside this crate.
    #[test]
    fn the_queue_row_holds_the_art_token_not_a_url() {
        let qt = cached_to_queue_track(&row(), SourceId::JELLYFIN);
        assert_eq!(qt.artwork_url.as_deref(), Some("jellyfin:748607df"));
        assert!(!qt.artwork_url.unwrap().contains("http"));
    }

    #[test]
    fn the_queue_prefers_disc_art_then_falls_back_to_collection_art() {
        let mut with_disc = row();
        with_disc.artwork_token = Some("disc-02".into());
        with_disc.collection_artwork_token = Some("box-cover".into());
        assert_eq!(
            cached_to_queue_track(&with_disc, SourceId::JELLYFIN)
                .artwork_url
                .as_deref(),
            Some("jellyfin:disc-02")
        );

        with_disc.artwork_token = None;
        assert_eq!(
            cached_to_queue_track(&with_disc, SourceId::JELLYFIN)
                .artwork_url
                .as_deref(),
            Some("jellyfin:box-cover")
        );
    }

    #[test]
    fn album_meta_sums_the_album_and_reads_the_first_row() {
        let mut a = row();
        a.duration_ms = 60_000;
        let mut b = row();
        b.item_id = "srv-def".into();
        b.duration_ms = 90_000;
        let item = MediaRef::new(SourceId::JELLYFIN, ItemKind::Album, "alb-1");
        let m = meta_of_rows(&[a, b], &item, SourceBadge::Jellyfin, ArtRef::None);
        assert_eq!(m.title, "...And Justice For All");
        assert_eq!(m.subtitle, "Metallica");
        assert_eq!(m.track_count, Some(2));
        assert_eq!(m.duration_secs, Some(150));
        assert_eq!(m.year, Some(1988));
        assert_eq!(m.quality.tier(), "hires");
        assert_eq!(m.badge, SourceBadge::Jellyfin);
    }

    /// A lossy row must badge as mp3 even when the container name is empty and
    /// only the codec says so.
    #[test]
    fn a_lossy_row_tiers_as_mp3_from_either_field() {
        let item = MediaRef::new(SourceId::SUBSONIC, ItemKind::Album, "a");
        let mut by_container = row();
        by_container.container = "mp3".into();
        by_container.bit_depth = None;
        assert_eq!(
            meta_of_rows(&[by_container], &item, SourceBadge::Subsonic, ArtRef::None)
                .quality
                .tier(),
            "mp3"
        );
        let mut by_codec = row();
        by_codec.container = String::new();
        by_codec.codec = Some("MP3".into());
        by_codec.bit_depth = None;
        assert_eq!(
            meta_of_rows(&[by_codec], &item, SourceBadge::Subsonic, ArtRef::None)
                .quality
                .tier(),
            "mp3"
        );
    }

    #[test]
    fn a_track_meta_shows_the_track_title_not_the_album() {
        let item = MediaRef::new(SourceId::JELLYFIN, ItemKind::Track, "x");
        let m = meta_of_rows(&[row()], &item, SourceBadge::Jellyfin, ArtRef::None);
        assert_eq!(m.title, "Harvester Of Sorrow");
        assert_eq!(m.kind_label, "track");
    }
}
