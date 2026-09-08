//! Plex — and the home of bug 2.
//!
//! Plex has FIVE ids for one track (survey §6.2): rating key `44440`, cache row
//! id `44440`, namespaced row id `1099511671736`, content-hash album key
//! `plex:5677211365378243606`, per-edition album key `plex:album:44439`. Two
//! different code paths pick different ones for `QueueTrack.id`:
//!
//! | path | `QueueTrack.id` | `source_item_id_hint` |
//! |---|---|---|
//! | LocalLibrary (`local_playback.rs:31-108`) | namespaced `1099511671736` | `file_path` = the raw rating key `"44440"` |
//! | MyQBZ (`PlexSource::tracks` + `resolve_collection_tracks`) | raw `44440` | overwritten with `plex:<hash>` |
//!
//! Nobody wrote that table down: `plex_rating_key` (local_playback.rs:229-234,
//! copy-pasted to local_album_actions.rs:439-450) is one engineer's
//! reconstruction of it, and its fallback ("ignore a `plex:` hint, use the
//! numeric queue id") is correct for the MyQBZ path and WRONG for the
//! LocalLibrary path — it only survives because that path always arrives with a
//! good hint. So [`PlexSource::rating_key`] does not guess; it decides on
//! evidence, and the table lives here, once.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use qbz_models::QueueTrack;
use qbz_plex::PlexCachedTrack;

use crate::art::{ArtRef, ArtSize};
use crate::error::SourceError;
use crate::id::{ItemKind, MediaRef, RawRef, SourceId};
use crate::meta::{ItemMeta, QualityHint, SourceBadge};
use crate::playback::PlaybackTicket;
use crate::source::Source;

/// Namespace bit for Plex track ids so they can never collide with a real
/// `local_tracks.id`. Moved from `local_plex::PLEX_TRACK_ID_FLOOR`
/// (local_plex.rs:37); every frontend path uses the same namespace.
pub const PLEX_TRACK_ID_FLOOR: u64 = 1 << 40;

/// Memo ceiling, mirroring `artwork_qt::MEMO_CAP` (artwork_qt.rs:82).
const MEMO_CAP: usize = 8192;

/// Map the Plex cache row into the queue shape owned by this source.
///
/// The raw numeric rating key is deliberately the queue id for MyQBZ. The
/// registry's claim path can then route playback back to the same Plex object,
/// while `resolve_collection_tracks` stamps the album boundary separately.
fn plex_queue_track(track: &PlexCachedTrack) -> QueueTrack {
    let id = track.rating_key.parse().unwrap_or(track.id);
    let sample_rate = (track.sample_rate > 0).then_some(track.sample_rate as f64 / 1000.0);
    QueueTrack {
        id,
        title: track.title.clone(),
        version: None,
        artist: track.artist.clone(),
        album: track.album.clone(),
        album_version: None,
        duration_secs: track.duration_secs,
        artwork_url: track
            .artwork_path
            .clone()
            .or_else(|| track.collection_artwork_path.clone()),
        hires: track.bit_depth.is_some_and(|depth| depth > 16),
        bit_depth: track.bit_depth,
        sample_rate,
        is_local: true,
        album_id: Some(track.album_key.clone()),
        artist_id: None,
        streamable: true,
        source: Some("plex".to_string()),
        parental_warning: false,
        source_item_id_hint: None,
        context_kind: None,
        context_id: None,
        isrc: None,
        recording_mbid: None,
    }
}

/// The Plex credentials, implemented by the frontend over its EXISTING store.
///
/// Plex credentials live in a per-user SQLite store `qbz-qt` already owns
/// (`local_plex.rs:33`, `:48-98`, over `qbz_app::settings::plex::
/// PlexSettingsState`). Opening a second copy here would be a second authority
/// — and would drag `qbz-app` (hence `qbz-core`/`qbz-player`/`qbz-audio`) into
/// this crate's graph, which design §8 forbids. So it is injected: `qbz-qt`'s
/// impl is three lines over `local_plex::is_enabled()` / `local_plex::
/// settings()`.
pub trait PlexCreds: Send + Sync + 'static {
    /// The master toggle (`local_plex::is_enabled`).
    fn is_enabled(&self) -> bool;
    /// `(base_url, token)`, both non-empty, or `None`
    /// (`local_plex::settings()`, checked exactly as local_playback.rs:194-198
    /// and local_plex.rs:250-253 check it).
    fn server(&self) -> Option<(String, String)>;
}

/// The Plex source.
pub struct PlexSource {
    creds: RwLock<Option<Arc<dyn PlexCreds>>>,
    /// MediaRef id → the RAW `/library/...` thumb path, so a repeat `artwork`
    /// call costs no cache query.
    art: RwLock<HashMap<String, String>>,
}

impl PlexSource {
    /// A source with no credentials published yet.
    pub fn new() -> Self {
        Self {
            creds: RwLock::new(None),
            art: RwLock::new(HashMap::new()),
        }
    }

    /// Publish the frontend's credentials handle. `None` clears it.
    pub fn set_creds(&self, c: Option<Arc<dyn PlexCreds>>) {
        if let Ok(mut slot) = self.creds.write() {
            *slot = c;
        }
    }

    /// `(base_url, token)` when Plex is usable right now.
    ///
    /// Clone-then-drop: the guard is never held across an `.await`. It is a
    /// `std::sync::RwLock` (this crate has no tokio dep, §8), whose guard is
    /// `!Send`, so holding it across an await inside an `#[async_trait]` method
    /// is a COMPILE ERROR rather than a code-review rule.
    fn server(&self) -> Option<(String, String)> {
        let creds = { self.creds.read().ok()?.clone()? };
        if !creds.is_enabled() {
            return None;
        }
        creds
            .server()
            .filter(|(base, token)| !base.is_empty() && !token.is_empty())
    }

    /// Resolve a MASKED cache row id back to its rating key.
    ///
    /// `PlexCachedTrack.id` is `playback_track_id(rating_key)`
    /// (`qbz-plex/lib.rs:737-741`): the numeric rating key when it parses, else
    /// `DefaultHasher(rating_key)`. `map_cached_to_local_track`
    /// (local_plex.rs:277) then stores `FLOOR | (id & (FLOOR-1))`, so the mask
    /// recovers `id`, NOT necessarily the rating key.
    ///
    /// Fast path: the common case is a numeric rating key, where `id` IS the
    /// key — one indexed PRIMARY KEY lookup CONFIRMS it instead of assuming it.
    /// Slow path: a non-numeric rating key hashed into `id`, which only a scan
    /// can invert. That case is today's silent 404 (`plex_rating_key` returns
    /// the hash and `GET /library/metadata/<hash>` misses).
    fn resolve_cache_row_id(masked: u64) -> Option<String> {
        let candidate = masked.to_string();
        if let Ok(rows) = qbz_plex::plex_cache_get_cached_tracks_by_keys(&[candidate.clone()]) {
            if rows
                .iter()
                .any(|t| t.id & (PLEX_TRACK_ID_FLOOR - 1) == masked)
            {
                return Some(candidate);
            }
        }
        // `plex_cache_search_tracks("")` matches every row (`WHERE ?1 = '' OR
        // …`, qbz-plex/lib.rs:1490-1494); the cache is a bounded set.
        qbz_plex::plex_cache_search_tracks(String::new(), None)
            .ok()?
            .into_iter()
            .find(|t| t.id & (PLEX_TRACK_ID_FLOOR - 1) == masked)
            .map(|t| t.rating_key)
    }

    /// The rating key a track reference names, decided on EVIDENCE.
    ///
    /// Replaces `local_playback::plex_rating_key` (local_playback.rs:229-234)
    /// AND its copy-paste at `local_album_actions.rs:439-450` (deleted, not
    /// moved). The ordered cases:
    ///
    /// 1. a hint that is present and NOT `plex:`-prefixed → that IS the rating
    ///    key (the LocalLibrary path, where `local_queue_track` stamps
    ///    `file_path`, which for a Plex row is the server key);
    /// 2. a numeric id with [`PLEX_TRACK_ID_FLOOR`] set → mask the namespace
    ///    bit and RESOLVE that cache row id, never `id.to_string()`;
    /// 3. a bare numeric id → the rating key (the MyQBZ path built by
    ///    [`plex_queue_track`]);
    /// 4. otherwise → [`SourceError::BadIdShape`]. Notably a NON-NUMERIC rating
    ///    key with no hint: today `playback_track_id` falls back to a hash for
    ///    those, so case 3 would silently produce a hash and 404. Here it is a
    ///    named error.
    ///
    /// `resolve` is injected so the pure ladder is unit-testable without a Plex
    /// cache on disk; [`PlexSource::rating_key`] passes
    /// [`PlexSource::resolve_cache_row_id`].
    fn rating_key_with(
        raw: &RawRef,
        resolve: impl Fn(u64) -> Option<String>,
    ) -> Result<String, SourceError> {
        if let Some(hint) = raw.hint_str() {
            if !hint.starts_with("plex:") {
                return Ok(hint.to_string());
            }
        }
        match raw.numeric() {
            Some(n) if n & PLEX_TRACK_ID_FLOOR != 0 => {
                let masked = n & (PLEX_TRACK_ID_FLOOR - 1);
                resolve(masked).ok_or(SourceError::NotFound {
                    by: SourceId::PLEX,
                    kind: ItemKind::Track,
                    id: raw.id.clone(),
                })
            }
            Some(n) => Ok(n.to_string()),
            None => Err(SourceError::BadIdShape {
                by: SourceId::PLEX,
                id: raw.id.clone(),
                why: "not a rating key: non-numeric id with no usable rating-key hint",
            }),
        }
    }

    fn rating_key(raw: &RawRef) -> Result<String, SourceError> {
        Self::rating_key_with(raw, Self::resolve_cache_row_id)
    }

    fn cached_rows(item: &MediaRef) -> Vec<PlexCachedTrack> {
        match item.kind() {
            ItemKind::Album => {
                // The provider-owned replacement for the legacy mixtape and
                // `local_plex::album_tracks` cache reads.
                qbz_plex::plex_cache_get_album_tracks(item.id().to_string()).unwrap_or_default()
            }
            ItemKind::Track => {
                qbz_plex::plex_cache_get_cached_tracks_by_keys(&[item.id().to_string()])
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn not_found(item: &MediaRef) -> SourceError {
        SourceError::NotFound {
            by: SourceId::PLEX,
            kind: item.kind(),
            id: item.id().to_string(),
        }
    }

    fn preferred_art(item: &MediaRef, rows: &[PlexCachedTrack]) -> Option<String> {
        rows.iter().find_map(|track| {
            let item_art = track.artwork_path.clone().filter(|path| !path.is_empty());
            let collection_art = track
                .collection_artwork_path
                .clone()
                .filter(|path| !path.is_empty());
            if item.kind() == ItemKind::Track {
                item_art.or(collection_art)
            } else {
                collection_art.or(item_art)
            }
        })
    }

    fn memoize_art(&self, item: &MediaRef, rows: &[PlexCachedTrack]) {
        let Some(path) = Self::preferred_art(item, rows) else {
            return;
        };
        if let Ok(mut memo) = self.art.write() {
            if memo.len() >= MEMO_CAP {
                memo.clear();
            }
            memo.insert(item.id().to_string(), path);
        }
    }
}

impl Default for PlexSource {
    fn default() -> Self {
        Self::new()
    }
}

/// A Plex thumb is a SERVER-RELATIVE path, never a filesystem path.
/// Moved from `local_plex::is_thumb_path` (local_plex.rs:239-241).
pub fn is_thumb_path(path: &str) -> bool {
    path.starts_with("/library/") || path.starts_with("/photo/")
}

/// POSITIVE ownership: the source word, one of the two declared `plex:` id
/// shapes, or the namespace bit (`local_plex::is_plex_track_id`,
/// local_plex.rs:37-42).
///
/// Split out of `claim` so the predicate is testable without touching
/// `plex_cache.db` (the rating-key ladder's case 2 queries it).
pub(crate) fn recognises(raw: &RawRef) -> bool {
    if raw.source == Some(SourceId::PLEX) {
        return true;
    }
    // AN EXPLICIT, DIFFERENT SOURCE WORD IS EVIDENCE AGAINST OWNERSHIP — see
    // the long note in sources/subsonic.rs. A namespace bit must never
    // outvote a caller who already said which source this is.
    if raw.source.is_some() {
        return false;
    }

    if raw.id.trim().starts_with("plex:") {
        return true;
    }

    // A namespaced numeric is a TRACK rowid by construction. This arm is the
    // LOOSEST of the three (a bare bit test, no payload bound), so an album
    // barcode with bit 40 set used to land in `claim`'s Album branch and come
    // back as `Err(BadIdShape)` — which registry.rs propagates IMMEDIATELY,
    // ahead of any tiebreak. That failed the caller with a confusing *Plex*
    // error on an id Plex never owned.
    if raw.kind == Some(ItemKind::Album) {
        return false;
    }
    raw.numeric()
        .map(|n| n & PLEX_TRACK_ID_FLOOR != 0)
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl Source for PlexSource {
    fn id(&self) -> SourceId {
        SourceId::PLEX
    }

    fn claim(&self, raw: &RawRef) -> Option<Result<MediaRef, SourceError>> {
        if !recognises(raw) {
            return None;
        }
        let id = raw.id.trim();
        let prefixed = id.starts_with("plex:");

        let kind = raw.kind.unwrap_or(if prefixed {
            ItemKind::Album
        } else {
            ItemKind::Track
        });

        Some(match kind {
            ItemKind::Album => {
                if id.starts_with("plex:album:") {
                    // survey IC-2, second face: `local_plex.rs:283-291` derives
                    // the per-EDITION key while `plex_cache_get_album_tracks`
                    // (qbz-plex/lib.rs:1402-1410) only queries `album_key =
                    // plex:<hash>`. `local_albums.rs:226-228` forwards it
                    // verbatim and the album page comes back empty. Say so.
                    Err(SourceError::BadIdShape {
                        by: SourceId::PLEX,
                        id: id.to_string(),
                        why: "plex:album:<parent rating key> is a per-edition key; the cache is keyed by the content-hash album key",
                    })
                } else if prefixed {
                    Ok(MediaRef::new(SourceId::PLEX, ItemKind::Album, id))
                } else {
                    Err(SourceError::BadIdShape {
                        by: SourceId::PLEX,
                        id: id.to_string(),
                        why: "a plex album id is the content-hash key plex:<hash>",
                    })
                }
            }
            ItemKind::Track => {
                if prefixed {
                    // BUG 2, at the moment of the mistake instead of two layers
                    // down as a 404 (`GET /library/metadata/plex:<hash>`).
                    Err(SourceError::BadIdShape {
                        by: SourceId::PLEX,
                        id: id.to_string(),
                        why: "plex:<hash> is an album boundary key, not a rating key",
                    })
                } else {
                    Self::rating_key(raw)
                        .map(|key| MediaRef::new(SourceId::PLEX, ItemKind::Track, key))
                }
            }
            other => Err(SourceError::Unsupported {
                by: SourceId::PLEX,
                kind: other,
            }),
        })
    }

    async fn tracks(&self, item: &MediaRef) -> Result<Vec<QueueTrack>, SourceError> {
        // Cache reads only — no `.await` in this body.
        let rows = Self::cached_rows(item);
        if rows.is_empty() {
            return Err(Self::not_found(item));
        }
        self.memoize_art(item, &rows);
        // ONE mapper for both entry points. `plex_queue_track` is chosen over
        // `map_cached_to_local_track` + `local_queue_track`
        // (local_plex.rs:276-302 → local_playback.rs:31-108) because it emits
        // the RAW rating key as `QueueTrack.id`. That is survey requirement 2:
        // the same Plex track now carries ONE queue id regardless of which view
        // built the queue, instead of the two-way split in §6.2.
        Ok(rows.iter().map(plex_queue_track).collect())
    }

    async fn meta(&self, item: &MediaRef) -> Result<ItemMeta, SourceError> {
        let rows = Self::cached_rows(item);
        let Some(first) = rows.first() else {
            return Err(Self::not_found(item));
        };
        self.memoize_art(item, &rows);
        Ok(ItemMeta {
            title: if item.kind() == ItemKind::Track {
                first.title.clone()
            } else {
                first.album.clone()
            },
            subtitle: first.artist.clone(),
            year: None,
            track_count: Some(rows.len() as u32),
            duration_secs: Some(rows.iter().map(|t| t.duration_secs).sum()),
            quality: QualityHint::from_hz(
                first.bit_depth,
                first.sample_rate as f64,
                // The cache stores a container name; the tier only needs to
                // know whether it is MP3 (local_rows.rs:181-183).
                if first.format.eq_ignore_ascii_case("mp3") {
                    "MP3"
                } else {
                    "FLAC"
                },
            ),
            art: self.artwork(item, ArtSize::Card),
            badge: SourceBadge::Plex,
            kind_label: item.kind().label(),
        })
    }

    fn artwork(&self, item: &MediaRef, size: ArtSize) -> ArtRef {
        // Moved from `local_plex::thumb_url` (local_plex.rs:243-252) + the
        // Plex arm of `artwork_qt::classify` (artwork_qt.rs:167-176).
        let path = {
            let memo = self.art.read().ok().and_then(|m| m.get(item.id()).cloned());
            match memo {
                Some(p) => Some(p),
                None => {
                    let rows = Self::cached_rows(item);
                    self.memoize_art(item, &rows);
                    Self::preferred_art(item, &rows)
                }
            }
        };
        let Some(path) = path.filter(|p| is_thumb_path(p)) else {
            return ArtRef::None;
        };
        let Some((base, token)) = self.server() else {
            // Distinct from `None` so the miss is logged for what it is —
            // `ArtUrl::PlexUnconfigured`'s reason for existing.
            return ArtRef::Unavailable("plex is not connected");
        };
        ArtRef::Fetch {
            url: qbz_models::plex_thumb_url(&base, &token, &path, size.plex_px()),
            // The RAW `/library/...` path is the stable memo key; the tokenized
            // url is rebuilt every pass (artwork_qt.rs:231-238).
            cache_key: path,
        }
    }

    fn artwork_token(&self, token: &str, size: ArtSize) -> ArtRef {
        // The Plex arm of `artwork_qt::classify` (artwork_qt.rs:168-178), now
        // owned by the source that knows what a `/library/...` path is. The
        // caller no longer has to have heard of `is_thumb_path`.
        let token = token.trim();
        if token.is_empty() || !is_thumb_path(token) {
            return ArtRef::None;
        }
        let Some((base, token_secret)) = self.server() else {
            // Distinct from `None` so the miss is logged for what it is —
            // `ArtUrl::PlexUnconfigured`'s reason for existing.
            return ArtRef::Unavailable("plex is not connected");
        };
        ArtRef::Fetch {
            url: qbz_models::plex_thumb_url(&base, &token_secret, token, size.plex_px()),
            // The RAW `/library/...` path is the stable memo key; the tokenized
            // url is rebuilt every pass (artwork_qt.rs:231-238).
            cache_key: token.to_string(),
        }
    }

    async fn playback(
        &self,
        item: &MediaRef,
        track: &QueueTrack,
    ) -> Result<PlaybackTicket, SourceError> {
        // Moved from `local_playback::play_plex_track`, minus the player calls
        // (`play_data`, and the feeder handoff), which STAY in qbz-qt. `item
        // .id()` IS the rating key by construction — this function does not
        // re-derive anything, which is why no caller can smuggle a
        // `plex:<hash>` in here.
        if item.kind() != ItemKind::Track {
            return Err(SourceError::Unsupported {
                by: SourceId::PLEX,
                kind: item.kind(),
            });
        }
        let (base, token) = self.server().ok_or(SourceError::NotConfigured {
            by: SourceId::PLEX,
            why: "no Plex credentials configured",
        })?;
        let rating_key = item.id().to_string();
        // `plex_resolve_part_url`, NOT `plex_resolve_track_media`: the ticket
        // resolves a LOCATION, it does not fetch a body. The frontend
        // Range-streams it (~1s to first audio) and falls back to a plain GET
        // of this same url when the feeder cannot start.
        //
        // Dropping the old `resolve_track_media` fallback is not a behaviour
        // loss: the two functions run the SAME metadata request, the SAME
        // parse and the SAME part-key extraction, and only then does
        // `resolve_track_media` also GET the body. Every way the first can
        // fail, the second fails identically — so the fallback was a second
        // round trip that could only reproduce the first one's error.
        //
        // The part url is a DIRECT-PLAY url: the original on-disk bytes,
        // bit-perfect. No transcode is requested here, and none may be.
        match qbz_plex::plex_resolve_part_url(base, token, rating_key).await {
            Ok(loc) => Ok(PlaybackTicket::Stream {
                url: loc.part_url,
                play_id: track.id,
                // From the QUEUE row rather than the core's "current track":
                // the caller may be priming a track that is not current yet
                // (play-next, an album's first row), where the core would
                // answer about the wrong one. 0 is acceptable — it only makes
                // the feeder's estimate conservative.
                duration_secs: track.duration_secs,
                start_secs: 0,
                log_tag: "PLEX",
                request_headers: loc.request_headers,
            }),
            Err(e) => Err(SourceError::Backend {
                by: SourceId::PLEX,
                msg: e,
            }),
        }
    }

    fn bind_user(&self, _uid: u64, _dir: &std::path::Path) {
        // The creds store is re-bound by the frontend and re-published through
        // `set_creds`; the art memo belongs to the previous account's server.
        if let Ok(mut m) = self.art.write() {
            m.clear();
        }
    }

    fn teardown(&self) {
        self.set_creds(None);
        if let Ok(mut m) = self.art.write() {
            m.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::SourceBadge;

    fn cached_art_row(item_art: Option<&str>, collection_art: Option<&str>) -> PlexCachedTrack {
        PlexCachedTrack {
            id: 1,
            rating_key: "1".into(),
            title: "Track".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            duration_secs: 1,
            format: "flac".into(),
            bit_depth: Some(16),
            sample_rate: 44_100,
            artwork_path: item_art.map(str::to_string),
            collection_artwork_path: collection_art.map(str::to_string),
            source: "plex".into(),
            album_key: "plex:album".into(),
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2026),
            parent_rating_key: Some("album".into()),
        }
    }

    #[test]
    fn queue_mapper_preserves_plex_identity_and_quality() {
        let mut row = cached_art_row(Some("/disc/thumb"), Some("/collection/thumb"));
        row.id = 99;
        row.rating_key = "44440".into();
        row.bit_depth = Some(24);
        row.sample_rate = 192_000;

        let queue = plex_queue_track(&row);

        assert_eq!(queue.id, 44_440);
        assert_eq!(queue.album_id.as_deref(), Some("plex:album"));
        assert_eq!(queue.source.as_deref(), Some("plex"));
        assert_eq!(queue.artwork_url.as_deref(), Some("/disc/thumb"));
        assert_eq!(queue.bit_depth, Some(24));
        assert_eq!(queue.sample_rate, Some(192.0));
        assert!(queue.hires);
        assert!(queue.is_local);
        assert!(queue.streamable);
    }

    #[test]
    fn track_art_prefers_the_disc_then_falls_back_to_the_collection() {
        let item = MediaRef::new(SourceId::PLEX, ItemKind::Track, "1");
        let row = cached_art_row(Some("/disc/thumb"), Some("/collection/thumb"));
        assert_eq!(
            PlexSource::preferred_art(&item, std::slice::from_ref(&row)).as_deref(),
            Some("/disc/thumb")
        );
        let row = cached_art_row(None, Some("/collection/thumb"));
        assert_eq!(
            PlexSource::preferred_art(&item, &[row]).as_deref(),
            Some("/collection/thumb")
        );
    }

    #[test]
    fn album_art_prefers_the_collection_then_falls_back_to_an_item() {
        let item = MediaRef::new(SourceId::PLEX, ItemKind::Album, "plex:album");
        let row = cached_art_row(Some("/disc/thumb"), Some("/collection/thumb"));
        assert_eq!(
            PlexSource::preferred_art(&item, std::slice::from_ref(&row)).as_deref(),
            Some("/collection/thumb")
        );
        let row = cached_art_row(Some("/disc/thumb"), None);
        assert_eq!(
            PlexSource::preferred_art(&item, &[row]).as_deref(),
            Some("/disc/thumb")
        );
    }

    /// The stub stands in for the Plex cache, so the pure ladder is testable
    /// with no `plex_cache.db` on disk.
    fn no_cache(_masked: u64) -> Option<String> {
        None
    }

    fn track_ref(id: &str, hint: Option<&str>) -> RawRef {
        RawRef {
            source: Some(SourceId::PLEX),
            badge: SourceBadge::Plex,
            kind: Some(ItemKind::Track),
            id: id.into(),
            is_local: Some(true),
            hint: hint.map(String::from),
        }
    }

    // ── The three cases local_playback.rs:410-444 already pinned ────────────

    /// LocalLibrary path: the hint IS the raw rating key (`local_queue_track`
    /// stamps `file_path`, which for a Plex row is the server key).
    #[test]
    fn raw_hint_is_used_verbatim() {
        assert_eq!(
            PlexSource::rating_key_with(&track_ref("999", Some("12345")), no_cache).unwrap(),
            "12345"
        );
        // Non-numeric server keys are legal and must survive untouched.
        assert_eq!(
            PlexSource::rating_key_with(&track_ref("999", Some("/library/metadata/771")), no_cache)
                .unwrap(),
            "/library/metadata/771"
        );
    }

    /// BUG 2: the MyQBZ collections path stamps the ALBUM boundary key
    /// (`qbz_plex::plex_album_key` → `plex:<hash>`). It is
    /// not a rating key, so it is IGNORED in favour of the numeric queue id.
    #[test]
    fn plex_prefixed_hint_is_normalised_away_not_passed_through() {
        assert_eq!(
            PlexSource::rating_key_with(&track_ref("771", Some("plex:deadbeef")), no_cache)
                .unwrap(),
            "771"
        );
        // Real album key from the on-disk DB (survey §6.1).
        assert_eq!(
            PlexSource::rating_key_with(
                &track_ref("44440", Some("plex:5677211365378243606")),
                no_cache
            )
            .unwrap(),
            "44440"
        );
        // Prefix test is on the LEADING bytes only, exactly like `starts_with`.
        assert_eq!(
            PlexSource::rating_key_with(&track_ref("42", Some("plex:")), no_cache).unwrap(),
            "42"
        );
        assert_eq!(
            PlexSource::rating_key_with(&track_ref("42", Some("77plex:1")), no_cache).unwrap(),
            "77plex:1"
        );
    }

    /// A missing hint falls back to the bare numeric id (the MyQBZ shape).
    #[test]
    fn missing_hint_falls_back_to_queue_id() {
        assert_eq!(
            PlexSource::rating_key_with(&track_ref("771", None), no_cache).unwrap(),
            "771"
        );
    }

    // ── New coverage the old tests never had ────────────────────────────────

    /// Case 2: the LocalLibrary namespaced id is RESOLVED, never stringified.
    ///
    /// The pair is DERIVED from the floor rather than transcribed. The survey's
    /// id table asserted that `1099511671736` was rating key `44440`; it is
    /// not — `PLEX_TRACK_ID_FLOOR` is `1 << 40` = 1_099_511_627_776, so that id
    /// unmasks to 43_960 and 44_440 namespaces to 1_099_511_672_216. Hardcoding
    /// a transcribed pair is how the wrong one got here in the first place, so
    /// the test computes it.
    #[test]
    fn namespaced_id_is_resolved_through_the_cache() {
        const FLOOR: u64 = 1 << 40;
        let rating_key: u64 = 44_440;
        let namespaced = (rating_key + FLOOR).to_string();
        let raw = track_ref(&namespaced, None);
        let resolved =
            PlexSource::rating_key_with(&raw, |masked| Some(format!("rk-{masked}"))).unwrap();
        assert_eq!(resolved, format!("rk-{rating_key}"));
        // And a cache miss is a named NotFound, not a silent wrong key.
        let err = PlexSource::rating_key_with(&raw, no_cache).unwrap_err();
        assert!(matches!(
            err,
            SourceError::NotFound {
                by: SourceId::PLEX,
                kind: ItemKind::Track,
                ..
            }
        ));
    }

    /// Case 4: a non-numeric rating key with no hint. `playback_track_id`
    /// (qbz-plex/lib.rs:737-741) hashes those, so today it 404s silently.
    #[test]
    fn non_numeric_id_without_a_hint_is_a_named_error() {
        let err = PlexSource::rating_key_with(&track_ref("abc-123", None), no_cache).unwrap_err();
        assert!(matches!(
            err,
            SourceError::BadIdShape {
                by: SourceId::PLEX,
                ..
            }
        ));
    }

    // ── claim ───────────────────────────────────────────────────────────────

    #[test]
    fn album_boundary_key_is_rejected_in_track_position() {
        // The shape bug 2 is named after, arriving as the ID rather than as a
        // hint (a mis-stored mixtape Track item).
        let raw = RawRef {
            kind: Some(ItemKind::Track),
            id: "plex:5677211365378243606".into(),
            is_local: Some(true),
            ..Default::default()
        };
        let err = PlexSource::new().claim(&raw).unwrap().unwrap_err();
        match err {
            SourceError::BadIdShape { by, why, .. } => {
                assert_eq!(by, SourceId::PLEX);
                assert!(why.contains("album boundary key"));
            }
            other => panic!("expected BadIdShape, got {other:?}"),
        }
    }

    #[test]
    fn album_boundary_key_is_owned_in_album_position() {
        let raw = RawRef {
            kind: Some(ItemKind::Album),
            id: "plex:5677211365378243606".into(),
            is_local: Some(true),
            ..Default::default()
        };
        let m = PlexSource::new().claim(&raw).unwrap().unwrap();
        assert_eq!(m.source(), SourceId::PLEX);
        assert_eq!(m.kind(), ItemKind::Album);
        assert_eq!(m.id(), "plex:5677211365378243606");
    }

    #[test]
    fn per_edition_key_is_rejected_instead_of_returning_nothing() {
        // survey IC-2, second face — real value `plex:album:44439`.
        let raw = RawRef {
            kind: Some(ItemKind::Album),
            id: "plex:album:44439".into(),
            is_local: Some(true),
            ..Default::default()
        };
        let err = PlexSource::new().claim(&raw).unwrap().unwrap_err();
        assert!(matches!(
            err,
            SourceError::BadIdShape {
                by: SourceId::PLEX,
                ..
            }
        ));
    }

    #[test]
    fn does_not_claim_what_it_does_not_own() {
        let plex = PlexSource::new();
        // A local group key.
        assert!(plex
            .claim(&RawRef {
                kind: Some(ItemKind::Album),
                id: "HIT ME HARD AND SOFT|Billie Eilish".into(),
                is_local: Some(true),
                ..Default::default()
            })
            .is_none());
        // A local row id (real row, survey §6.1) — no source word, no bit.
        assert!(plex
            .claim(&RawRef {
                kind: Some(ItemKind::Track),
                id: "2954".into(),
                is_local: Some(true),
                ..Default::default()
            })
            .is_none());
        // A Qobuz album id.
        assert!(plex
            .claim(&RawRef::new("qobuz", ItemKind::Album, "0060254702523"))
            .is_none());
    }

    #[test]
    fn namespace_bit_is_enough_to_recognise_without_a_source_word() {
        // survey IC-3: a mixtape Plex TRACK item carries the namespaced id as a
        // STRING and no usable source word. Plex must still recognise it, or it
        // falls through to LocalSource and `db.get_track` misses.
        //
        // The predicate is asserted rather than `claim`, because claiming a
        // namespaced id runs the cache resolve (case 2) and a unit test must
        // not touch `plex_cache.db`.
        assert!(recognises(&RawRef {
            kind: Some(ItemKind::Track),
            id: "1099511671736".into(),
            is_local: Some(true),
            ..Default::default()
        }));
        assert!(!recognises(&RawRef {
            kind: Some(ItemKind::Track),
            id: "2954".into(),
            is_local: Some(true),
            ..Default::default()
        }));
        // An ephemeral id (2^48) has bit 40 CLEAR, so it is never mistaken for
        // a Plex row (survey §6.3).
        assert!(!recognises(&RawRef {
            kind: Some(ItemKind::Track),
            id: "281474976710656".into(),
            ..Default::default()
        }));
    }

    // ── artwork_token: the Plex arm of the dead `classify` ──────────────────

    /// With no credentials a Plex thumb is UNAVAILABLE, never `None`: the art
    /// exists, the server does not answer yet, and the two must stay
    /// distinguishable or the miss is logged as a dead download.
    #[test]
    fn a_thumb_path_without_credentials_is_unavailable_not_none() {
        let p = PlexSource::new();
        assert!(matches!(
            p.artwork_token("/library/metadata/42/thumb/1", ArtSize::Card),
            ArtRef::Unavailable(_)
        ));
    }

    /// Anything that is not one of the two declared thumb prefixes is not
    /// Plex's to interpret — including an absolute path, which `classify`
    /// would have called a LOCAL FILE purely because it starts with `/`.
    #[test]
    fn only_the_declared_thumb_prefixes_are_claimed() {
        let p = PlexSource::new();
        assert!(matches!(p.artwork_token("", ArtSize::Card), ArtRef::None));
        assert!(matches!(
            p.artwork_token("/home/u/Music/Album/cover.jpg", ArtSize::Card),
            ArtRef::None
        ));
        assert!(matches!(
            p.artwork_token("https://static.qobuz.com/a.jpg", ArtSize::Card),
            ArtRef::None
        ));
        // Both prefixes the taxonomy declares.
        assert!(is_thumb_path("/library/metadata/42/thumb/1"));
        assert!(is_thumb_path("/photo/:/transcode"));
    }
}
