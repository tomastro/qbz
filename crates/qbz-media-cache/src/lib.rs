//! ONE SQLite cache for every remote media server QBZ reads.
//!
//! A remote source cannot be queried at scroll speed, so its library is mirrored
//! locally and the Local Library grid `ATTACH`es that mirror and `UNION`s it with
//! `local_tracks`. That mechanism already exists — for Plex, by name:
//!
//! ```text
//! qbz-library/src/database.rs:2097
//!   get_albums_metadata_page(…, plex_cache_path: Option<&Path>, …)
//!     ATTACH DATABASE '<path>' AS plex_cache
//!     FROM plex_cache.plex_cache_tracks      // twice
//! ```
//!
//! Copying that shape per source means three `Option<&Path>` parameters and six
//! UNION arms in a SHARED crate, and every future source pays the tax again.
//! This crate is the generic form: one table with a `source` discriminator, one
//! ATTACH, one UNION arm.
//!
//! # Why Plex is NOT migrated in the same change
//!
//! The schema below is `plex_cache_tracks` with its columns renamed — every one
//! of its 19 columns maps 1:1 onto Jellyfin and Subsonic, which is what made
//! "generic" the right call rather than a guess. Folding Plex in is therefore a
//! rename plus a data migration, not a redesign.
//!
//! It is still a migration of a table that ships with a user's data — 17 145
//! rows on the owner's install — and doing it in the same change that introduces
//! two new sources would mean one commit where a regression could come from
//! either. So: this table serves Jellyfin and Subsonic first and proves itself
//! with two sources; Plex moves in as its own change, with its own verification,
//! once it has. Until then the union carries two ATTACHes instead of one, which
//! is a cost paid in one SQL builder rather than a risk taken with a live cache.
//!
//! # Identity: the namespaced row id
//!
//! The Local Library speaks in `i64` row ids, so a remote track needs one that
//! can never collide with a real `local_tracks.id`. The existing scheme is a
//! high bit per source:
//!
//! | source | floor | payload |
//! |---|---|---|
//! | Plex (`local_plex::PLEX_TRACK_ID_FLOOR`) | `1 << 40` | hashed rating key, 40 bits |
//! | ephemeral (`qbz_library::ephemeral`) | `1 << 48` | session counter |
//! | **Jellyfin** | `1 << 41` | this cache's rowid |
//! | **Subsonic** | `1 << 42` | this cache's rowid |
//!
//! Payloads are masked to 40 bits, so the ranges are disjoint by construction.
//!
//! The payload is **this table's `AUTOINCREMENT` rowid, not a hash of the
//! server's id.** Plex had to hash because its ids predate its cache, and that
//! hash is why `PlexSource::resolve_cache_row_id` has to scan to invert it. A
//! new table has no such debt: the mapping is a primary-key lookup in both
//! directions and cannot collide. With ~50 000 tracks a 40-bit hash carries
//! roughly a 0.1 % chance of at least one collision, and a collision here plays
//! the WRONG TRACK — which is precisely the class of bug the source seam exists
//! to remove, so it is not worth reintroducing for a marginally simpler write.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};

/// Namespace bit for Jellyfin row ids.
pub const JELLYFIN_ID_FLOOR: i64 = 1 << 41;
/// Namespace bit for Subsonic row ids.
pub const SUBSONIC_ID_FLOOR: i64 = 1 << 42;
/// Payload width. Matches Plex's, so every source's payload occupies the same
/// low 40 bits and the floors stay disjoint.
pub const ID_PAYLOAD_BITS: u32 = 40;
const PAYLOAD_MASK: i64 = (1 << ID_PAYLOAD_BITS) - 1;

// Jellyfin, Subsonic, and the quality hydrator own separate SQLite connections
// to the same cache file. WAL keeps their readers concurrent, but SQLite still
// permits only one writer. Serializing the short write transactions here avoids
// two deferred transactions colliding on their first UPDATE during Resync all.
// The SQLite busy timeout below remains the fallback for another QBZ process.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn lock_writer() -> MutexGuard<'static, ()> {
    WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Which remote source a cached row came from. The wire values are the same
/// words `qbz_source::SourceId` uses, so nothing has to translate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteSource {
    Jellyfin,
    Subsonic,
}

impl RemoteSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            RemoteSource::Jellyfin => "jellyfin",
            RemoteSource::Subsonic => "subsonic",
        }
    }

    pub fn from_word(w: &str) -> Option<Self> {
        match w {
            "jellyfin" => Some(RemoteSource::Jellyfin),
            "subsonic" => Some(RemoteSource::Subsonic),
            _ => None,
        }
    }

    pub const fn floor(self) -> i64 {
        match self {
            RemoteSource::Jellyfin => JELLYFIN_ID_FLOOR,
            RemoteSource::Subsonic => SUBSONIC_ID_FLOOR,
        }
    }

    /// Namespace a cache rowid for the Local Library's `i64` space.
    pub fn namespace(self, rowid: i64) -> i64 {
        self.floor() | (rowid & PAYLOAD_MASK)
    }

    /// Which source owns a namespaced id, if any.
    ///
    /// Tested against the OTHER floors on purpose: bit 40 is Plex's and bit 48
    /// is the ephemeral store's, and a predicate that answered "mine" for
    /// either would route a row to the wrong source.
    pub fn of_id(id: i64) -> Option<Self> {
        for s in [RemoteSource::Jellyfin, RemoteSource::Subsonic] {
            let floor = s.floor();
            if id & floor != 0 && id & !(floor | PAYLOAD_MASK) == 0 {
                return Some(s);
            }
        }
        None
    }

    /// Recover the cache rowid from a namespaced id.
    pub fn rowid_of(id: i64) -> i64 {
        id & PAYLOAD_MASK
    }
}

/// One cached track, as this crate stores and returns it.
///
/// Deliberately flat and owned: it crosses a `spawn_blocking` boundary on every
/// read, and a borrowed shape would tie it to the connection's lifetime.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CachedTrack {
    /// The namespaced `i64` the Local Library uses. Assigned by this crate;
    /// zero until the row is written.
    pub id: i64,
    pub source: String,
    /// The server's OWN id for this track, opaque. The only thing that can be
    /// handed back to the server.
    pub item_id: String,
    pub server_id: String,
    pub library_id: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    /// The server's album id — the grouping key for the album view.
    pub album_id: String,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub duration_ms: u64,
    pub year: Option<u32>,
    /// Complete genre set when the protocol exposes one. The singular field
    /// remains the compatibility primary value and migration fallback.
    pub genres: Vec<String>,
    pub genre: Option<String>,
    pub container: String,
    pub codec: Option<String>,
    /// `None` for lossy. Both wire sentinels (Jellyfin's null, Subsonic's 0)
    /// are folded before they reach here.
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    /// Cross-source identity when the server exposes it (Jellyfin
    /// `ProviderIds.MusicBrainzTrack`, OpenSubsonic `musicBrainzId` /
    /// `isrc`). `None` otherwise; never synthesised.
    pub isrc: Option<String>,
    pub recording_mbid: Option<String>,
    /// The RAW artwork token as the server issued it — a Jellyfin image tag, a
    /// Subsonic `coverArt` id. Never a URL: a URL embeds credentials and a
    /// size, and both change while the token does not.
    pub artwork_token: Option<String>,
    /// Album/collection artwork kept separately from the item token. Servers
    /// can expose a box cover and different artwork for each disc; combining
    /// those layers makes one overwrite the other during synchronization.
    pub collection_artwork_token: Option<String>,
    pub size_bytes: Option<u64>,
}

/// Quality-only update from a secondary server hydration pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CachedTrackQuality {
    pub item_id: String,
    pub container: String,
    pub codec: Option<String>,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u32>,
    pub bitrate_kbps: Option<u32>,
}

/// One source sync generation. The authoritative cache remains readable while
/// a newer generation is incomplete; only completion may prune older rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSyncGeneration {
    pub generation: u64,
    pub observed_rows: u64,
    pub resumed: bool,
}

/// One source-local artist aggregate for the Local Library rail.
///
/// A row is emitted for both a track artist and a distinct album artist. The
/// Qt-side normalizer then folds spelling variants and merges these aggregates
/// with local/Plex rows. Keeping this aggregation in SQLite avoids loading a
/// whole remote track cache merely to answer artist counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedArtist {
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
}

/// One cached library / music folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedLibrary {
    pub source: String,
    pub library_id: String,
    pub name: String,
    pub server_id: String,
}

pub type Result<T> = std::result::Result<T, String>;

fn map_err<E: std::fmt::Display>(what: &str) -> impl Fn(E) -> String + '_ {
    move |e| format!("media cache: {what}: {e}")
}

/// Open (creating if needed) the cache at `path` and bring its schema up to date.
///
/// WAL, like every other QBZ database (ADR-002): the scan writes while the grid
/// reads, and without WAL the grid blocks behind the sync.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(map_err("create dir"))?;
    }
    let conn = Connection::open(path).map_err(map_err("open"))?;
    conn.busy_timeout(Duration::from_secs(10))
        .map_err(map_err("busy timeout"))?;
    init_schema(&conn)?;
    Ok(conn)
}

/// The schema. Split out so tests can drive an in-memory connection.
pub fn init_schema(conn: &Connection) -> Result<()> {
    let _writer = lock_writer();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .map_err(map_err("pragmas"))?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS remote_cache_libraries (
            source      TEXT NOT NULL,
            library_id  TEXT NOT NULL,
            name        TEXT NOT NULL,
            server_id   TEXT NOT NULL DEFAULT '',
            updated_at  INTEGER NOT NULL,
            PRIMARY KEY (source, library_id)
        );

        -- `id INTEGER PRIMARY KEY AUTOINCREMENT` is load-bearing: it is the
        -- payload of the namespaced Local Library id, so it must never be
        -- REUSED after a delete. Plain `INTEGER PRIMARY KEY` reuses the highest
        -- freed rowid, which would silently point an already-published id at a
        -- different track.
        CREATE TABLE IF NOT EXISTS remote_cache_tracks (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            source          TEXT NOT NULL,
            item_id         TEXT NOT NULL,
            server_id       TEXT NOT NULL DEFAULT '',
            library_id      TEXT NOT NULL DEFAULT '',
            title           TEXT NOT NULL DEFAULT '',
            artist          TEXT NOT NULL DEFAULT '',
            album_artist    TEXT NOT NULL DEFAULT '',
            album           TEXT NOT NULL DEFAULT '',
            album_id        TEXT NOT NULL DEFAULT '',
            track_number    INTEGER,
            disc_number     INTEGER,
            duration_ms     INTEGER NOT NULL DEFAULT 0,
            year            INTEGER,
            genre           TEXT,
            genres_json     TEXT NOT NULL DEFAULT '[]',
            container       TEXT NOT NULL DEFAULT '',
            codec           TEXT,
            bit_depth       INTEGER,
            sample_rate_hz  INTEGER,
            channels        INTEGER,
            bitrate_kbps    INTEGER,
            artwork_token   TEXT,
            collection_artwork_token TEXT,
            size_bytes      INTEGER,
            observed_generation INTEGER NOT NULL DEFAULT 0,
            quality_hydrated INTEGER NOT NULL DEFAULT 0,
            quality_retry_at INTEGER NOT NULL DEFAULT 0,
            updated_at      INTEGER NOT NULL,
            UNIQUE (source, item_id)
        );

        CREATE TABLE IF NOT EXISTS remote_cache_source_sync (
            source          TEXT PRIMARY KEY,
            generation      INTEGER NOT NULL DEFAULT 0,
            observed_rows   INTEGER NOT NULL DEFAULT 0,
            status          TEXT NOT NULL DEFAULT 'idle',
            updated_at      INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_rct_source_album
            ON remote_cache_tracks(source, album_id);
        CREATE INDEX IF NOT EXISTS idx_rct_source_library
            ON remote_cache_tracks(source, library_id);
        -- The Local Library grid sorts and filters on these two.
        CREATE INDEX IF NOT EXISTS idx_rct_album_artist
            ON remote_cache_tracks(album_artist);
        CREATE INDEX IF NOT EXISTS idx_rct_title ON remote_cache_tracks(title);
        ",
    )
    .map_err(map_err("schema"))?;

    // Forward-only additive migration for caches created before incremental
    // source generations and deferred Jellyfin quality existed.
    for column in [
        "genres_json TEXT NOT NULL DEFAULT '[]'",
        "observed_generation INTEGER NOT NULL DEFAULT 0",
        "quality_hydrated INTEGER NOT NULL DEFAULT 0",
        "quality_retry_at INTEGER NOT NULL DEFAULT 0",
        "collection_artwork_token TEXT",
        // Identity (2026-08-28): the server's MusicBrainz recording id and
        // ISRC when it has them. NULL until the next sync re-reads the rows.
        "isrc TEXT",
        "recording_mbid TEXT",
    ] {
        let statement = format!("ALTER TABLE remote_cache_tracks ADD COLUMN {column}");
        let _ = conn.execute(&statement, []);
    }
    let _ = conn.execute(
        "ALTER TABLE remote_cache_source_sync
         ADD COLUMN observed_rows INTEGER NOT NULL DEFAULT 0",
        [],
    );
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_rct_quality_pending
         ON remote_cache_tracks(source,quality_hydrated,quality_retry_at,updated_at)",
        [],
    )
    .map_err(map_err("quality index"))?;
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn row_to_track(row: &rusqlite::Row<'_>) -> rusqlite::Result<CachedTrack> {
    let source: String = row.get("source")?;
    let rowid: i64 = row.get("id")?;
    let id = RemoteSource::from_word(&source)
        .map(|s| s.namespace(rowid))
        .unwrap_or(rowid);
    let genre: Option<String> = row.get("genre")?;
    let mut genres = serde_json::from_str::<Vec<String>>(&row.get::<_, String>("genres_json")?)
        .unwrap_or_default();
    if genres.is_empty() {
        if let Some(value) = genre.as_ref().filter(|value| !value.trim().is_empty()) {
            genres.push(value.trim().to_string());
        }
    }
    Ok(CachedTrack {
        id,
        source,
        item_id: row.get("item_id")?,
        server_id: row.get("server_id")?,
        library_id: row.get("library_id")?,
        title: row.get("title")?,
        artist: row.get("artist")?,
        album_artist: row.get("album_artist")?,
        album: row.get("album")?,
        album_id: row.get("album_id")?,
        track_number: row.get::<_, Option<i64>>("track_number")?.map(|v| v as u32),
        disc_number: row.get::<_, Option<i64>>("disc_number")?.map(|v| v as u32),
        duration_ms: row.get::<_, i64>("duration_ms")? as u64,
        year: row.get::<_, Option<i64>>("year")?.map(|v| v as u32),
        genres,
        genre,
        container: row.get("container")?,
        codec: row.get("codec")?,
        bit_depth: row.get::<_, Option<i64>>("bit_depth")?.map(|v| v as u32),
        sample_rate_hz: row
            .get::<_, Option<i64>>("sample_rate_hz")?
            .map(|v| v as u32),
        channels: row.get::<_, Option<i64>>("channels")?.map(|v| v as u32),
        bitrate_kbps: row.get::<_, Option<i64>>("bitrate_kbps")?.map(|v| v as u32),
        isrc: row.get::<_, Option<String>>("isrc")?.filter(|v| !v.is_empty()),
        recording_mbid: row
            .get::<_, Option<String>>("recording_mbid")?
            .filter(|v| !v.is_empty()),
        artwork_token: row.get("artwork_token")?,
        collection_artwork_token: row.get("collection_artwork_token")?,
        size_bytes: row.get::<_, Option<i64>>("size_bytes")?.map(|v| v as u64),
    })
}

const SELECT: &str = "SELECT id, source, item_id, server_id, library_id, title, artist, \
     album_artist, album, album_id, track_number, disc_number, duration_ms, year, genre, genres_json, \
     container, codec, bit_depth, sample_rate_hz, channels, bitrate_kbps, artwork_token, \
     collection_artwork_token, size_bytes, isrc, recording_mbid FROM remote_cache_tracks";

fn genres_json(track: &CachedTrack) -> String {
    let mut genres = Vec::<String>::new();
    for value in track.genres.iter().chain(track.genre.iter()) {
        let value = value.trim();
        if !value.is_empty()
            && !genres
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(value))
        {
            genres.push(value.to_string());
        }
    }
    serde_json::to_string(&genres).unwrap_or_else(|_| "[]".to_string())
}

/// UPSERT a batch of tracks in ONE transaction.
///
/// `ON CONFLICT (source, item_id) DO UPDATE` rather than delete-then-insert:
/// the row id is a published identity (it is inside every queue entry and every
/// `session.db` row), so a re-scan must not mint a new one for a track that was
/// already there. A user who re-syncs while a Jellyfin track is playing would
/// otherwise find the queue pointing at nothing.
pub fn save_tracks(
    conn: &mut Connection,
    source: RemoteSource,
    tracks: &[CachedTrack],
) -> Result<usize> {
    if tracks.is_empty() {
        return Ok(0);
    }
    let _writer = lock_writer();
    let ts = now();
    let tx = conn.transaction().map_err(map_err("begin"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO remote_cache_tracks
                   (source, item_id, server_id, library_id, title, artist, album_artist, album,
                    album_id, track_number, disc_number, duration_ms, year, genre, genres_json, container,
                    codec, bit_depth, sample_rate_hz, channels, bitrate_kbps, artwork_token,
                    collection_artwork_token, size_bytes, quality_hydrated, quality_retry_at, updated_at,
                    isrc, recording_mbid)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,1,0,?25,?26,?27)
                 ON CONFLICT(source, item_id) DO UPDATE SET
                    isrc=COALESCE(excluded.isrc, remote_cache_tracks.isrc),
                    recording_mbid=COALESCE(excluded.recording_mbid, remote_cache_tracks.recording_mbid),
                    server_id=excluded.server_id, library_id=excluded.library_id,
                    title=excluded.title, artist=excluded.artist,
                    album_artist=excluded.album_artist, album=excluded.album,
                    album_id=excluded.album_id, track_number=excluded.track_number,
                    disc_number=excluded.disc_number, duration_ms=excluded.duration_ms,
                    year=excluded.year, genre=excluded.genre,
                    genres_json=excluded.genres_json, container=excluded.container,
                    codec=excluded.codec, bit_depth=excluded.bit_depth,
                    sample_rate_hz=excluded.sample_rate_hz, channels=excluded.channels,
                    bitrate_kbps=excluded.bitrate_kbps, artwork_token=excluded.artwork_token,
                    collection_artwork_token=excluded.collection_artwork_token,
                    size_bytes=excluded.size_bytes, quality_hydrated=1,
                    quality_retry_at=0, updated_at=excluded.updated_at",
            )
            .map_err(map_err("prepare insert"))?;
        for t in tracks {
            let genres_json = genres_json(t);
            stmt.execute(params![
                source.as_str(),
                t.item_id,
                t.server_id,
                t.library_id,
                t.title,
                t.artist,
                t.album_artist,
                t.album,
                t.album_id,
                t.track_number.map(|v| v as i64),
                t.disc_number.map(|v| v as i64),
                t.duration_ms as i64,
                t.year.map(|v| v as i64),
                t.genre,
                genres_json,
                t.container,
                t.codec,
                t.bit_depth.map(|v| v as i64),
                t.sample_rate_hz.map(|v| v as i64),
                t.channels.map(|v| v as i64),
                t.bitrate_kbps.map(|v| v as i64),
                t.artwork_token,
                t.collection_artwork_token,
                t.size_bytes.map(|v| v as i64),
                ts,
                t.isrc,
                t.recording_mbid,
            ])
            .map_err(map_err("insert track"))?;
        }
    }
    tx.commit().map_err(map_err("commit"))?;
    Ok(tracks.len())
}

fn next_source_revision(conn: &Connection, source: RemoteSource) -> Result<i64> {
    let current = conn
        .query_row(
            "SELECT COALESCE(MAX(updated_at),0) FROM remote_cache_tracks WHERE source=?1",
            params![source.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_err("read source revision"))?;
    Ok(now().max(current.saturating_add(1)))
}

/// Start a fresh authoritative generation for one remote source.
///
/// A previous interrupted generation is deliberately not resumed here: the
/// Jellyfin essential pass is cheap and its offset spans multiple libraries.
/// Long-running quality hydration resumes independently from persisted pending
/// rows, while stale catalog rows remain intact until this generation finishes.
pub fn begin_source_sync(
    conn: &mut Connection,
    source: RemoteSource,
) -> Result<SourceSyncGeneration> {
    let _writer = lock_writer();
    let tx = conn.transaction().map_err(map_err("begin source sync"))?;
    let previous = tx
        .query_row(
            "SELECT generation FROM remote_cache_source_sync WHERE source=?1",
            params![source.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(map_err("read source sync"))?
        .unwrap_or(0)
        .max(0) as u64;
    let generation = previous.saturating_add(1).min(i64::MAX as u64).max(1);
    tx.execute(
        "INSERT INTO remote_cache_source_sync(source,generation,observed_rows,status,updated_at)
         VALUES (?1,?2,0,'running',?3)
         ON CONFLICT(source) DO UPDATE SET generation=excluded.generation,
             observed_rows=0,status='running',updated_at=excluded.updated_at",
        params![source.as_str(), generation as i64, now()],
    )
    .map_err(map_err("write source sync"))?;
    tx.commit().map_err(map_err("commit source sync"))?;
    Ok(SourceSyncGeneration {
        generation,
        observed_rows: 0,
        resumed: false,
    })
}

/// Upsert one cheap metadata page and mark its quality pending.
///
/// Existing quality values remain visible until their replacement arrives;
/// `quality_hydrated=0` is the separate truth that makes even a legitimately
/// NULL lossy bit depth distinguishable from "not fetched yet".
pub fn save_essential_tracks(
    conn: &mut Connection,
    source: RemoteSource,
    generation: u64,
    tracks: &[CachedTrack],
) -> Result<usize> {
    save_source_generation_page(conn, source, generation, tracks, false)
}

/// Upsert one complete-quality page in an authoritative source generation.
/// Subsonic carries its quality fields in the ordinary song row, so discovery
/// and hydration are one atomic page transaction for that protocol.
pub fn save_generation_tracks(
    conn: &mut Connection,
    source: RemoteSource,
    generation: u64,
    tracks: &[CachedTrack],
) -> Result<usize> {
    save_source_generation_page(conn, source, generation, tracks, true)
}

fn save_source_generation_page(
    conn: &mut Connection,
    source: RemoteSource,
    generation: u64,
    tracks: &[CachedTrack],
    complete_quality: bool,
) -> Result<usize> {
    if tracks.is_empty() {
        return Ok(0);
    }
    let _writer = lock_writer();
    let tx = conn
        .transaction()
        .map_err(map_err("begin essential page"))?;
    let state = tx
        .query_row(
            "SELECT status,observed_rows FROM remote_cache_source_sync
              WHERE source=?1 AND generation=?2",
            params![source.as_str(), generation.min(i64::MAX as u64) as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(map_err("read essential generation"))?;
    let Some((status, observed_rows)) = state else {
        return Err("media cache: essential generation is missing".to_string());
    };
    if status != "running" {
        return Err("media cache: essential generation is no longer current".to_string());
    }
    let revision = next_source_revision(&tx, source)?;
    let generation = generation.min(i64::MAX as u64) as i64;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO remote_cache_tracks
                   (source,item_id,server_id,library_id,title,artist,album_artist,album,
                    album_id,track_number,disc_number,duration_ms,year,genre,genres_json,container,
                    codec,bit_depth,sample_rate_hz,channels,bitrate_kbps,artwork_token,
                    collection_artwork_token,size_bytes,observed_generation,quality_hydrated,quality_retry_at,updated_at,
                    isrc,recording_mbid)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,
                         ?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,0,?27,?28,?29)
                 ON CONFLICT(source,item_id) DO UPDATE SET
                    isrc=COALESCE(excluded.isrc,remote_cache_tracks.isrc),
                    recording_mbid=COALESCE(excluded.recording_mbid,remote_cache_tracks.recording_mbid),
                    server_id=excluded.server_id,library_id=excluded.library_id,
                    title=excluded.title,artist=excluded.artist,
                    album_artist=excluded.album_artist,album=excluded.album,
                    album_id=excluded.album_id,track_number=excluded.track_number,
                    disc_number=excluded.disc_number,duration_ms=excluded.duration_ms,
                    year=excluded.year,genre=excluded.genre,
                    genres_json=excluded.genres_json,
                    container=CASE WHEN excluded.quality_hydrated=1
                        THEN excluded.container
                        ELSE COALESCE(NULLIF(excluded.container,''),remote_cache_tracks.container)
                    END,
                    codec=CASE WHEN excluded.quality_hydrated=1
                        THEN excluded.codec ELSE remote_cache_tracks.codec END,
                    bit_depth=CASE WHEN excluded.quality_hydrated=1
                        THEN excluded.bit_depth ELSE remote_cache_tracks.bit_depth END,
                    sample_rate_hz=CASE WHEN excluded.quality_hydrated=1
                        THEN excluded.sample_rate_hz ELSE remote_cache_tracks.sample_rate_hz END,
                    channels=CASE WHEN excluded.quality_hydrated=1
                        THEN excluded.channels ELSE remote_cache_tracks.channels END,
                    bitrate_kbps=CASE WHEN excluded.quality_hydrated=1
                        THEN excluded.bitrate_kbps ELSE remote_cache_tracks.bitrate_kbps END,
                    artwork_token=excluded.artwork_token,
                    collection_artwork_token=excluded.collection_artwork_token,
                    size_bytes=excluded.size_bytes,
                    observed_generation=excluded.observed_generation,
                    quality_hydrated=excluded.quality_hydrated,
                    quality_retry_at=0,updated_at=excluded.updated_at",
            )
            .map_err(map_err("prepare essential page"))?;
        for track in tracks {
            let genres_json = genres_json(track);
            stmt.execute(params![
                source.as_str(),
                track.item_id,
                track.server_id,
                track.library_id,
                track.title,
                track.artist,
                track.album_artist,
                track.album,
                track.album_id,
                track.track_number.map(|value| value as i64),
                track.disc_number.map(|value| value as i64),
                track.duration_ms.min(i64::MAX as u64) as i64,
                track.year.map(|value| value as i64),
                track.genre,
                genres_json,
                track.container,
                track.codec,
                track.bit_depth.map(|value| value as i64),
                track.sample_rate_hz.map(|value| value as i64),
                track.channels.map(|value| value as i64),
                track.bitrate_kbps.map(|value| value as i64),
                track.artwork_token,
                track.collection_artwork_token,
                track
                    .size_bytes
                    .map(|value| value.min(i64::MAX as u64) as i64),
                generation,
                i64::from(complete_quality),
                revision,
                track.isrc,
                track.recording_mbid,
            ])
            .map_err(map_err("write essential track"))?;
        }
    }
    let expected = (observed_rows.max(0) as u64).saturating_add(tracks.len() as u64);
    let actual = tx
        .query_row(
            "SELECT COUNT(*) FROM remote_cache_tracks
              WHERE source=?1 AND observed_generation=?2",
            params![source.as_str(), generation],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_err("verify essential identities"))?
        .max(0) as u64;
    if actual != expected {
        return Err("media cache: source repeated an item id across pages".to_string());
    }
    tx.execute(
        "UPDATE remote_cache_source_sync SET observed_rows=?3,updated_at=?4
          WHERE source=?1 AND generation=?2 AND status='running'",
        params![source.as_str(), generation, actual as i64, now()],
    )
    .map_err(map_err("checkpoint essential page"))?;
    tx.commit().map_err(map_err("commit essential page"))?;
    Ok(tracks.len())
}

/// Finish a source generation and, for a full sweep, atomically prune rows the
/// server did not observe. A failed/interrupted caller never reaches this gate.
pub fn complete_source_sync(
    conn: &mut Connection,
    source: RemoteSource,
    generation: u64,
    prune_old: bool,
) -> Result<usize> {
    let _writer = lock_writer();
    let tx = conn
        .transaction()
        .map_err(map_err("begin source completion"))?;
    let generation = generation.min(i64::MAX as u64) as i64;
    let state = tx
        .query_row(
            "SELECT status,observed_rows FROM remote_cache_source_sync
              WHERE source=?1 AND generation=?2",
            params![source.as_str(), generation],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(map_err("read source completion"))?;
    let Some((status, observed_rows)) = state else {
        return Err("media cache: source generation is missing".to_string());
    };
    if status != "running" {
        return Err("media cache: source generation cannot authorize prune".to_string());
    }
    let actual = tx
        .query_row(
            "SELECT COUNT(*) FROM remote_cache_tracks
              WHERE source=?1 AND observed_generation=?2",
            params![source.as_str(), generation],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_err("verify source generation"))?;
    if actual.max(0) != observed_rows.max(0) {
        return Err("media cache: source generation identity count changed".to_string());
    }
    let pruned = if prune_old {
        tx.execute(
            "DELETE FROM remote_cache_tracks
              WHERE source=?1 AND observed_generation<>?2",
            params![source.as_str(), generation],
        )
        .map_err(map_err("prune source generation"))?
    } else {
        0
    };
    tx.execute(
        "UPDATE remote_cache_source_sync SET status='complete',updated_at=?3
          WHERE source=?1 AND generation=?2",
        params![source.as_str(), generation, now()],
    )
    .map_err(map_err("finish source generation"))?;
    tx.commit().map_err(map_err("commit source completion"))?;
    Ok(pruned)
}

pub fn interrupt_source_sync(
    conn: &Connection,
    source: RemoteSource,
    generation: u64,
) -> Result<()> {
    let _writer = lock_writer();
    conn.execute(
        "UPDATE remote_cache_source_sync SET status='interrupted',updated_at=?3
          WHERE source=?1 AND generation=?2 AND status='running'",
        params![
            source.as_str(),
            generation.min(i64::MAX as u64) as i64,
            now()
        ],
    )
    .map(|_| ())
    .map_err(map_err("interrupt source sync"))
}

/// Pending quality ids in deterministic order. The retry timestamp prevents a
/// deleted item or down server from creating a hot loop.
pub fn quality_candidates(
    conn: &Connection,
    source: RemoteSource,
    limit: usize,
) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(
            "SELECT item_id FROM remote_cache_tracks
              WHERE source=?1 AND quality_hydrated=0 AND quality_retry_at<=?2
              ORDER BY updated_at DESC,id LIMIT ?3",
        )
        .map_err(map_err("prepare quality candidates"))?;
    let rows = stmt
        .query_map(
            params![source.as_str(), now(), limit.min(i64::MAX as usize) as i64],
            |row| row.get::<_, String>(0),
        )
        .map_err(map_err("query quality candidates"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_err("read quality candidates"))
}

/// Preserve caller priority while dropping ids already hydrated or deferred.
pub fn pending_quality_ids(
    conn: &Connection,
    source: RemoteSource,
    item_ids: &[String],
) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(
            "SELECT EXISTS(
                 SELECT 1 FROM remote_cache_tracks
                  WHERE source=?1 AND item_id=?2 AND quality_hydrated=0
                    AND quality_retry_at<=?3
             )",
        )
        .map_err(map_err("prepare pending quality lookup"))?;
    let mut pending = Vec::new();
    for item_id in item_ids {
        let exists = stmt
            .query_row(params![source.as_str(), item_id, now()], |row| {
                row.get::<_, bool>(0)
            })
            .map_err(map_err("read pending quality lookup"))?;
        if exists {
            pending.push(item_id.clone());
        }
    }
    Ok(pending)
}

/// Apply a quality batch without rewriting metadata or changing published ids.
pub fn update_track_quality(
    conn: &mut Connection,
    source: RemoteSource,
    updates: &[CachedTrackQuality],
) -> Result<usize> {
    if updates.is_empty() {
        return Ok(0);
    }
    let _writer = lock_writer();
    let tx = conn
        .transaction()
        .map_err(map_err("begin quality update"))?;
    let revision = next_source_revision(&tx, source)?;
    let mut affected = 0usize;
    {
        let mut stmt = tx
            .prepare(
                "UPDATE remote_cache_tracks SET
                    container=COALESCE(NULLIF(?3,''),container),codec=?4,bit_depth=?5,
                    sample_rate_hz=?6,channels=?7,bitrate_kbps=?8,
                    quality_hydrated=1,quality_retry_at=0,updated_at=?9
                  WHERE source=?1 AND item_id=?2",
            )
            .map_err(map_err("prepare quality update"))?;
        for update in updates {
            affected += stmt
                .execute(params![
                    source.as_str(),
                    update.item_id,
                    update.container,
                    update.codec,
                    update.bit_depth.map(|value| value as i64),
                    update.sample_rate_hz.map(|value| value as i64),
                    update.channels.map(|value| value as i64),
                    update.bitrate_kbps.map(|value| value as i64),
                    revision,
                ])
                .map_err(map_err("write quality update"))?;
        }
    }
    tx.commit().map_err(map_err("commit quality update"))?;
    Ok(affected)
}

pub fn defer_track_quality(
    conn: &mut Connection,
    source: RemoteSource,
    item_ids: &[String],
    retry_after_secs: i64,
) -> Result<usize> {
    if item_ids.is_empty() {
        return Ok(0);
    }
    let _writer = lock_writer();
    let retry_at = now().saturating_add(retry_after_secs.max(1));
    let tx = conn.transaction().map_err(map_err("begin quality defer"))?;
    let mut affected = 0usize;
    {
        let mut stmt = tx
            .prepare(
                "UPDATE remote_cache_tracks SET quality_retry_at=?3
                  WHERE source=?1 AND item_id=?2 AND quality_hydrated=0",
            )
            .map_err(map_err("prepare quality defer"))?;
        for item_id in item_ids {
            affected += stmt
                .execute(params![source.as_str(), item_id, retry_at])
                .map_err(map_err("write quality defer"))?;
        }
    }
    tx.commit().map_err(map_err("commit quality defer"))?;
    Ok(affected)
}

/// Replace the known libraries for one source.
pub fn save_libraries(
    conn: &mut Connection,
    source: RemoteSource,
    libs: &[CachedLibrary],
) -> Result<()> {
    let _writer = lock_writer();
    let ts = now();
    let tx = conn.transaction().map_err(map_err("begin"))?;
    tx.execute(
        "DELETE FROM remote_cache_libraries WHERE source = ?1",
        params![source.as_str()],
    )
    .map_err(map_err("clear libraries"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO remote_cache_libraries (source, library_id, name, server_id, updated_at)
                 VALUES (?1,?2,?3,?4,?5)",
            )
            .map_err(map_err("prepare library"))?;
        for l in libs {
            stmt.execute(params![
                source.as_str(),
                l.library_id,
                l.name,
                l.server_id,
                ts
            ])
            .map_err(map_err("insert library"))?;
        }
    }
    tx.commit().map_err(map_err("commit"))
}

pub fn libraries(conn: &Connection, source: RemoteSource) -> Result<Vec<CachedLibrary>> {
    let mut stmt = conn
        .prepare("SELECT source, library_id, name, server_id FROM remote_cache_libraries WHERE source = ?1 ORDER BY name")
        .map_err(map_err("prepare libraries"))?;
    let rows = stmt
        .query_map(params![source.as_str()], |r| {
            Ok(CachedLibrary {
                source: r.get(0)?,
                library_id: r.get(1)?,
                name: r.get(2)?,
                server_id: r.get(3)?,
            })
        })
        .map_err(map_err("query libraries"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_err("read libraries"))
}

/// One track by its NAMESPACED id — a primary-key lookup, not a scan.
///
/// This is the whole payoff of using the rowid as the payload: `PlexSource`
/// needs `resolve_cache_row_id` to invert a hash, and its slow path is a full
/// table scan. Here the id IS the key.
pub fn track_by_id(conn: &Connection, id: i64) -> Result<Option<CachedTrack>> {
    let Some(source) = RemoteSource::of_id(id) else {
        return Ok(None);
    };
    let rowid = RemoteSource::rowid_of(id);
    conn.query_row(
        &format!("{SELECT} WHERE id = ?1 AND source = ?2"),
        params![rowid, source.as_str()],
        row_to_track,
    )
    .optional()
    .map_err(map_err("track by id"))
}

/// One track by the SERVER's own id.
pub fn track_by_item_id(
    conn: &Connection,
    source: RemoteSource,
    item_id: &str,
) -> Result<Option<CachedTrack>> {
    conn.query_row(
        &format!("{SELECT} WHERE source = ?1 AND item_id = ?2"),
        params![source.as_str(), item_id],
        row_to_track,
    )
    .optional()
    .map_err(map_err("track by item id"))
}

/// Every track of one album, in disc/track order.
///
/// NULLs sort LAST rather than first: 75 of 4924 measured Jellyfin rows carry no
/// track number, and letting them lead would put the untagged tracks above
/// track 1 on every album that has any.
pub fn album_tracks(
    conn: &Connection,
    source: RemoteSource,
    album_id: &str,
) -> Result<Vec<CachedTrack>> {
    let mut stmt = conn
        .prepare(&format!(
            "{SELECT} WHERE source = ?1 AND album_id = ?2 \
             ORDER BY disc_number IS NULL, disc_number, track_number IS NULL, track_number, title"
        ))
        .map_err(map_err("prepare album"))?;
    let rows = stmt
        .query_map(params![source.as_str(), album_id], row_to_track)
        .map_err(map_err("query album"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_err("read album"))
}

/// Substring search across title / artist / album. An empty needle matches
/// everything, which is what the Local Library's unfiltered listing wants.
pub fn search(
    conn: &Connection,
    source: RemoteSource,
    needle: &str,
    limit: Option<u32>,
) -> Result<Vec<CachedTrack>> {
    search_page(
        conn,
        source,
        needle,
        0,
        limit.unwrap_or(u32::MAX) as u64,
        "default",
    )
}

/// One deterministic page of a remote source, in the same allowlisted sort
/// orders used by the local Tracks query. `offset` belongs to this source only.
pub fn search_page(
    conn: &Connection,
    source: RemoteSource,
    needle: &str,
    offset: u64,
    limit: u64,
    sort: &str,
) -> Result<Vec<CachedTrack>> {
    search_page_filtered(conn, source, needle, offset, limit, sort, &[], false, &[])
}

pub fn search_page_filtered(
    conn: &Connection,
    source: RemoteSource,
    needle: &str,
    offset: u64,
    limit: u64,
    sort: &str,
    formats: &[String],
    other_formats: bool,
    quality_tiers: &[String],
) -> Result<Vec<CachedTrack>> {
    let like = format!("%{}%", needle.trim());
    let order = match sort {
        "title-asc" => "title COLLATE NOCASE, artist COLLATE NOCASE, id",
        "title-desc" => "title COLLATE NOCASE DESC, artist COLLATE NOCASE, id",
        "artist-asc" => "COALESCE(NULLIF(album_artist, ''), artist) COLLATE NOCASE, album COLLATE NOCASE, disc_number, track_number, id",
        "artist-desc" => "COALESCE(NULLIF(album_artist, ''), artist) COLLATE NOCASE DESC, album COLLATE NOCASE, disc_number, track_number, id",
        "group-artist" => "artist COLLATE NOCASE, album COLLATE NOCASE, title COLLATE NOCASE, id",
        "year-desc" => "year IS NULL, year DESC, album COLLATE NOCASE, disc_number, track_number, id",
        "year-asc" => "year IS NULL, year ASC, album COLLATE NOCASE, disc_number, track_number, id",
        // These rows currently map to LocalTrack with indexed_at=0.
        "added-desc" => "album COLLATE NOCASE, disc_number, track_number, id",
        _ => "album COLLATE NOCASE, COALESCE(NULLIF(album_artist, ''), artist) COLLATE NOCASE, disc_number, track_number, title COLLATE NOCASE, id",
    };
    let media_filter = cached_track_media_filter_sql(
        "container",
        "bit_depth",
        "sample_rate_hz",
        formats,
        other_formats,
        quality_tiers,
    );
    let mut stmt = conn
        .prepare(&format!(
            "{SELECT} WHERE source = ?1 AND (?2 = '' OR title LIKE ?3 OR artist LIKE ?3 \
             OR album LIKE ?3 OR album_artist LIKE ?3) {media_filter} \
             ORDER BY {order} LIMIT ?4 OFFSET ?5"
        ))
        .map_err(map_err("prepare search"))?;
    let rows = stmt
        .query_map(
            params![
                source.as_str(),
                needle.trim(),
                like,
                limit as i64,
                offset as i64
            ],
            row_to_track,
        )
        .map_err(map_err("query search"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_err("read search"))
}

fn cached_track_media_filter_sql(
    format_col: &str,
    depth_col: &str,
    rate_col: &str,
    formats: &[String],
    other_formats: bool,
    quality_tiers: &[String],
) -> String {
    let known = ["flac", "alac", "ape", "wav", "mp3", "aac"];
    let selected = known
        .iter()
        .copied()
        .filter(|value| {
            formats
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(value))
        })
        .collect::<Vec<_>>();
    let mut clauses = Vec::new();
    if !selected.is_empty() || other_formats {
        let mut values = selected
            .into_iter()
            .map(|value| format!("LOWER(COALESCE({format_col},''))='{value}'"))
            .collect::<Vec<_>>();
        if other_formats {
            values.push(format!(
                "LOWER(COALESCE({format_col},'')) NOT IN ('flac','alac','ape','wav','mp3','aac')"
            ));
        }
        clauses.push(format!("AND ({})", values.join(" OR ")));
    }
    let hires = quality_tiers.iter().any(|value| value == "hires");
    let cd = quality_tiers.iter().any(|value| value == "cd");
    let lossy = quality_tiers.iter().any(|value| value == "lossy");
    if hires || cd || lossy {
        let khz = format!(
            "CASE WHEN COALESCE({rate_col},0)>=1000 THEN COALESCE({rate_col},0)/1000.0 ELSE COALESCE({rate_col},0) END"
        );
        let mut values = Vec::new();
        if hires {
            values.push(format!(
                "(LOWER(COALESCE({format_col},'')) IN ('dsd','dsf','dff') OR COALESCE({depth_col},0)>=24)"
            ));
        }
        if cd {
            values.push(format!(
                "(LOWER(COALESCE({format_col},'')) NOT IN ('mp3','dsd','dsf','dff') AND (({depth_col} IS NOT NULL AND {depth_col}<24) OR ({depth_col} IS NULL AND {khz}>=44.1)))"
            ));
        }
        if lossy {
            values.push(format!("LOWER(COALESCE({format_col},''))='mp3'"));
        }
        clauses.push(format!("AND ({})", values.join(" OR ")));
    }
    clauses.join(" ")
}

/// Artist aggregates for one remote source, including track credits and
/// distinct album artists.
pub fn artists(conn: &Connection, source: RemoteSource) -> Result<Vec<CachedArtist>> {
    let mut stmt = conn
        .prepare(
            "WITH credits AS (
                 SELECT id, TRIM(artist) AS name,
                        COALESCE(NULLIF(album_id, ''), album) AS album_key
                   FROM remote_cache_tracks
                  WHERE source = ?1 AND TRIM(artist) != ''
                 UNION
                 SELECT id, TRIM(album_artist) AS name,
                        COALESCE(NULLIF(album_id, ''), album) AS album_key
                   FROM remote_cache_tracks
                  WHERE source = ?1 AND TRIM(album_artist) != ''
             )
             SELECT name, COUNT(DISTINCT album_key), COUNT(DISTINCT id)
               FROM credits
              GROUP BY name COLLATE NOCASE
              ORDER BY name COLLATE NOCASE",
        )
        .map_err(map_err("prepare artists"))?;
    let rows = stmt
        .query_map(params![source.as_str()], |row| {
            Ok(CachedArtist {
                name: row.get(0)?,
                album_count: row.get::<_, i64>(1)? as u32,
                track_count: row.get::<_, i64>(2)? as u32,
            })
        })
        .map_err(map_err("query artists"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_err("read artists"))
}

pub fn count(conn: &Connection, source: RemoteSource) -> Result<u64> {
    conn.query_row(
        "SELECT COUNT(*) FROM remote_cache_tracks WHERE source = ?1",
        params![source.as_str()],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as u64)
    .map_err(map_err("count"))
}

/// Forget everything one source cached (a disconnect, or a server swap).
pub fn clear(conn: &mut Connection, source: RemoteSource) -> Result<usize> {
    let _writer = lock_writer();
    let tx = conn.transaction().map_err(map_err("begin"))?;
    let n = tx
        .execute(
            "DELETE FROM remote_cache_tracks WHERE source = ?1",
            params![source.as_str()],
        )
        .map_err(map_err("clear tracks"))?;
    tx.execute(
        "DELETE FROM remote_cache_libraries WHERE source = ?1",
        params![source.as_str()],
    )
    .map_err(map_err("clear libraries"))?;
    tx.execute(
        "DELETE FROM remote_cache_source_sync WHERE source = ?1",
        params![source.as_str()],
    )
    .map_err(map_err("clear source sync"))?;
    tx.commit().map_err(map_err("commit"))?;
    Ok(n)
}

/// Drop rows the last sweep did not touch — i.e. tracks deleted on the server.
///
/// Keyed on `updated_at` rather than on a set of ids: a full sweep of 50 000
/// tracks would otherwise have to hold every id in memory and build an
/// `IN (…)` of that size. `before` is the timestamp taken BEFORE the sweep
/// started, so anything still older than it was not seen.
///
/// Only ever called after a sweep that COMPLETED. A partial sweep — a dropped
/// connection halfway through — would otherwise read as "the server deleted
/// everything it did not get to".
pub fn prune_stale(conn: &mut Connection, source: RemoteSource, before: i64) -> Result<usize> {
    let _writer = lock_writer();
    let tx = conn.transaction().map_err(map_err("begin"))?;
    let n = tx
        .execute(
            "DELETE FROM remote_cache_tracks WHERE source = ?1 AND updated_at < ?2",
            params![source.as_str(), before],
        )
        .map_err(map_err("prune"))?;
    tx.commit().map_err(map_err("commit"))?;
    Ok(n)
}

/// The timestamp a caller should hand to [`prune_stale`] after its sweep.
pub fn sweep_start() -> i64 {
    now()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init_schema(&c).unwrap();
        c
    }

    fn track(item_id: &str, album: &str, disc: Option<u32>, no: Option<u32>) -> CachedTrack {
        CachedTrack {
            item_id: item_id.into(),
            album_id: album.into(),
            album: album.into(),
            title: format!("t-{item_id}"),
            artist: "A".into(),
            album_artist: "A".into(),
            disc_number: disc,
            track_number: no,
            duration_ms: 1000,
            container: "flac".into(),
            bit_depth: Some(24),
            sample_rate_hz: Some(96000),
            ..Default::default()
        }
    }

    // ── Identity ───────────────────────────────────────────────────────────

    /// The floors must not overlap each other, Plex's (`1 << 40`) or the
    /// ephemeral store's (`1 << 48`). A predicate that answered "mine" for a
    /// neighbouring source would route a row to the wrong server.
    #[test]
    fn the_id_namespaces_do_not_collide_with_each_other_or_the_existing_ones() {
        const PLEX_FLOOR: i64 = 1 << 40;
        const EPHEMERAL_FLOOR: i64 = 1 << 48;

        let j = RemoteSource::Jellyfin.namespace(1);
        let s = RemoteSource::Subsonic.namespace(1);
        assert_eq!(RemoteSource::of_id(j), Some(RemoteSource::Jellyfin));
        assert_eq!(RemoteSource::of_id(s), Some(RemoteSource::Subsonic));
        assert_ne!(j, s);

        // Neither claims a Plex id, an ephemeral id, or a plain rowid.
        assert_eq!(RemoteSource::of_id(PLEX_FLOOR | 44_440), None);
        assert_eq!(RemoteSource::of_id(EPHEMERAL_FLOOR + 10), None);
        assert_eq!(RemoteSource::of_id(2954), None);
        assert_eq!(RemoteSource::of_id(0), None);

        // ...and the round trip is exact for the whole payload range.
        for rowid in [1i64, 2, 999, PAYLOAD_MASK] {
            for src in [RemoteSource::Jellyfin, RemoteSource::Subsonic] {
                let id = src.namespace(rowid);
                assert_eq!(RemoteSource::of_id(id), Some(src), "rowid {rowid}");
                assert_eq!(RemoteSource::rowid_of(id), rowid);
            }
        }
    }

    // ── Storage ────────────────────────────────────────────────────────────

    #[test]
    fn existing_cache_adds_the_collection_artwork_layer_in_place() {
        let c = db();
        c.execute(
            "ALTER TABLE remote_cache_tracks DROP COLUMN collection_artwork_token",
            [],
        )
        .unwrap();

        init_schema(&c).unwrap();
        let present = c
            .prepare("PRAGMA table_info(remote_cache_tracks)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|row| row.ok())
            .any(|column| column == "collection_artwork_token");
        assert!(present);
    }

    #[test]
    fn a_saved_track_round_trips_by_both_of_its_ids() {
        let mut c = db();
        let mut row = track("srv-1", "alb", Some(1), Some(3));
        row.artwork_token = Some("disc-cover".into());
        row.collection_artwork_token = Some("box-cover".into());
        save_tracks(&mut c, RemoteSource::Jellyfin, &[row]).unwrap();
        let by_item = track_by_item_id(&c, RemoteSource::Jellyfin, "srv-1")
            .unwrap()
            .expect("by item id");
        assert_eq!(by_item.title, "t-srv-1");
        assert_eq!(by_item.bit_depth, Some(24));
        assert_eq!(by_item.artwork_token.as_deref(), Some("disc-cover"));
        assert_eq!(
            by_item.collection_artwork_token.as_deref(),
            Some("box-cover")
        );
        assert_eq!(
            RemoteSource::of_id(by_item.id),
            Some(RemoteSource::Jellyfin)
        );

        let by_id = track_by_id(&c, by_item.id)
            .unwrap()
            .expect("by namespaced id");
        assert_eq!(by_id, by_item);
    }

    #[test]
    fn every_remote_genre_round_trips_in_server_order() {
        let mut c = db();
        let mut row = track("genres", "album", Some(1), Some(1));
        row.genre = Some("Progressive Rock".into());
        row.genres = vec![
            "Progressive Rock".into(),
            "Art Rock".into(),
            "Psychedelic".into(),
        ];
        save_tracks(&mut c, RemoteSource::Jellyfin, &[row]).unwrap();
        let stored = track_by_item_id(&c, RemoteSource::Jellyfin, "genres")
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.genres,
            ["Progressive Rock", "Art Rock", "Psychedelic"]
        );
        assert_eq!(stored.genre.as_deref(), Some("Progressive Rock"));
    }

    /// THE reason this is an UPSERT. A published row id lives inside queue
    /// entries and `session.db`; a re-scan that minted a new one would leave a
    /// playing track pointing at nothing.
    #[test]
    fn a_rescan_updates_a_row_without_changing_its_id() {
        let mut c = db();
        save_tracks(
            &mut c,
            RemoteSource::Jellyfin,
            &[track("srv-1", "alb", Some(1), Some(3))],
        )
        .unwrap();
        let first = track_by_item_id(&c, RemoteSource::Jellyfin, "srv-1")
            .unwrap()
            .unwrap();

        let mut renamed = track("srv-1", "alb", Some(1), Some(3));
        renamed.title = "retagged".into();
        renamed.bit_depth = Some(16);
        save_tracks(&mut c, RemoteSource::Jellyfin, &[renamed]).unwrap();

        let second = track_by_item_id(&c, RemoteSource::Jellyfin, "srv-1")
            .unwrap()
            .unwrap();
        assert_eq!(second.id, first.id, "the row id changed under a re-scan");
        assert_eq!(second.title, "retagged");
        assert_eq!(second.bit_depth, Some(16));
        assert_eq!(count(&c, RemoteSource::Jellyfin).unwrap(), 1);
    }

    /// The same `item_id` on two DIFFERENT servers is two different tracks.
    /// Subsonic and Jellyfin ids are opaque and can coincide.
    #[test]
    fn the_same_item_id_under_two_sources_is_two_rows() {
        let mut c = db();
        save_tracks(
            &mut c,
            RemoteSource::Jellyfin,
            &[track("same", "a", None, None)],
        )
        .unwrap();
        save_tracks(
            &mut c,
            RemoteSource::Subsonic,
            &[track("same", "a", None, None)],
        )
        .unwrap();
        assert_eq!(count(&c, RemoteSource::Jellyfin).unwrap(), 1);
        assert_eq!(count(&c, RemoteSource::Subsonic).unwrap(), 1);
        let j = track_by_item_id(&c, RemoteSource::Jellyfin, "same")
            .unwrap()
            .unwrap();
        let s = track_by_item_id(&c, RemoteSource::Subsonic, "same")
            .unwrap()
            .unwrap();
        assert_ne!(j.id, s.id);
        assert_eq!(RemoteSource::of_id(j.id), Some(RemoteSource::Jellyfin));
        assert_eq!(RemoteSource::of_id(s.id), Some(RemoteSource::Subsonic));
    }

    /// Untagged rows sort LAST. 75 of 4924 measured Jellyfin rows have no track
    /// number; leading with them would put them above track 1 on every album.
    #[test]
    fn album_order_puts_untagged_tracks_after_the_numbered_ones() {
        let mut c = db();
        save_tracks(
            &mut c,
            RemoteSource::Jellyfin,
            &[
                track("c", "alb", Some(1), None),
                track("b", "alb", Some(1), Some(2)),
                track("a", "alb", Some(1), Some(1)),
                track("d", "alb", Some(2), Some(1)),
            ],
        )
        .unwrap();
        let got: Vec<String> = album_tracks(&c, RemoteSource::Jellyfin, "alb")
            .unwrap()
            .into_iter()
            .map(|t| t.item_id)
            .collect();
        assert_eq!(got, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn search_matches_every_display_field_and_an_empty_needle_matches_all() {
        let mut c = db();
        let mut t = track("x", "Kind of Blue", Some(1), Some(1));
        t.artist = "Miles Davis".into();
        t.album_artist = "Miles Davis".into();
        t.title = "So What".into();
        save_tracks(&mut c, RemoteSource::Subsonic, &[t]).unwrap();

        for needle in ["So What", "miles", "Kind of", "so wh"] {
            assert_eq!(
                search(&c, RemoteSource::Subsonic, needle, None)
                    .unwrap()
                    .len(),
                1,
                "needle {needle:?} found nothing"
            );
        }
        assert_eq!(
            search(&c, RemoteSource::Subsonic, "", None).unwrap().len(),
            1
        );
        assert!(search(&c, RemoteSource::Subsonic, "zzz", None)
            .unwrap()
            .is_empty());
        // A search never leaks another source's rows.
        assert!(search(&c, RemoteSource::Jellyfin, "", None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn remote_search_pages_cross_the_window_without_duplicates() {
        let mut c = db();
        let rows: Vec<CachedTrack> = (0..1_201)
            .map(|i| {
                let mut t = track(
                    &format!("remote-{i:04}"),
                    &format!("Album {:03}", i / 12),
                    Some(1),
                    Some((i % 12 + 1) as u32),
                );
                t.title = format!("Track {i:04}");
                t
            })
            .collect();
        save_tracks(&mut c, RemoteSource::Jellyfin, &rows).unwrap();

        let mut offset = 0;
        let mut ids = std::collections::HashSet::new();
        loop {
            let page = search_page(
                &c,
                RemoteSource::Jellyfin,
                "Track",
                offset,
                500,
                "title-asc",
            )
            .unwrap();
            if page.is_empty() {
                break;
            }
            for row in &page {
                assert!(ids.insert(row.id), "duplicate id {}", row.id);
            }
            offset += page.len() as u64;
        }
        assert_eq!(ids.len(), 1_201);
        assert_eq!(offset, 1_201);
    }

    #[test]
    fn filtered_remote_pages_are_stable_and_do_not_leak_other_formats() {
        let mut c = db();
        let rows = (0..41)
            .map(|i| {
                let mut row = track(&format!("fmt-{i:02}"), "Album", Some(1), Some(i + 1));
                row.title = format!("Track {i:02}");
                if i % 2 == 0 {
                    row.container = "mp3".into();
                    row.codec = Some("mp3".into());
                    row.bit_depth = None;
                    row.sample_rate_hz = Some(44_100);
                }
                row
            })
            .collect::<Vec<_>>();
        save_tracks(&mut c, RemoteSource::Subsonic, &rows).unwrap();

        let formats = vec!["mp3".to_string()];
        let qualities = vec!["lossy".to_string()];
        let mut offset = 0;
        let mut ids = std::collections::HashSet::new();
        loop {
            let page = search_page_filtered(
                &c,
                RemoteSource::Subsonic,
                "",
                offset,
                7,
                "title-asc",
                &formats,
                false,
                &qualities,
            )
            .unwrap();
            if page.is_empty() {
                break;
            }
            for row in &page {
                assert_eq!(row.container, "mp3");
                assert!(ids.insert(row.id), "duplicate filtered id {}", row.id);
            }
            offset += page.len() as u64;
        }
        assert_eq!(ids.len(), 21);
    }

    #[test]
    fn remote_artist_group_uses_the_performing_artist() {
        let mut c = db();
        let mut zed = track("zed", "A Album", Some(1), Some(1));
        zed.artist = "Zed Performer".into();
        zed.album_artist = "Alpha Album Artist".into();
        let mut alpha = track("alpha", "Z Album", Some(1), Some(1));
        alpha.artist = "Alpha Performer".into();
        alpha.album_artist = "Zed Album Artist".into();
        save_tracks(&mut c, RemoteSource::Jellyfin, &[zed, alpha]).unwrap();

        let grouped = search_page(&c, RemoteSource::Jellyfin, "", 0, 10, "group-artist").unwrap();
        assert_eq!(grouped[0].artist, "Alpha Performer");

        let album_artist_sorted =
            search_page(&c, RemoteSource::Jellyfin, "", 0, 10, "artist-asc").unwrap();
        assert_eq!(album_artist_sorted[0].artist, "Zed Performer");
    }

    #[test]
    fn remote_only_artist_has_exact_album_and_track_counts() {
        let mut c = db();
        let mut rows = Vec::new();
        for (id, album) in [("one", "a"), ("two", "a"), ("three", "b")] {
            let mut t = track(id, album, Some(1), Some(1));
            t.artist = "Remote Only".into();
            t.album_artist = "Remote Only".into();
            rows.push(t);
        }
        save_tracks(&mut c, RemoteSource::Subsonic, &rows).unwrap();

        let got = artists(&c, RemoteSource::Subsonic).unwrap();
        assert_eq!(
            got,
            vec![CachedArtist {
                name: "Remote Only".into(),
                album_count: 2,
                track_count: 3,
            }]
        );
        assert!(artists(&c, RemoteSource::Jellyfin).unwrap().is_empty());
    }

    #[test]
    fn clearing_one_source_leaves_the_other_alone() {
        let mut c = db();
        save_tracks(
            &mut c,
            RemoteSource::Jellyfin,
            &[track("j", "a", None, None)],
        )
        .unwrap();
        save_tracks(
            &mut c,
            RemoteSource::Subsonic,
            &[track("s", "a", None, None)],
        )
        .unwrap();
        assert_eq!(clear(&mut c, RemoteSource::Jellyfin).unwrap(), 1);
        assert_eq!(count(&c, RemoteSource::Jellyfin).unwrap(), 0);
        assert_eq!(count(&c, RemoteSource::Subsonic).unwrap(), 1);
    }

    /// A row deleted on the server disappears; a row the sweep re-saw survives.
    #[test]
    fn prune_drops_only_what_the_sweep_did_not_touch() {
        let mut c = db();
        save_tracks(
            &mut c,
            RemoteSource::Subsonic,
            &[
                track("keep", "a", None, None),
                track("gone", "a", None, None),
            ],
        )
        .unwrap();
        // A later sweep re-sees only `keep`. `updated_at` is whole seconds, so
        // the timestamps are set explicitly rather than raced against the clock.
        c.execute(
            "UPDATE remote_cache_tracks SET updated_at = 100 WHERE item_id = 'gone'",
            [],
        )
        .unwrap();
        c.execute(
            "UPDATE remote_cache_tracks SET updated_at = 200 WHERE item_id = 'keep'",
            [],
        )
        .unwrap();
        assert_eq!(prune_stale(&mut c, RemoteSource::Subsonic, 150).unwrap(), 1);
        assert_eq!(count(&c, RemoteSource::Subsonic).unwrap(), 1);
        assert!(track_by_item_id(&c, RemoteSource::Subsonic, "keep")
            .unwrap()
            .is_some());
        assert!(track_by_item_id(&c, RemoteSource::Subsonic, "gone")
            .unwrap()
            .is_none());
    }

    /// AUTOINCREMENT, not plain rowid: a freed id must never be handed to a
    /// different track, because the old one may still be sitting in a queue.
    #[test]
    fn a_deleted_rows_id_is_never_reused() {
        let mut c = db();
        save_tracks(
            &mut c,
            RemoteSource::Jellyfin,
            &[track("first", "a", None, None)],
        )
        .unwrap();
        let first = track_by_item_id(&c, RemoteSource::Jellyfin, "first")
            .unwrap()
            .unwrap();
        clear(&mut c, RemoteSource::Jellyfin).unwrap();
        save_tracks(
            &mut c,
            RemoteSource::Jellyfin,
            &[track("second", "a", None, None)],
        )
        .unwrap();
        let second = track_by_item_id(&c, RemoteSource::Jellyfin, "second")
            .unwrap()
            .unwrap();
        assert_ne!(
            second.id, first.id,
            "the deleted track's id was recycled — a stale queue entry would now play this row"
        );
    }

    #[test]
    fn libraries_round_trip_per_source() {
        let mut c = db();
        save_libraries(
            &mut c,
            RemoteSource::Jellyfin,
            &[CachedLibrary {
                source: "jellyfin".into(),
                library_id: "lib1".into(),
                name: "Music".into(),
                server_id: "srv".into(),
            }],
        )
        .unwrap();
        let got = libraries(&c, RemoteSource::Jellyfin).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Music");
        assert!(libraries(&c, RemoteSource::Subsonic).unwrap().is_empty());
    }

    #[test]
    fn a_lossy_row_keeps_its_absent_quality_through_the_database() {
        let mut c = db();
        let mut t = track("mp3", "a", None, None);
        t.bit_depth = None;
        t.sample_rate_hz = Some(44100);
        t.container = "mp3".into();
        save_tracks(&mut c, RemoteSource::Subsonic, &[t]).unwrap();
        let got = track_by_item_id(&c, RemoteSource::Subsonic, "mp3")
            .unwrap()
            .unwrap();
        assert_eq!(got.bit_depth, None, "NULL came back as something else");
        assert_eq!(got.sample_rate_hz, Some(44100));
    }

    #[test]
    fn essential_generation_preserves_quality_and_prunes_only_on_completion() {
        let mut c = db();
        let mut keep = track("keep", "album", Some(1), Some(1));
        keep.title = "Old title".into();
        keep.container = "flac".into();
        keep.codec = Some("flac".into());
        keep.bit_depth = Some(24);
        keep.sample_rate_hz = Some(96_000);
        let gone = track("gone", "album", Some(1), Some(2));
        save_tracks(&mut c, RemoteSource::Jellyfin, &[keep.clone(), gone]).unwrap();

        let first = begin_source_sync(&mut c, RemoteSource::Jellyfin).unwrap();
        let mut essential = keep.clone();
        essential.title = "Fresh title".into();
        essential.codec = None;
        essential.bit_depth = None;
        essential.sample_rate_hz = None;
        save_essential_tracks(
            &mut c,
            RemoteSource::Jellyfin,
            first.generation,
            &[essential],
        )
        .unwrap();
        assert_eq!(count(&c, RemoteSource::Jellyfin).unwrap(), 2);
        let visible = track_by_item_id(&c, RemoteSource::Jellyfin, "keep")
            .unwrap()
            .unwrap();
        assert_eq!(visible.title, "Fresh title");
        assert_eq!(visible.bit_depth, Some(24));
        assert_eq!(visible.sample_rate_hz, Some(96_000));
        assert_eq!(
            quality_candidates(&c, RemoteSource::Jellyfin, 10).unwrap(),
            vec!["keep".to_string()]
        );

        interrupt_source_sync(&c, RemoteSource::Jellyfin, first.generation).unwrap();
        assert_eq!(count(&c, RemoteSource::Jellyfin).unwrap(), 2);
        let second = begin_source_sync(&mut c, RemoteSource::Jellyfin).unwrap();
        save_essential_tracks(&mut c, RemoteSource::Jellyfin, second.generation, &[keep]).unwrap();
        assert_eq!(
            complete_source_sync(&mut c, RemoteSource::Jellyfin, second.generation, true,).unwrap(),
            1
        );
        assert!(track_by_item_id(&c, RemoteSource::Jellyfin, "gone")
            .unwrap()
            .is_none());
    }

    #[test]
    fn quality_hydration_is_keyed_by_id_and_clears_stale_lossless_values() {
        let mut c = db();
        let mut row = track("changed", "album", Some(1), Some(1));
        row.container = "flac".into();
        row.codec = Some("flac".into());
        row.bit_depth = Some(24);
        row.sample_rate_hz = Some(192_000);
        save_tracks(&mut c, RemoteSource::Jellyfin, &[row.clone()]).unwrap();
        let generation = begin_source_sync(&mut c, RemoteSource::Jellyfin)
            .unwrap()
            .generation;
        save_essential_tracks(&mut c, RemoteSource::Jellyfin, generation, &[row]).unwrap();

        let before_revision: i64 = c
            .query_row(
                "SELECT updated_at FROM remote_cache_tracks WHERE item_id='changed'",
                [],
                |record| record.get(0),
            )
            .unwrap();
        assert_eq!(
            pending_quality_ids(
                &c,
                RemoteSource::Jellyfin,
                &["missing".into(), "changed".into()],
            )
            .unwrap(),
            vec!["changed".to_string()]
        );
        update_track_quality(
            &mut c,
            RemoteSource::Jellyfin,
            &[CachedTrackQuality {
                item_id: "changed".into(),
                container: "mp3".into(),
                codec: Some("mp3".into()),
                bit_depth: None,
                sample_rate_hz: Some(44_100),
                channels: Some(2),
                bitrate_kbps: Some(320),
            }],
        )
        .unwrap();
        let hydrated = track_by_item_id(&c, RemoteSource::Jellyfin, "changed")
            .unwrap()
            .unwrap();
        assert_eq!(hydrated.container, "mp3");
        assert_eq!(hydrated.bit_depth, None);
        assert_eq!(hydrated.sample_rate_hz, Some(44_100));
        assert!(quality_candidates(&c, RemoteSource::Jellyfin, 10)
            .unwrap()
            .is_empty());
        let after_revision: i64 = c
            .query_row(
                "SELECT updated_at FROM remote_cache_tracks WHERE item_id='changed'",
                [],
                |record| record.get(0),
            )
            .unwrap();
        assert!(after_revision > before_revision);
    }

    #[test]
    fn completed_delta_generation_keeps_unobserved_rows() {
        let mut c = db();
        let keep = track("changed", "album", Some(1), Some(1));
        let untouched = track("unchanged", "album", Some(1), Some(2));
        save_tracks(&mut c, RemoteSource::Jellyfin, &[keep.clone(), untouched]).unwrap();
        let generation = begin_source_sync(&mut c, RemoteSource::Jellyfin)
            .unwrap()
            .generation;
        save_essential_tracks(&mut c, RemoteSource::Jellyfin, generation, &[keep]).unwrap();
        assert_eq!(
            complete_source_sync(&mut c, RemoteSource::Jellyfin, generation, false,).unwrap(),
            0
        );
        assert_eq!(count(&c, RemoteSource::Jellyfin).unwrap(), 2);
        assert!(track_by_item_id(&c, RemoteSource::Jellyfin, "unchanged")
            .unwrap()
            .is_some());
    }

    /// Jellyfin can remint every item id after a library rebuild while the
    /// audio and album metadata remain identical. A full user-requested
    /// reconciliation must treat the newly observed identity set as
    /// authoritative; otherwise every physical track appears twice forever.
    #[test]
    fn full_reconciliation_replaces_a_completely_remapped_identity_set() {
        let mut c = db();
        let mut old_one = track("old-one", "same album", Some(1), Some(1));
        old_one.title = "First".into();
        let mut old_two = track("old-two", "same album", Some(1), Some(2));
        old_two.title = "Second".into();
        save_tracks(
            &mut c,
            RemoteSource::Jellyfin,
            &[old_one.clone(), old_two.clone()],
        )
        .unwrap();

        let generation = begin_source_sync(&mut c, RemoteSource::Jellyfin)
            .unwrap()
            .generation;
        let mut new_one = old_one;
        new_one.item_id = "new-one".into();
        let mut new_two = old_two;
        new_two.item_id = "new-two".into();
        save_essential_tracks(
            &mut c,
            RemoteSource::Jellyfin,
            generation,
            &[new_one, new_two],
        )
        .unwrap();

        assert_eq!(count(&c, RemoteSource::Jellyfin).unwrap(), 4);
        assert_eq!(
            complete_source_sync(&mut c, RemoteSource::Jellyfin, generation, true).unwrap(),
            2
        );
        assert_eq!(count(&c, RemoteSource::Jellyfin).unwrap(), 2);
        assert!(track_by_item_id(&c, RemoteSource::Jellyfin, "old-one")
            .unwrap()
            .is_none());
        assert!(track_by_item_id(&c, RemoteSource::Jellyfin, "old-two")
            .unwrap()
            .is_none());
        assert!(track_by_item_id(&c, RemoteSource::Jellyfin, "new-one")
            .unwrap()
            .is_some());
        assert!(track_by_item_id(&c, RemoteSource::Jellyfin, "new-two")
            .unwrap()
            .is_some());
    }

    #[test]
    fn duplicate_item_across_essential_pages_rolls_back_the_second_page() {
        let mut c = db();
        let generation = begin_source_sync(&mut c, RemoteSource::Jellyfin)
            .unwrap()
            .generation;
        let first = track("same", "album", Some(1), Some(1));
        save_essential_tracks(&mut c, RemoteSource::Jellyfin, generation, &[first.clone()])
            .unwrap();
        let mut duplicate = first;
        duplicate.title = "Wrong duplicate".into();
        assert!(
            save_essential_tracks(&mut c, RemoteSource::Jellyfin, generation, &[duplicate],)
                .is_err()
        );
        assert_eq!(
            track_by_item_id(&c, RemoteSource::Jellyfin, "same")
                .unwrap()
                .unwrap()
                .title,
            "t-same"
        );
    }

    #[test]
    fn complete_quality_generation_is_atomic_and_prunes_only_after_completion() {
        let mut c = db();
        let mut keep = track("keep", "album", Some(1), Some(1));
        keep.container = "flac".into();
        keep.codec = Some("flac".into());
        keep.bit_depth = Some(24);
        keep.sample_rate_hz = Some(96_000);
        let gone = track("gone", "album", Some(1), Some(2));
        save_tracks(&mut c, RemoteSource::Subsonic, &[keep.clone(), gone]).unwrap();

        let first = begin_source_sync(&mut c, RemoteSource::Subsonic).unwrap();
        keep.container = "mp3".into();
        keep.codec = Some("audio/mpeg".into());
        keep.bit_depth = None;
        keep.sample_rate_hz = Some(44_100);
        save_generation_tracks(
            &mut c,
            RemoteSource::Subsonic,
            first.generation,
            &[keep.clone()],
        )
        .unwrap();
        let visible = track_by_item_id(&c, RemoteSource::Subsonic, "keep")
            .unwrap()
            .unwrap();
        assert_eq!(visible.container, "mp3");
        assert_eq!(visible.bit_depth, None, "lossy quality stayed stale");
        assert!(quality_candidates(&c, RemoteSource::Subsonic, 10)
            .unwrap()
            .is_empty());
        interrupt_source_sync(&c, RemoteSource::Subsonic, first.generation).unwrap();
        assert_eq!(count(&c, RemoteSource::Subsonic).unwrap(), 2);

        let second = begin_source_sync(&mut c, RemoteSource::Subsonic).unwrap();
        save_generation_tracks(&mut c, RemoteSource::Subsonic, second.generation, &[keep]).unwrap();
        assert_eq!(
            complete_source_sync(&mut c, RemoteSource::Subsonic, second.generation, true).unwrap(),
            1
        );
        assert!(track_by_item_id(&c, RemoteSource::Subsonic, "gone")
            .unwrap()
            .is_none());
    }

    #[test]
    fn shared_cache_writes_are_serialized_across_connections() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote-media.db");
        let _first = open(&path).unwrap();
        let mut second = open(&path).unwrap();
        let writer = lock_writer();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx
                .send(save_tracks(
                    &mut second,
                    RemoteSource::Subsonic,
                    &[track("serialized", "album", Some(1), Some(1))],
                ))
                .unwrap();
        });

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "a second cache connection wrote while the writer lock was held"
        );
        drop(writer);
        assert_eq!(
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            1
        );
        worker.join().unwrap();
    }
}
