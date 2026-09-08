//! Files on this machine: the indexed `library.db`, Qobuz downloads/purchases
//! and the session-scoped ephemeral folder.
//!
//! `local_ephemeral` is a *fourth flavour of local*, not a fourth source:
//! session-scoped rather than user-scoped, floor-namespaced ids
//! (`local_ephemeral.rs:64-65` over `qbz_library::ephemeral::
//! EPHEMERAL_ID_FLOOR`). It is an arm inside this source, reached through the
//! injected [`EphemeralTracks`] handle.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use qbz_library::album_grouping::AlbumGroupMode;
use qbz_library::ephemeral::EPHEMERAL_ID_FLOOR;
use qbz_library::{LibraryDatabase, LibraryError, LocalTrack};
use qbz_models::QueueTrack;

use crate::art::{ArtRef, ArtSize};
use crate::error::SourceError;
use crate::id::{ItemKind, MediaRef, RawRef, SourceId};
use crate::meta::{ItemMeta, QualityHint, SourceBadge};
use crate::playback::PlaybackTicket;
use crate::source::Source;
use crate::sources::pool::DbPool;

/// Memo ceiling, mirroring `local_artwork::MEMO_CAP` (local_artwork.rs:66) and
/// `artwork_qt::MEMO_CAP` (artwork_qt.rs:82): an accelerator, cleared wholesale
/// rather than carrying LRU bookkeeping.
const MEMO_CAP: usize = 8192;

/// Per-source generation locks, sharded. Moved from `local_artwork::GEN_SHARDS`
/// (local_artwork.rs:76-78).
const GEN_SHARDS: usize = 64;

/// The session-scoped ephemeral store, injected by the frontend.
///
/// The store itself is a process-global in the frontend
/// (`local_ephemeral.rs:59`, a `LazyLock<EphemeralLibraryState>`); opening a
/// second copy here would be a second authority. Same injection shape as
/// [`crate::PlexCreds`], for the same reason.
pub trait EphemeralTracks: Send + Sync + 'static {
    /// Resolve a synthetic id to its cached row (`local_ephemeral::get_track`,
    /// local_ephemeral.rs:70-72).
    fn get_track(&self, id: i64) -> Option<LocalTrack>;
    /// The tracks of one ephemeral album block, in scan order
    /// (`local_ephemeral::album_tracks`, local_ephemeral.rs:95-101).
    fn album_tracks(&self, group_key: &str) -> Vec<LocalTrack>;
}

/// Map a `LocalTrack` to a `QueueTrack`.
///
/// Moved VERBATIM from `local_playback::local_queue_track`
/// (local_playback.rs:31-108), which is the most complete of the three mappers
/// in the tree (`qbz-mixtape/enqueue.rs:513-553` drops the offline/Plex
/// branches). Only one line changed: `crate::local_plex::album_key_for` became
/// `qbz_plex::plex_album_key`, which is what that helper forwards to
/// (local_plex.rs:326-328).
///
/// The `is_plex` branches are retained rather than deleted: a Plex row reaches
/// this shape through `local_plex::map_cached_to_local_track`
/// (local_plex.rs:276-302), and the LocalLibrary Tracks tab merges those rows
/// into the same list it maps here.
pub fn local_queue_track(t: &LocalTrack) -> QueueTrack {
    let src = match t.source.as_deref() {
        Some("qobuz_download") => "qobuz_download",
        Some("plex") => "plex",
        _ => "local",
    };
    let is_offline = src == "qobuz_download";
    let is_plex = src == "plex";
    // A Plex row carries a RAW server-relative thumb path; it must stay raw
    // so the now-playing bar / queue resolve it from current creds.
    // `file://`-prefixing it poisons it into a local-read miss.
    let artwork_url = t.artwork_path.as_ref().map(|p| {
        if is_plex {
            p.clone()
        } else {
            // Idempotent, and drive/UNC-aware: format!("file://{p}") turned
            // C:/... into a URL whose HOST was the drive letter.
            qbz_models::fs_url::file_url(p)
        }
    });
    let sample_rate_khz = if t.sample_rate >= 1000.0 {
        t.sample_rate / 1000.0
    } else {
        t.sample_rate
    };
    QueueTrack {
        id: if is_offline {
            t.qobuz_track_id.unwrap_or(t.id) as u64
        } else {
            t.id as u64
        },
        title: t.title.clone(),
        version: None,
        artist: t.artist.clone(),
        album: t.album_group_title.clone(),
        album_version: None,
        duration_secs: t.duration_secs,
        artwork_url,
        hires: t.bit_depth.map(|d| d > 16).unwrap_or(false),
        bit_depth: t.bit_depth,
        sample_rate: Some(sample_rate_khz),
        is_local: true,
        // Navigation key. For Plex the track's `album_group_key` is the
        // per-edition split key, which the album cache is NOT keyed by —
        // recover the content-hash key so "go to album" resolves.
        album_id: Some(if is_plex {
            qbz_plex::plex_album_key(&t.artist, &t.album)
        } else {
            t.album_group_key.clone()
        }),
        artist_id: None,
        streamable: true,
        source: Some(src.to_string()),
        parental_warning: false,
        // Plex: the string rating_key the resolve needs (the numeric queue id
        // is a namespaced form). Offline: the local row id.
        source_item_id_hint: if is_plex || is_offline {
            Some(if is_plex {
                t.file_path.clone()
            } else {
                t.id.to_string()
            })
        } else {
            None
        },
        // NO container origin — `playback_qt::derive_context` owns it
        // (local_playback.rs:94-104 explains at length why these stay None).
        context_kind: None,
        context_id: None,
        // Identity straight from the scanned tags (None on untagged files).
        isrc: t.isrc.clone(),
        recording_mbid: t.musicbrainz_recording_id.clone(),
    }
}

/// The local-files source.
pub struct LocalSource {
    pool: DbPool,
    ephemeral: RwLock<Option<Arc<dyn EphemeralTracks>>>,
    /// The ACTIVE album identity mode (`local_state::group_mode`, a persisted
    /// UI pref). Folder is the owner default
    /// (`qbz-library/src/album_grouping.rs:24-33`).
    mode: Mutex<AlbumGroupMode>,
    /// Cover source path → its generated thumbnail. Moved from
    /// `local_artwork::THUMBS` (local_artwork.rs:81-82).
    thumbs: RwLock<HashMap<String, PathBuf>>,
    /// Item id → its cover SOURCE path, so a repeat `artwork` call costs no
    /// query at all.
    covers: RwLock<HashMap<String, String>>,
    /// Per-source generation locks. Moved from `local_artwork::GEN_LOCKS`
    /// (local_artwork.rs:84-85): `qbz_library::generate_thumbnail` writes the
    /// jpeg straight to its final path (no temp+rename), so two workers must
    /// never generate the SAME source concurrently.
    gen_locks: Vec<Mutex<()>>,
}

impl LocalSource {
    /// A source with no user bound and no ephemeral store.
    pub fn new() -> Self {
        Self {
            pool: DbPool::new(),
            ephemeral: RwLock::new(None),
            mode: Mutex::new(AlbumGroupMode::Folder),
            thumbs: RwLock::new(HashMap::new()),
            covers: RwLock::new(HashMap::new()),
            gen_locks: (0..GEN_SHARDS).map(|_| Mutex::new(())).collect(),
        }
    }

    /// THE `library.db` read accessor.
    ///
    /// `local_state::with_db` (local_state.rs:39-61) forwards here with its
    /// signature unchanged, so its 48 call sites are untouched and only the
    /// number of `LibraryDatabase::open` calls changes — which is bug 1.
    pub fn with<R>(
        &self,
        f: impl FnOnce(&LibraryDatabase) -> Result<R, LibraryError>,
    ) -> Option<R> {
        self.pool.with(f)
    }

    /// THE create-capable accessor. `library_db_qt::with_db(true, …)`
    /// (library_db_qt.rs:50-76) forwards here.
    pub fn with_creating<R>(
        &self,
        f: impl FnOnce(&LibraryDatabase) -> Result<R, LibraryError>,
    ) -> Option<R> {
        self.pool.with_creating(f)
    }

    /// Install the unbound-window path computation. See [`DbPool`]'s
    /// `fallback` field: the frontend passes `local_state::db_path`, because
    /// `UserDataPaths` lives in `qbz-app` and this crate must not link it.
    pub fn set_unbound_fallback(
        &self,
        f: Option<Box<dyn Fn() -> Option<PathBuf> + Send + Sync>>,
    ) {
        self.pool.set_unbound_fallback(f);
    }

    /// Publish the session-scoped ephemeral store.
    pub fn set_ephemeral(&self, store: Option<Arc<dyn EphemeralTracks>>) {
        if let Ok(mut slot) = self.ephemeral.write() {
            *slot = store;
        }
    }

    /// Set the ACTIVE album identity mode (the header dropdown / Settings
    /// pref that `local_state::group_mode` reads).
    pub fn set_album_group_mode(&self, mode: AlbumGroupMode) {
        if let Ok(mut m) = self.mode.lock() {
            *m = mode;
        }
    }

    /// Is the frontend's ephemeral session store published?
    ///
    /// For the boot-time wiring report only (`source_wiring::report_wiring`):
    /// an absent store makes every ephemeral row resolve as "not found", which
    /// is indistinguishable from an empty session unless somebody asks.
    pub fn has_ephemeral_store(&self) -> bool {
        self.ephemeral.read().map(|e| e.is_some()).unwrap_or(false)
    }

    fn ephemeral(&self) -> Option<Arc<dyn EphemeralTracks>> {
        self.ephemeral.read().ok().and_then(|s| s.clone())
    }

    fn group_mode(&self) -> AlbumGroupMode {
        self.mode
            .lock()
            .map(|m| *m)
            .unwrap_or(AlbumGroupMode::Folder)
    }

    /// True when `id` looks like a local album/folder GROUP KEY.
    ///
    /// The three shapes, from survey §6.1: an absolute directory path (folder
    /// mode), `<album>|<album_artist>` (metadata mode, and the ephemeral
    /// `<album>|||<album_artist>` form), and the literal unknown bucket
    /// (`library_qt.rs:1053`).
    fn is_group_key(id: &str) -> bool {
        qbz_models::fs_url::is_local_abs_path(id) || id.contains('|') || id == "__unknown_album__"
    }

    /// True when `id` is a Plex namespaced row id (`local_plex.rs:37-42`).
    fn is_plex_namespaced(raw: &RawRef) -> bool {
        raw.numeric()
            .map(|n| n & crate::sources::plex::PLEX_TRACK_ID_FLOOR != 0)
            .unwrap_or(false)
    }

    /// Is this a session-scoped ephemeral id? (`local_ephemeral::
    /// is_ephemeral_id`, local_ephemeral.rs:64-66.)
    fn is_ephemeral(id: i64) -> bool {
        id >= EPHEMERAL_ID_FLOOR
    }

    /// The `library.db` row id (or ephemeral id) a track reference names.
    ///
    /// Decided on EVIDENCE, in order — the same discipline
    /// [`crate::PlexSource`] applies to rating keys:
    ///
    /// 1. an ephemeral-floor numeric id → itself (the session store owns it);
    /// 2. an OFFLINE row (`badge == Offline`) with a numeric hint → the hint.
    ///    `local_queue_track` puts the Qobuz track id in `QueueTrack.id` and
    ///    the local row id in `source_item_id_hint` for exactly these rows
    ///    (local_playback.rs:55-59, :85-90); today `play_local_file(runtime,
    ///    track.id)` (local_playback.rs:243) looks the row up by the *Qobuz*
    ///    id and misses;
    /// 3. any other numeric id → itself;
    /// 4. otherwise → [`SourceError::BadIdShape`].
    fn row_id(raw: &RawRef) -> Result<i64, SourceError> {
        if let Some(n) = raw.numeric() {
            if Self::is_ephemeral(n as i64) {
                return Ok(n as i64);
            }
        }
        if raw.badge == SourceBadge::Offline {
            if let Some(h) = raw.hint_str().and_then(|h| h.parse::<i64>().ok()) {
                return Ok(h);
            }
        }
        if let Some(n) = raw.numeric() {
            return Ok(n as i64);
        }
        Err(SourceError::BadIdShape {
            by: SourceId::LOCAL,
            id: raw.id.clone(),
            why: "a local track id is a numeric library.db row id (or an ephemeral id >= 2^48)",
        })
    }

    /// The tracks of one album group key.
    ///
    /// Moved from `local_albums::fetch_album_tracks_blocking`
    /// (local_albums.rs:225-241) MINUS its `plex:` fork — that fork dissolves
    /// into `claim`, and a `plex:` key never reaches this source.
    fn album_tracks(&self, group_key: &str) -> Vec<LocalTrack> {
        if let Some(eph) = self.ephemeral() {
            let rows = eph.album_tracks(group_key);
            if !rows.is_empty() {
                return rows;
            }
        }
        let mode = self.group_mode();
        self.with(|db| match mode {
            AlbumGroupMode::Metadata => {
                let meta = db.get_album_tracks_metadata(group_key)?;
                if meta.is_empty() {
                    db.get_album_tracks(group_key)
                } else {
                    Ok(meta)
                }
            }
            AlbumGroupMode::Folder => db.get_album_tracks(group_key),
        })
        .unwrap_or_default()
    }

    /// One row by id: the ephemeral store for a floor id, else `library.db`.
    fn track_row(&self, row_id: i64) -> Option<LocalTrack> {
        if Self::is_ephemeral(row_id) {
            return self.ephemeral().and_then(|e| e.get_track(row_id));
        }
        self.with(|db| db.get_track(row_id)).flatten()
    }

    fn not_found(&self, item: &MediaRef) -> SourceError {
        SourceError::NotFound {
            by: SourceId::LOCAL,
            kind: item.kind(),
            id: item.id().to_string(),
        }
    }

    // ── Artwork (moved from local_artwork.rs) ────────────────────────────────

    fn gen_shard(src: &str) -> usize {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        src.hash(&mut h);
        (h.finish() as usize) % GEN_SHARDS
    }

    fn memoize(&self, src: &str, resolved: &Path) {
        if let Ok(mut memo) = self.thumbs.write() {
            if memo.len() >= MEMO_CAP {
                memo.clear();
            }
            memo.insert(src.to_string(), resolved.to_path_buf());
        }
    }

    /// The thumbnail for a local cover IF it costs nothing: the memo, else one
    /// `stat` on the thumbnail path (which is where a previous run's
    /// thumbnails live). `None` means "must be generated" — the caller defers
    /// it to [`Source::thumbnail`].
    ///
    /// Moved from `local_artwork::cached_thumbnail` (local_artwork.rs:190-217).
    fn cached_thumbnail(&self, src: &str) -> Option<PathBuf> {
        // A local cover is very often ALREADY the thumbnail the scanner
        // generated. Re-hashing its path yields a DIFFERENT filename, so the
        // cold phase would decode it, Lanczos-resize it to its own size (a null
        // scale) and re-encode a byte-identical-looking duplicate — that is
        // most of the wait before the first cover appears.
        if let Ok(dir) = qbz_library::get_thumbnails_dir() {
            let p = Path::new(src);
            if p.starts_with(&dir) && p.is_file() {
                return Some(p.to_owned());
            }
        }
        if let Ok(memo) = self.thumbs.read() {
            if let Some(hit) = memo.get(src) {
                if hit.is_file() {
                    return Some(hit.clone());
                }
            }
        }
        let thumb = qbz_library::get_thumbnail_path(Path::new(src)).ok()?;
        if !thumb.is_file() {
            return None;
        }
        self.memoize(src, &thumb);
        Some(thumb)
    }

    /// The thumbnail for a local cover, GENERATING it if needed. Falls back to
    /// the original file when no thumbnail can be produced (unsupported source)
    /// so the card is not blank; `None` only when the cover itself is gone.
    ///
    /// Moved from `local_artwork::local_thumbnail` (local_artwork.rs:219-244),
    /// per-source generation lock included.
    fn local_thumbnail(&self, src: &str) -> Option<PathBuf> {
        if let Some(hit) = self.cached_thumbnail(src) {
            return Some(hit);
        }
        let path = Path::new(src);
        if !path.is_file() {
            return None;
        }
        // Serialize same-source generation: the loser of the race re-checks the
        // memo and returns without decoding anything.
        let guard = self.gen_locks[Self::gen_shard(src)]
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = self.cached_thumbnail(src) {
            drop(guard);
            return Some(hit);
        }
        let resolved =
            qbz_library::get_or_generate_thumbnail(path).unwrap_or_else(|_| path.to_owned());
        drop(guard);
        self.memoize(src, &resolved);
        Some(resolved)
    }

    /// The cover SOURCE path for an item, memoized. At most ONE indexed query
    /// on a cold miss.
    fn cover_source(&self, item: &MediaRef) -> Option<String> {
        if let Ok(memo) = self.covers.read() {
            if let Some(hit) = memo.get(item.id()) {
                return Some(hit.clone());
            }
        }
        let found = match item.kind() {
            ItemKind::Track => item
                .id()
                .parse::<i64>()
                .ok()
                .and_then(|id| self.track_row(id))
                .and_then(|t| t.artwork_path)
                .filter(|p| !p.is_empty()),
            ItemKind::Album | ItemKind::Folder => self
                .album_tracks(item.id())
                .iter()
                .find_map(|t| t.artwork_path.clone().filter(|p| !p.is_empty())),
            _ => None,
        }?;
        if let Ok(mut memo) = self.covers.write() {
            if memo.len() >= MEMO_CAP {
                memo.clear();
            }
            memo.insert(item.id().to_string(), found.clone());
        }
        Some(found)
    }

    /// Seed the cover memo from rows the caller ALREADY has, so the `artwork`
    /// call inside `meta_of_rows` costs nothing instead of re-querying.
    fn memoize_cover(&self, item: &MediaRef, rows: &[LocalTrack]) {
        let Some(path) = rows
            .iter()
            .find_map(|t| t.artwork_path.clone().filter(|p| !p.is_empty()))
        else {
            return;
        };
        if let Ok(mut memo) = self.covers.write() {
            if memo.len() >= MEMO_CAP {
                memo.clear();
            }
            memo.insert(item.id().to_string(), path);
        }
    }

    fn meta_of_rows(&self, item: &MediaRef, rows: &[LocalTrack]) -> ItemMeta {
        self.memoize_cover(item, rows);
        let first = &rows[0];
        let badge = SourceBadge::from_word(first.source.as_deref());
        ItemMeta {
            title: if item.kind() == ItemKind::Track {
                first.title.clone()
            } else {
                first.album_group_title.clone()
            },
            subtitle: first
                .album_artist
                .clone()
                .filter(|a| !a.is_empty())
                .unwrap_or_else(|| first.artist.clone()),
            year: first.year,
            track_count: Some(rows.len() as u32),
            duration_secs: Some(rows.iter().map(|t| t.duration_secs).sum()),
            quality: QualityHint::from_hz(
                first.bit_depth,
                first.sample_rate,
                format_name(&first.format),
            ),
            art: self.artwork(item, ArtSize::Card),
            badge,
            kind_label: item.kind().label(),
        }
    }
}

impl Default for LocalSource {
    fn default() -> Self {
        Self::new()
    }
}

/// `AudioFormat` as a `&'static str` for [`QualityHint`]. `Display` allocates;
/// the tier check only needs the name.
fn format_name(f: &qbz_library::AudioFormat) -> &'static str {
    use qbz_library::AudioFormat::*;
    match f {
        Flac => "FLAC",
        Alac => "ALAC",
        Wav => "WAV",
        Aiff => "AIFF",
        Ape => "APE",
        Mp3 => "MP3",
        Dsd => "DSD",
        Unknown => "",
    }
}

#[async_trait::async_trait]
impl Source for LocalSource {
    fn id(&self) -> SourceId {
        SourceId::LOCAL
    }

    fn claim(&self, raw: &RawRef) -> Option<Result<MediaRef, SourceError>> {
        // Two shapes another source PROVABLY owns. These are not "claim by
        // elimination" — they are Plex's declared id forms (survey §6.1), and
        // excluding them is what keeps the walk single-claim instead of
        // `Ambiguous`.
        if raw.id.trim_start().starts_with("plex:") || Self::is_plex_namespaced(raw) {
            return None;
        }

        let numeric = raw.numeric();
        let mine = raw.source == Some(SourceId::LOCAL)
            || Self::is_group_key(raw.id.trim())
            || numeric
                .map(|n| Self::is_ephemeral(n as i64))
                .unwrap_or(false)
            // A bare numeric with NO source word, that the caller knows is not
            // Qobuz. The mirror of QobuzSource's `is_local == Some(false)`
            // rule; `resolve_bulk_qobuz_track_ids` (myqbz_play_qt.rs:246-248)
            // already splits exactly this way.
            || (raw.source.is_none() && raw.is_local == Some(true) && numeric.is_some());
        if !mine {
            return None;
        }

        let id = raw.id.trim();
        if id.is_empty() {
            return Some(Err(SourceError::BadIdShape {
                by: SourceId::LOCAL,
                id: raw.id.clone(),
                why: "empty id",
            }));
        }

        let kind = raw.kind.unwrap_or({
            if Self::is_group_key(id) {
                ItemKind::Album
            } else {
                ItemKind::Track
            }
        });

        Some(match kind {
            // Group keys and folder paths are opaque and stay verbatim.
            ItemKind::Album | ItemKind::Folder => {
                Ok(MediaRef::new(SourceId::LOCAL, kind, id.to_string()))
            }
            ItemKind::Track => Self::row_id(raw)
                .map(|row| MediaRef::new(SourceId::LOCAL, ItemKind::Track, row.to_string())),
            // The library schema stores qobuz_playlist_id + local_track_id rows
            // but there is no unique local-only playlist id to resolve against.
            ItemKind::Playlist => Err(SourceError::Unsupported {
                by: SourceId::LOCAL,
                kind: ItemKind::Playlist,
            }),
            // `local_album_actions.rs:335` routes a local artist by NAME to a
            // view, never to a resolver.
            ItemKind::Artist => Err(SourceError::Unsupported {
                by: SourceId::LOCAL,
                kind: ItemKind::Artist,
            }),
        })
    }

    async fn tracks(&self, item: &MediaRef) -> Result<Vec<QueueTrack>, SourceError> {
        // No `.await` in this body: `&LibraryDatabase` is `!Send` and must never
        // cross one. The future is ready on first poll; the
        // caller keeps wrapping the call in `spawn_blocking` exactly where it
        // does today (local_playback.rs:302, :313, :323).
        let rows: Vec<LocalTrack> = match item.kind() {
            // Provider-owned replacement for the legacy mixtape resolver and
            // `local_albums::fetch_album_tracks_blocking`.
            ItemKind::Album => self.album_tracks(item.id()),
            // Moved from `local_playback::play_folder` (local_playback.rs:312-319).
            ItemKind::Folder => self
                .with(|db| db.list_folder_tracks_recursive(item.id(), false))
                .unwrap_or_default(),
            // Provider-owned replacement for the legacy local-track resolver.
            ItemKind::Track => {
                let row_id = item.id().parse::<i64>().map_err(|_| SourceError::BadIdShape {
                    by: SourceId::LOCAL,
                    id: item.id().to_string(),
                    why: "a normalised local track id is always numeric",
                })?;
                self.track_row(row_id).into_iter().collect()
            }
            ItemKind::Playlist | ItemKind::Artist => {
                return Err(SourceError::Unsupported {
                    by: SourceId::LOCAL,
                    kind: item.kind(),
                })
            }
        };
        if rows.is_empty() {
            return Err(self.not_found(item));
        }
        Ok(rows.iter().map(local_queue_track).collect())
    }

    async fn meta(&self, item: &MediaRef) -> Result<ItemMeta, SourceError> {
        let rows: Vec<LocalTrack> = match item.kind() {
            ItemKind::Album | ItemKind::Folder => self.album_tracks(item.id()),
            ItemKind::Track => item
                .id()
                .parse::<i64>()
                .ok()
                .and_then(|id| self.track_row(id))
                .into_iter()
                .collect(),
            ItemKind::Playlist | ItemKind::Artist => {
                return Err(SourceError::Unsupported {
                    by: SourceId::LOCAL,
                    kind: item.kind(),
                })
            }
        };
        if rows.is_empty() {
            return Err(self.not_found(item));
        }
        Ok(self.meta_of_rows(item, &rows))
    }

    fn artwork(&self, item: &MediaRef, _size: ArtSize) -> ArtRef {
        let Some(src) = self.cover_source(item) else {
            return ArtRef::None;
        };
        // CHEAP phase only: the memo, or one `stat` on the thumbnail path.
        // A miss returns the ORIGINAL, which `thumbnail` upgrades on a worker.
        match self.cached_thumbnail(&src) {
            Some(thumb) => ArtRef::File(thumb),
            None if Path::new(&src).is_file() => ArtRef::File(PathBuf::from(src)),
            None => ArtRef::None,
        }
    }

    fn artwork_token(&self, token: &str, _size: ArtSize) -> ArtRef {
        // The `LocalFile` half of `artwork_qt::classify` (artwork_qt.rs:155-183),
        // minus the guessing: this only ever sees a token a LOCAL row carried,
        // so an absolute path is a file rather than "a shape that might be a
        // Plex thumb". No `stat` — this must stay pure; the CHEAP-phase
        // thumbnail lookup happens in the pipeline, exactly where it does
        // today.
        let token = token.trim();
        if token.is_empty() {
            return ArtRef::None;
        }
        // A `file://` url is ALREADY on disk — reqwest cannot even build a
        // request for it ("builder error for url"), so it must never be
        // fetched.
        if token.starts_with("file://") {
            // NOT strip_prefix: on Windows that leaves the `/` before the drive
            // (`/C:/Users/...`), which names nothing, and it leaves the three
            // percent-escapes in place on every platform.
            return ArtRef::File(PathBuf::from(qbz_models::fs_url::local_path(token)));
        }
        // An offline-download row can keep the CDN url its cover came from.
        if token.starts_with("http://") || token.starts_with("https://") {
            return ArtRef::Fetch {
                url: token.to_string(),
                cache_key: token.to_string(),
            };
        }
        if qbz_models::fs_url::is_local_abs_path(token) {
            return ArtRef::File(PathBuf::from(token));
        }
        // A bare relative string is not something this source can resolve.
        // `classify` returned `Empty` here too.
        ArtRef::None
    }

    fn thumbnail(&self, art: &ArtRef, _size: ArtSize) -> ArtRef {
        // EXPENSIVE phase: this is where the decode+downscale happens, on the
        // caller's worker pool (`local_artwork::stream_cold`,
        // local_artwork.rs:164-188).
        match art {
            ArtRef::File(p) => match self.local_thumbnail(&p.to_string_lossy()) {
                Some(thumb) => ArtRef::File(thumb),
                None => ArtRef::None,
            },
            other => other.clone(),
        }
    }

    async fn playback(
        &self,
        item: &MediaRef,
        track: &QueueTrack,
    ) -> Result<PlaybackTicket, SourceError> {
        // Moved from `local_playback::play_local_file`
        // (local_playback.rs:119-182), minus the three player calls (`:143`
        // `play_dsd_file`, `:173` `play_data`, `:155`/`:179` `seek`), which
        // STAY in qbz-qt: this crate does not link the PROTECTED backend.
        if item.kind() != ItemKind::Track {
            return Err(SourceError::Unsupported {
                by: SourceId::LOCAL,
                kind: item.kind(),
            });
        }
        let row_id = item.id().parse::<i64>().map_err(|_| SourceError::BadIdShape {
            by: SourceId::LOCAL,
            id: item.id().to_string(),
            why: "a normalised local track id is always numeric",
        })?;
        let row = self.track_row(row_id).ok_or_else(|| self.not_found(item))?;

        // An OFFLINE row is recognised here and REFUSED here, which is the
        // discipline `BadIdShape` exists for: say no at the moment of the
        // mistake instead of forwarding a shape that fails two layers down.
        //
        // A CMAF download's `local_tracks.file_path` is the BUNDLE DIRECTORY
        // (`…/audio/tracks-cmaf/<qobuz_id>/`, holding `init.mp4` and an
        // AES-encrypted `segments.bin`), not a playable container. Returned as
        // a `File` ticket it reached `std::fs::read`, which reported it as
        //
        //   audible: file not available at …/tracks-cmaf/266725026 (drive unmounted?)
        //
        // — a message about a drive, for a bundle sitting on the local disk.
        // Only `qbz-core`'s offline tier can decrypt it, and it is asked for by
        // the QOBUZ catalog id the row carries in `QueueTrack.id`, not by this
        // row id. NO fs call is made to decide this: a `is_dir()` here would
        // block for the mount timeout on exactly the unreachable share the
        // audible step's bounded probe exists to survive.
        if SourceBadge::from_word(row.source.as_deref()) == SourceBadge::Offline {
            return Err(SourceError::BadIdShape {
                by: SourceId::LOCAL,
                id: item.id().to_string(),
                why: "an offline (qobuz_download) row is a CMAF bundle only qbz-core's offline \
                      tier can read — play it by its Qobuz catalog id, not as a local file",
            });
        }

        let path = PathBuf::from(&row.file_path);

        // The player keys on the QUEUE's id, which is what every caller passes
        // today (`play_local_file(runtime, track.id)`, local_playback.rs:243).
        // It is NOT always the row id — an offline row's queue id is the Qobuz
        // track id (local_playback.rs:55-59).
        let play_id = track.id;

        // A CD track is answered FIRST and never reaches the extension tests
        // below, because it is not a file: `path` here is a `cdda:` reference
        // and `File`/`DsdFile` would both hand it to something that opens it.
        // The test is `CdRef::is_cd_path`, not a second `starts_with` written
        // out here — one parser, one shape, so this cannot drift from the
        // encoder the way a hand-copied prefix list does.
        if qbz_disc::CdRef::is_cd_path(&row.file_path) {
            return match qbz_disc::CdRef::parse(&row.file_path) {
                Some(r) => Ok(PlaybackTicket::CdTrack {
                    device: r.device,
                    start_lsn: r.start_lsn,
                    sectors: r.sectors,
                    play_id,
                }),
                None => Err(SourceError::BadIdShape {
                    by: SourceId::LOCAL,
                    id: row.file_path.clone(),
                    why: "a cdda: reference is device#start+sectors",
                }),
            };
        }

        // A SACD track rides the DSD ticket unchanged: `open_dsd` recognises
        // the reference and returns a demuxer, so the player's PCM, DoP and
        // native paths all inherit disc images with no arm of their own.
        // Answered before the extension tests because a `sacd:` string is not
        // a path and must never be opened as one.
        if qbz_disc::SacdRef::is_sacd_path(&row.file_path) {
            return Ok(PlaybackTicket::DsdFile { path, play_id });
        }

        let lower = row.file_path.to_lowercase();
        if lower.ends_with(".dsf") || lower.ends_with(".dff") {
            return Ok(PlaybackTicket::DsdFile { path, play_id });
        }
        // CUE: every virtual track of a CUE album shares ONE audio file. The
        // "is that container already loaded?" test reads `player().state`,
        // which this crate cannot link — so the offset rides along and the
        // frontend keeps making that decision (local_playback.rs:150-158).
        let seek_secs = row.cue_start_secs.filter(|s| *s > 0.0);
        Ok(PlaybackTicket::File {
            path,
            play_id,
            seek_secs,
        })
    }

    fn bind_user(&self, _uid: u64, dir: &Path) {
        self.pool.bind(dir);
        if let Ok(mut m) = self.thumbs.write() {
            m.clear();
        }
        if let Ok(mut m) = self.covers.write() {
            m.clear();
        }
    }

    fn teardown(&self) {
        self.pool.teardown();
        if let Ok(mut m) = self.thumbs.write() {
            m.clear();
        }
        if let Ok(mut m) = self.covers.write() {
            m.clear();
        }
        if let Ok(mut e) = self.ephemeral.write() {
            *e = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ItemKind;

    fn claimed(raw: &RawRef) -> Result<MediaRef, SourceError> {
        LocalSource::new()
            .claim(raw)
            .expect("LocalSource should claim this reference")
    }

    #[test]
    fn owns_the_three_group_key_shapes() {
        // survey §6.1, real rows.
        for id in [
            "/home/u/Music/Artist/Album",
            "HIT ME HARD AND SOFT|Billie Eilish",
            "__unknown_album__",
        ] {
            let raw = RawRef {
                kind: Some(ItemKind::Album),
                id: id.into(),
                ..Default::default()
            };
            let m = claimed(&raw).expect("group key is local");
            assert_eq!(m.source(), SourceId::LOCAL);
            assert_eq!(m.kind(), ItemKind::Album);
            assert_eq!(m.id(), id, "a group key is opaque and stays verbatim");
        }
    }

    #[test]
    fn owns_a_bare_numeric_row_id_only_with_evidence() {
        // Real row: local_tracks.id = 2954 (survey §6.1). With no source word
        // and no `is_local`, LocalSource must NOT claim — that guess is IC-4.
        let bare = RawRef {
            kind: Some(ItemKind::Track),
            id: "2954".into(),
            ..Default::default()
        };
        assert!(LocalSource::new().claim(&bare).is_none());

        // With `is_local = Some(true)` (a mixtape Local item), it is ours.
        let evidenced = RawRef {
            kind: Some(ItemKind::Track),
            id: "2954".into(),
            is_local: Some(true),
            ..Default::default()
        };
        assert_eq!(claimed(&evidenced).expect("local").id(), "2954");

        // With the source word, likewise.
        let worded = RawRef::new("local", ItemKind::Track, "2954");
        assert_eq!(claimed(&worded).expect("local").id(), "2954");
    }

    #[test]
    fn ephemeral_ids_are_a_local_arm_not_a_source() {
        // qbz-library/src/ephemeral.rs:43 — EPHEMERAL_ID_FLOOR = 2^48.
        let raw = RawRef {
            kind: Some(ItemKind::Track),
            id: "281474976710656".into(),
            ..Default::default()
        };
        let m = claimed(&raw).expect("ephemeral is local");
        assert_eq!(m.source(), SourceId::LOCAL);
        assert_eq!(m.id(), "281474976710656");
    }

    #[test]
    fn never_claims_a_plex_shape() {
        // The album boundary key…
        let album_key = RawRef {
            kind: Some(ItemKind::Album),
            id: "plex:5677211365378243606".into(),
            is_local: Some(true),
            ..Default::default()
        };
        assert!(LocalSource::new().claim(&album_key).is_none());
        // …and the namespaced row id (local_plex.rs:277 — real value for
        // rating key 44440).
        let namespaced = RawRef {
            kind: Some(ItemKind::Track),
            id: "1099511671736".into(),
            is_local: Some(true),
            ..Default::default()
        };
        assert!(LocalSource::new().claim(&namespaced).is_none());
    }

    #[test]
    fn an_offline_row_normalises_to_its_library_row_id() {
        // local_playback.rs:55-59 + :85-90: the queue id is the QOBUZ track id
        // and the local row id rides in the hint. Today `play_local_file` looks
        // the row up by the queue id and misses.
        let raw = RawRef {
            source: Some(SourceId::LOCAL),
            badge: SourceBadge::Offline,
            kind: Some(ItemKind::Track),
            id: "139578884".into(),
            is_local: Some(true),
            hint: Some("2954".into()),
        };
        assert_eq!(claimed(&raw).expect("local").id(), "2954");
    }

    #[test]
    fn a_local_row_ignores_a_collection_boundary_hint() {
        // resolve_collection_tracks (enqueue.rs:52-56) overwrites the hint with
        // the OWNING ITEM's id — an album group key for a local album. A plain
        // local row must not mistake it for its own id.
        let raw = RawRef {
            source: Some(SourceId::LOCAL),
            badge: SourceBadge::Local,
            kind: Some(ItemKind::Track),
            id: "2954".into(),
            is_local: Some(true),
            hint: Some("/home/u/Music/Artist/Album".into()),
        };
        assert_eq!(claimed(&raw).expect("local").id(), "2954");
    }

    #[test]
    fn local_playlists_are_refused_loudly() {
        // enqueue.rs:418-423 — the contract, now a named error instead of a
        // free-text string.
        let raw = RawRef::new("local", ItemKind::Playlist, "whatever");
        let err = claimed(&raw).unwrap_err();
        assert!(matches!(
            err,
            SourceError::Unsupported {
                by: SourceId::LOCAL,
                kind: ItemKind::Playlist
            }
        ));
    }

    #[test]
    fn a_non_numeric_track_id_is_a_named_error() {
        let raw = RawRef::new("local", ItemKind::Track, "not-a-number");
        let err = claimed(&raw).unwrap_err();
        assert!(matches!(err, SourceError::BadIdShape { by: SourceId::LOCAL, .. }));
    }

    // ── artwork_token: the LocalFile arm of the dead `classify` ─────────────

    /// The four shapes a local row's token really takes. `file://` must come
    /// back as a PATH — reqwest cannot even build a request for that scheme,
    /// so treating it as fetchable is a guaranteed miss.
    #[test]
    fn a_local_token_is_a_path_a_cdn_url_or_nothing() {
        let l = LocalSource::new();
        assert_eq!(
            l.artwork_token("file:///home/u/.cache/qbz/thumb.jpg", ArtSize::Card),
            ArtRef::File(PathBuf::from("/home/u/.cache/qbz/thumb.jpg"))
        );
        assert_eq!(
            l.artwork_token("/home/u/Music/Album/cover.jpg", ArtSize::Card),
            ArtRef::File(PathBuf::from("/home/u/Music/Album/cover.jpg"))
        );
        // An offline-download row can keep the CDN url its cover came from.
        assert!(matches!(
            l.artwork_token("https://static.qobuz.com/a.jpg", ArtSize::Card),
            ArtRef::Fetch { .. }
        ));
        assert_eq!(l.artwork_token("", ArtSize::Card), ArtRef::None);
        assert_eq!(l.artwork_token("   ", ArtSize::Card), ArtRef::None);
        // A bare relative string is not resolvable — `classify` said Empty too.
        assert_eq!(l.artwork_token("covers/a.jpg", ArtSize::Card), ArtRef::None);
    }
}
