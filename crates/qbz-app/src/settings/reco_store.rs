//! Recommendation event store (headless, frontend-agnostic).
//!
//! Cleanroom port (ADR-006) of Tauri's `src-tauri/src/reco_store/{mod,db,helpers}.rs`
//! into `qbz-app` so the Slint frontend (and, eventually, Tauri) can drive the
//! Discover recommendation seeds without any `tauri::State` dependency. This
//! module does NOT wrap legacy — it owns the logic and runs headless.
//!
//! ## DB file shared with Tauri
//!
//! Tauri's `RecoState::init_at` opens `<base_dir>/reco/events.db`
//! (`src-tauri/src/reco_store/mod.rs:162-167`; the non-per-user path is
//! `dirs::data_dir()/qbz/reco/events.db`, see `mod.rs:140-148`). To share a
//! user's existing Tauri recommendation history cross-frontend, `new_at(base)`
//! opens the SAME file: `<base>/reco/events.db`. The schema is created with
//! `CREATE TABLE IF NOT EXISTS` (+ idempotent column/index migrations), so the
//! store coexists with a DB that Tauri already created.
//!
//! ## What is ported
//!
//! - `reco_events` schema + the `genre_id` migration (Tauri's base schema omits
//!   `genre_id` and adds it via `ALTER TABLE`; we create it inline AND keep the
//!   idempotent migration so an old Tauri DB without the column is upgraded).
//! - `reco_scores` companion table (written by `train()`, read by `get_home_seeds`).
//! - `reco_album_meta` (needed by `get_top_genres`, which LEFT JOINs it for the
//!   genre name).
//! - Event logging (`log_play_event` / `log_favorite_event` / generic `insert_event`).
//! - Read APIs: `get_recent_track_ids`, `get_recent_track_ids_since` (NEW —
//!   time-windowed, for WeeklyQ's 7-day window), `get_favorite_track_ids`,
//!   `get_top_genres`, `get_home_seeds` (mirrors `get_home_seeds_internal`).
//! - `train()` — the decay/weight scorer from Tauri's `v2_reco_train_scores`,
//!   ported verbatim (same default lookback 90d / half-life 21d / max 5000
//!   events / 200 per type, same event + item weights, same exponential decay).
//!
//! Album/artist *metadata resolution* (the 3-tier Qobuz-API cache in Tauri's
//! `helpers.rs`) is intentionally NOT ported here: it depends on the Qobuz HTTP
//! client + API cache and belongs in the frontend layer that has those. This
//! module returns IDs (seeds); the caller resolves them.

use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Event / item types (mirror src-tauri/src/reco_store/mod.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoEventType {
    Play,
    Favorite,
    PlaylistAdd,
}

impl RecoEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Favorite => "favorite",
            Self::PlaylistAdd => "playlist_add",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoItemType {
    Track,
    Album,
    Artist,
}

impl RecoItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Artist => "artist",
        }
    }
}

/// A single recommendation event to persist (mirrors `RecoEventInput`).
#[derive(Debug, Clone)]
pub struct RecoEventInput {
    pub event_type: RecoEventType,
    pub item_type: RecoItemType,
    pub track_id: Option<u64>,
    pub album_id: Option<String>,
    pub artist_id: Option<u64>,
    pub playlist_id: Option<u64>,
    pub genre_id: Option<u64>,
}

/// A top-artist seed (mirrors `TopArtistSeed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopArtistSeed {
    pub artist_id: u64,
    pub play_count: u32,
}

/// The ID seeds for the home/Discover recommendation rows (mirrors `HomeSeeds`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HomeSeeds {
    pub recently_played_album_ids: Vec<String>,
    pub continue_listening_track_ids: Vec<u64>,
    pub top_artist_ids: Vec<TopArtistSeed>,
    pub favorite_album_ids: Vec<String>,
    pub favorite_track_ids: Vec<u64>,
}

/// Limits for a `get_home_seeds` call (mirrors the four `v2_reco_get_home*` args).
#[derive(Debug, Clone, Copy)]
pub struct HomeSeedLimits {
    pub recent_albums: u32,
    pub continue_tracks: u32,
    pub top_artists: u32,
    pub favorites: u32,
}

impl Default for HomeSeedLimits {
    fn default() -> Self {
        // Same defaults as Tauri's v2_reco_get_home commands.
        Self {
            recent_albums: 12,
            continue_tracks: 10,
            top_artists: 10,
            favorites: 12,
        }
    }
}

/// Parameters for `train()` (mirrors `v2_reco_train_scores` args + defaults).
#[derive(Debug, Clone, Copy)]
pub struct TrainParams {
    pub lookback_days: i64,
    pub half_life_days: f64,
    pub max_events: u32,
    pub max_per_type: u32,
}

impl Default for TrainParams {
    fn default() -> Self {
        Self {
            lookback_days: 90,
            half_life_days: 21.0,
            max_events: 5000,
            max_per_type: 200,
        }
    }
}

/// A decoded event row (mirrors `RecoEventRecord`).
#[derive(Debug, Clone)]
struct RecoEventRecord {
    event_type: String,
    item_type: String,
    track_id: Option<u64>,
    album_id: Option<String>,
    artist_id: Option<u64>,
    #[allow(dead_code)]
    genre_id: Option<u64>,
    created_at: i64,
}

/// A scored entry to write into `reco_scores` (mirrors `RecoScoreEntry`).
#[derive(Debug, Clone)]
struct RecoScoreEntry {
    track_id: Option<u64>,
    album_id: Option<String>,
    artist_id: Option<u64>,
    score: f64,
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct RecoStore {
    conn: Connection,
}

impl RecoStore {
    fn open_at(reco_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(reco_dir)
            .map_err(|e| format!("Failed to create reco directory: {}", e))?;

        // Same filename as Tauri (src-tauri/src/reco_store/mod.rs:166): events.db
        let db_path = reco_dir.join("events.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open reco database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to enable WAL for reco database: {}", e))?;

        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    /// Default (non per-user) location: `dirs::data_dir()/qbz/reco/events.db`,
    /// matching Tauri's `RecoState::new` (mod.rs:140-148).
    pub fn new() -> Result<Self, String> {
        let reco_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz")
            .join("reco");
        Self::open_at(&reco_dir)
    }

    /// Per-user location: `<base_dir>/reco/events.db`, matching Tauri's
    /// `RecoState::init_at` (mod.rs:162-167). Shares the user's existing
    /// Tauri reco history.
    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(&base_dir.join("reco"))
    }

    /// Idempotent schema creation. Matches Tauri's `RecoStoreDb::init` exactly,
    /// except `genre_id` is included inline in the base `reco_events` table (the
    /// migration is still run so an OLD Tauri DB without the column is upgraded).
    fn init(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS reco_events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    event_type TEXT NOT NULL,
                    item_type TEXT NOT NULL,
                    track_id INTEGER,
                    album_id TEXT,
                    artist_id INTEGER,
                    playlist_id INTEGER,
                    genre_id INTEGER,
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_reco_events_type ON reco_events(event_type);
                CREATE INDEX IF NOT EXISTS idx_reco_events_track ON reco_events(track_id);
                CREATE INDEX IF NOT EXISTS idx_reco_events_album ON reco_events(album_id);
                CREATE INDEX IF NOT EXISTS idx_reco_events_artist ON reco_events(artist_id);
                CREATE INDEX IF NOT EXISTS idx_reco_events_created ON reco_events(created_at);
                CREATE INDEX IF NOT EXISTS idx_reco_events_genre ON reco_events(genre_id);

                CREATE INDEX IF NOT EXISTS idx_reco_events_play_albums
                    ON reco_events(event_type, album_id, created_at DESC)
                    WHERE album_id IS NOT NULL;
                CREATE INDEX IF NOT EXISTS idx_reco_events_play_tracks
                    ON reco_events(event_type, track_id, created_at DESC)
                    WHERE track_id IS NOT NULL;
                CREATE INDEX IF NOT EXISTS idx_reco_events_play_artists
                    ON reco_events(event_type, artist_id, created_at DESC)
                    WHERE artist_id IS NOT NULL;

                CREATE TABLE IF NOT EXISTS reco_scores (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    score_type TEXT NOT NULL,
                    item_type TEXT NOT NULL,
                    track_id INTEGER,
                    album_id TEXT,
                    artist_id INTEGER,
                    score REAL NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_reco_scores_type ON reco_scores(score_type);
                CREATE INDEX IF NOT EXISTS idx_reco_scores_item ON reco_scores(item_type);
                CREATE INDEX IF NOT EXISTS idx_reco_scores_track ON reco_scores(track_id);
                CREATE INDEX IF NOT EXISTS idx_reco_scores_album ON reco_scores(album_id);
                CREATE INDEX IF NOT EXISTS idx_reco_scores_artist ON reco_scores(artist_id);
                CREATE INDEX IF NOT EXISTS idx_reco_scores_lookup
                    ON reco_scores(score_type, item_type, score DESC);

                CREATE TABLE IF NOT EXISTS reco_album_meta (
                    album_id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    artist_name TEXT NOT NULL,
                    artist_id INTEGER,
                    artwork_url TEXT NOT NULL DEFAULT '',
                    genre_name TEXT NOT NULL DEFAULT '',
                    quality TEXT NOT NULL DEFAULT '',
                    release_date TEXT,
                    updated_at INTEGER NOT NULL
                );
                "#,
            )
            .map_err(|e| format!("Failed to initialize reco database: {}", e))?;

        // Upgrade an old Tauri DB whose base schema predates the genre_id column.
        self.migrate_add_genre_id()?;

        Ok(())
    }

    /// Idempotent: add `genre_id` (+ its index) if an old Tauri DB lacks it.
    fn migrate_add_genre_id(&self) -> Result<(), String> {
        let has_column: bool = self
            .conn
            .prepare("PRAGMA table_info(reco_events)")
            .map_err(|e| format!("Failed to query table info: {}", e))?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Failed to read table info: {}", e))?
            .filter_map(Result::ok)
            .any(|col| col == "genre_id");

        if !has_column {
            self.conn
                .execute("ALTER TABLE reco_events ADD COLUMN genre_id INTEGER", [])
                .map_err(|e| format!("Failed to add genre_id column: {}", e))?;
            self.conn
                .execute(
                    "CREATE INDEX IF NOT EXISTS idx_reco_events_genre ON reco_events(genre_id)",
                    [],
                )
                .map_err(|e| format!("Failed to create genre_id index: {}", e))?;
        }
        Ok(())
    }

    // ---- Event logging ----

    /// Generic insert (mirrors `RecoStoreDb::insert_event`).
    pub fn insert_event(&self, event: &RecoEventInput) -> Result<(), String> {
        self.conn
            .execute(
                r#"
                INSERT INTO reco_events (
                    event_type, item_type, track_id, album_id,
                    artist_id, playlist_id, genre_id, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
                params![
                    event.event_type.as_str(),
                    event.item_type.as_str(),
                    event.track_id.map(|v| v as i64),
                    event.album_id.as_deref(),
                    event.artist_id.map(|v| v as i64),
                    event.playlist_id.map(|v| v as i64),
                    event.genre_id.map(|v| v as i64),
                    now_ts(),
                ],
            )
            .map_err(|e| format!("Failed to insert reco event: {}", e))?;
        Ok(())
    }

    /// Log a track play (event_type=play, item_type=track). Captures
    /// track_id + artist_id + genre_id + occurred_at (now).
    pub fn log_play_event(
        &self,
        track_id: u64,
        album_id: Option<String>,
        artist_id: Option<u64>,
        genre_id: Option<u64>,
    ) -> Result<(), String> {
        self.insert_event(&RecoEventInput {
            event_type: RecoEventType::Play,
            item_type: RecoItemType::Track,
            track_id: Some(track_id),
            album_id,
            artist_id,
            playlist_id: None,
            genre_id,
        })
    }

    /// Log a track favorite (event_type=favorite, item_type=track).
    pub fn log_favorite_event(
        &self,
        track_id: u64,
        album_id: Option<String>,
        artist_id: Option<u64>,
        genre_id: Option<u64>,
    ) -> Result<(), String> {
        self.insert_event(&RecoEventInput {
            event_type: RecoEventType::Favorite,
            item_type: RecoItemType::Track,
            track_id: Some(track_id),
            album_id,
            artist_id,
            playlist_id: None,
            genre_id,
        })
    }

    // ---- Read APIs ----

    /// Most-recently-played distinct track IDs (mirrors `get_recent_track_ids`).
    pub fn get_recent_track_ids(&self, limit: u32) -> Result<Vec<u64>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT track_id, MAX(created_at) AS last_played
                FROM reco_events
                WHERE event_type = 'play' AND track_id IS NOT NULL
                GROUP BY track_id
                ORDER BY last_played DESC
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare recent tracks query: {}", e))?;

        let rows = stmt
            .query_map(params![limit], |row| row.get::<_, i64>(0).map(|v| v as u64))
            .map_err(|e| format!("Failed to query recent tracks: {}", e))?;

        let mut tracks = Vec::new();
        for row in rows {
            tracks.push(row.map_err(|e| format!("Failed to read recent track row: {}", e))?);
        }
        Ok(tracks)
    }

    /// NEW: time-windowed recent track IDs — distinct play tracks whose most
    /// recent play is within the last `window_secs` seconds, newest first,
    /// capped at `limit`. Backs WeeklyQ's 7-day window (window_secs = 7*86400).
    pub fn get_recent_track_ids_since(
        &self,
        window_secs: i64,
        limit: u32,
    ) -> Result<Vec<u64>, String> {
        let since_ts = now_ts().saturating_sub(window_secs.max(0));
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT track_id, MAX(created_at) AS last_played
                FROM reco_events
                WHERE event_type = 'play' AND track_id IS NOT NULL AND created_at >= ?
                GROUP BY track_id
                ORDER BY last_played DESC
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare windowed recent tracks query: {}", e))?;

        let rows = stmt
            .query_map(params![since_ts, limit], |row| row.get::<_, i64>(0).map(|v| v as u64))
            .map_err(|e| format!("Failed to query windowed recent tracks: {}", e))?;

        let mut tracks = Vec::new();
        for row in rows {
            tracks
                .push(row.map_err(|e| format!("Failed to read windowed recent track row: {}", e))?);
        }
        Ok(tracks)
    }

    /// Most-recently-favorited distinct track IDs (mirrors `get_favorite_track_ids`).
    pub fn get_favorite_track_ids(&self, limit: u32) -> Result<Vec<u64>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT track_id, MAX(created_at) AS last_favorite
                FROM reco_events
                WHERE event_type = 'favorite' AND track_id IS NOT NULL
                GROUP BY track_id
                ORDER BY last_favorite DESC
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare favorite tracks query: {}", e))?;

        let rows = stmt
            .query_map(params![limit], |row| row.get::<_, i64>(0).map(|v| v as u64))
            .map_err(|e| format!("Failed to query favorite tracks: {}", e))?;

        let mut tracks = Vec::new();
        for row in rows {
            tracks.push(row.map_err(|e| format!("Failed to read favorite track row: {}", e))?);
        }
        Ok(tracks)
    }

    fn get_recent_album_ids(&self, limit: u32) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT album_id, MAX(created_at) AS last_played
                FROM reco_events
                WHERE event_type = 'play' AND album_id IS NOT NULL
                GROUP BY album_id
                ORDER BY last_played DESC
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare recent albums query: {}", e))?;
        let rows = stmt
            .query_map(params![limit], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query recent albums: {}", e))?;
        let mut albums = Vec::new();
        for row in rows {
            albums.push(row.map_err(|e| format!("Failed to read recent album row: {}", e))?);
        }
        Ok(albums)
    }

    fn get_favorite_album_ids(&self, limit: u32) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT album_id, MAX(created_at) AS last_favorite
                FROM reco_events
                WHERE event_type = 'favorite' AND album_id IS NOT NULL
                GROUP BY album_id
                ORDER BY last_favorite DESC
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare favorite albums query: {}", e))?;
        let rows = stmt
            .query_map(params![limit], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query favorite albums: {}", e))?;
        let mut albums = Vec::new();
        for row in rows {
            albums.push(row.map_err(|e| format!("Failed to read favorite album row: {}", e))?);
        }
        Ok(albums)
    }

    fn get_top_artist_ids(&self, limit: u32) -> Result<Vec<TopArtistSeed>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT artist_id, COUNT(*) AS play_count, MAX(created_at) AS last_played
                FROM reco_events
                WHERE event_type = 'play' AND artist_id IS NOT NULL
                GROUP BY artist_id
                ORDER BY play_count DESC, last_played DESC
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare top artists query: {}", e))?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(TopArtistSeed {
                    artist_id: row.get::<_, i64>(0).map(|v| v as u64)?,
                    play_count: row.get::<_, u32>(1)?,
                })
            })
            .map_err(|e| format!("Failed to query top artists: {}", e))?;
        let mut artists = Vec::new();
        for row in rows {
            artists.push(row.map_err(|e| format!("Failed to read top artist row: {}", e))?);
        }
        Ok(artists)
    }

    /// Distinct artist ids the user "knows": played strictly more than
    /// `play_threshold` times OR favorited at least once. Backs the discovery
    /// "skip artists I already know" filter with a richer signal (plays +
    /// favorites) than the play-only `play_history` store.
    pub fn get_known_artist_ids(
        &self,
        play_threshold: u32,
    ) -> Result<std::collections::HashSet<u64>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT artist_id
                FROM reco_events
                WHERE artist_id IS NOT NULL
                GROUP BY artist_id
                HAVING SUM(CASE WHEN event_type = 'play' THEN 1 ELSE 0 END) > ?
                    OR SUM(CASE WHEN event_type = 'favorite' THEN 1 ELSE 0 END) > 0
                "#,
            )
            .map_err(|e| format!("Failed to prepare known artists query: {}", e))?;
        let rows = stmt
            .query_map(params![play_threshold], |row| row.get::<_, i64>(0).map(|v| v as u64))
            .map_err(|e| format!("Failed to query known artists: {}", e))?;
        let mut ids = std::collections::HashSet::new();
        for row in rows {
            ids.insert(row.map_err(|e| format!("Failed to read known artist row: {}", e))?);
        }
        Ok(ids)
    }

    /// The user's most-played genres by event count (mirrors `get_top_genre_ids`).
    /// Returns `(genre_id, genre_name)` — name from `reco_album_meta` (empty if unknown).
    pub fn get_top_genres(&self, limit: u32) -> Result<Vec<(u64, String)>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT e.genre_id, COALESCE(m.genre_name, ''), COUNT(*) AS play_count
                FROM reco_events e
                LEFT JOIN reco_album_meta m ON e.album_id = m.album_id
                WHERE e.genre_id IS NOT NULL AND e.genre_id > 0
                GROUP BY e.genre_id
                ORDER BY play_count DESC
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare top genres query: {}", e))?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok((row.get::<_, i64>(0).map(|v| v as u64)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Failed to query top genres: {}", e))?;
        let mut genres = Vec::new();
        for row in rows {
            genres.push(row.map_err(|e| format!("Failed to read genre row: {}", e))?);
        }
        Ok(genres)
    }

    /// Favorite album ids NOT played within `recency_days` ("forgotten
    /// favorites"). Ported from Tauri `reco_store/db.rs:391-426`, adapted to
    /// the integer `created_at` (unix seconds) schema: Tauri's
    /// `datetime('now', …)` string comparison never matched an integer column,
    /// so the recency filter is a numeric cutoff here.
    pub fn get_forgotten_favorite_album_ids(
        &self,
        limit: u32,
        recency_days: u32,
    ) -> Result<Vec<String>, String> {
        let cutoff = now_ts().saturating_sub((recency_days as i64) * 86_400);
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT f.album_id
                FROM reco_events f
                WHERE f.event_type = 'favorite' AND f.album_id IS NOT NULL
                  AND f.album_id NOT IN (
                    SELECT DISTINCT p.album_id
                    FROM reco_events p
                    WHERE p.event_type = 'play'
                      AND p.album_id IS NOT NULL
                      AND p.created_at > ?
                  )
                GROUP BY f.album_id
                ORDER BY RANDOM()
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare forgotten favorites query: {}", e))?;
        let rows = stmt
            .query_map(params![cutoff, limit], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query forgotten favorites: {}", e))?;
        let mut albums = Vec::new();
        for row in rows {
            albums
                .push(row.map_err(|e| format!("Failed to read forgotten favorite row: {}", e))?);
        }
        Ok(albums)
    }

    // ---- Scores (companion table, written by train()) ----

    fn has_scores(&self, score_type: &str) -> Result<bool, String> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM reco_scores WHERE score_type = ?",
                params![score_type],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to query reco scores count: {}", e))?;
        Ok(count > 0)
    }

    fn get_scored_album_ids(&self, score_type: &str, limit: u32) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT album_id FROM reco_scores
                WHERE score_type = ? AND item_type = 'album' AND album_id IS NOT NULL
                ORDER BY score DESC LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare scored albums query: {}", e))?;
        let rows = stmt
            .query_map(params![score_type, limit], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query scored albums: {}", e))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Failed to read scored album row: {}", e))?);
        }
        Ok(out)
    }

    fn get_scored_track_ids(&self, score_type: &str, limit: u32) -> Result<Vec<u64>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT track_id FROM reco_scores
                WHERE score_type = ? AND item_type = 'track' AND track_id IS NOT NULL
                ORDER BY score DESC LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare scored tracks query: {}", e))?;
        let rows = stmt
            .query_map(params![score_type, limit], |row| row.get::<_, i64>(0).map(|v| v as u64))
            .map_err(|e| format!("Failed to query scored tracks: {}", e))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Failed to read scored track row: {}", e))?);
        }
        Ok(out)
    }

    fn get_scored_artist_scores(
        &self,
        score_type: &str,
        limit: u32,
    ) -> Result<Vec<(u64, f64)>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT artist_id, score FROM reco_scores
                WHERE score_type = ? AND item_type = 'artist' AND artist_id IS NOT NULL
                ORDER BY score DESC LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare scored artists query: {}", e))?;
        let rows = stmt
            .query_map(params![score_type, limit], |row| {
                Ok((row.get::<_, i64>(0).map(|v| v as u64)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| format!("Failed to query scored artists: {}", e))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Failed to read scored artist row: {}", e))?);
        }
        Ok(out)
    }

    fn get_events_since(
        &self,
        since_ts: i64,
        limit: u32,
    ) -> Result<Vec<RecoEventRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT event_type, item_type, track_id, album_id, artist_id, genre_id, created_at
                FROM reco_events
                WHERE created_at >= ?
                ORDER BY created_at DESC
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare reco events query: {}", e))?;
        let rows = stmt
            .query_map(params![since_ts, limit], |row| {
                Ok(RecoEventRecord {
                    event_type: row.get(0)?,
                    item_type: row.get(1)?,
                    track_id: row.get::<_, Option<i64>>(2)?.map(|v| v as u64),
                    album_id: row.get(3)?,
                    artist_id: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                    genre_id: row.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                    created_at: row.get(6)?,
                })
            })
            .map_err(|e| format!("Failed to query reco events: {}", e))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|e| format!("Failed to read reco event row: {}", e))?);
        }
        Ok(events)
    }

    fn replace_scores(
        &mut self,
        score_type: &str,
        item_type: &str,
        entries: &[RecoScoreEntry],
    ) -> Result<(), String> {
        let updated_at = now_ts();
        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("Failed to start reco scores transaction: {}", e))?;

        tx.execute(
            "DELETE FROM reco_scores WHERE score_type = ? AND item_type = ?",
            params![score_type, item_type],
        )
        .map_err(|e| format!("Failed to clear reco scores: {}", e))?;

        if !entries.is_empty() {
            let mut stmt = tx
                .prepare(
                    r#"
                    INSERT INTO reco_scores
                        (score_type, item_type, track_id, album_id, artist_id, score, updated_at)
                    VALUES (?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .map_err(|e| format!("Failed to prepare reco scores insert: {}", e))?;
            for entry in entries {
                stmt.execute(params![
                    score_type,
                    item_type,
                    entry.track_id.map(|v| v as i64),
                    entry.album_id.as_deref(),
                    entry.artist_id.map(|v| v as i64),
                    entry.score,
                    updated_at,
                ])
                .map_err(|e| format!("Failed to insert reco score: {}", e))?;
            }
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit reco scores: {}", e))?;
        Ok(())
    }

    // ---- Home seeds ----

    /// Gather the home/Discover ID seeds (mirrors `get_home_seeds_internal`).
    /// When trained scores exist (`reco_scores` has a `score_type='all'` row),
    /// fresh recent items are merged ahead of scored items; otherwise it falls
    /// back to the raw event-based queries.
    pub fn get_home_seeds(&self, limits: HomeSeedLimits) -> Result<HomeSeeds, String> {
        let has_scores = self.has_scores("all")?;

        let recently_played_album_ids = if has_scores {
            let recent_fresh = self.get_recent_album_ids(4)?;
            let scored = self.get_scored_album_ids("all", limits.recent_albums + 4)?;
            let merged =
                merge_unique_preserve_order(recent_fresh, scored, limits.recent_albums as usize);
            if merged.is_empty() {
                self.get_recent_album_ids(limits.recent_albums)?
            } else {
                merged
            }
        } else {
            self.get_recent_album_ids(limits.recent_albums)?
        };

        let continue_listening_track_ids = if has_scores {
            let recent_fresh = self.get_recent_track_ids(4)?;
            let scored = self.get_scored_track_ids("all", limits.continue_tracks + 4)?;
            let merged =
                merge_unique_preserve_order(recent_fresh, scored, limits.continue_tracks as usize);
            if merged.is_empty() {
                self.get_recent_track_ids(limits.continue_tracks)?
            } else {
                merged
            }
        } else {
            self.get_recent_track_ids(limits.continue_tracks)?
        };

        let top_artist_ids = if has_scores {
            let scored: Vec<TopArtistSeed> = self
                .get_scored_artist_scores("all", limits.top_artists)?
                .into_iter()
                .map(|(artist_id, score)| TopArtistSeed {
                    artist_id,
                    play_count: score.round().max(1.0) as u32,
                })
                .collect();
            if scored.is_empty() {
                self.get_top_artist_ids(limits.top_artists)?
            } else {
                scored
            }
        } else {
            self.get_top_artist_ids(limits.top_artists)?
        };

        let favorite_album_ids = if has_scores {
            let scored = self.get_scored_album_ids("favorite", limits.favorites)?;
            if scored.is_empty() {
                self.get_favorite_album_ids(limits.favorites)?
            } else {
                scored
            }
        } else {
            self.get_favorite_album_ids(limits.favorites)?
        };

        let favorite_track_ids = if has_scores {
            let scored = self.get_scored_track_ids("favorite", limits.favorites)?;
            if scored.is_empty() {
                self.get_favorite_track_ids(limits.favorites)?
            } else {
                scored
            }
        } else {
            self.get_favorite_track_ids(limits.favorites)?
        };

        Ok(HomeSeeds {
            recently_played_album_ids,
            continue_listening_track_ids,
            top_artist_ids,
            favorite_album_ids,
            favorite_track_ids,
        })
    }

    // ---- Scorer ----

    /// Recompute and persist recommendation scores from recent events.
    ///
    /// Faithful port of Tauri's `v2_reco_train_scores`
    /// (`src-tauri/src/commands_v2/library.rs:1771-1911`): same lookback window,
    /// same exponential half-life decay, the same event weights
    /// (play=1.0 / favorite=3.0 / playlist_add=1.2) and item weights
    /// (primary=1.0; non-primary album=0.7 / artist=0.5 / track=0.85 / other=0.6),
    /// the same top-N-per-type cap, and the same `(all, favorite) x (track,
    /// album, artist)` six `replace_scores` writes.
    pub fn train(&mut self, params: TrainParams) -> Result<(), String> {
        use std::collections::HashMap;

        let now = now_ts();
        let since_ts = now.saturating_sub(params.lookback_days * 86_400);
        let events = self.get_events_since(since_ts, params.max_events)?;

        let half_life_days = params.half_life_days;
        let decay_factor = |age_secs: i64| -> f64 {
            if half_life_days <= 0.0 {
                return 1.0;
            }
            let half_life_secs = half_life_days * 86_400.0;
            let exponent = age_secs as f64 / half_life_secs;
            0.5_f64.powf(exponent)
        };

        let event_weight = |event_type: &str| -> f64 {
            match event_type {
                "play" => 1.0,
                "favorite" => 3.0,
                "playlist_add" => 1.2,
                _ => 1.0,
            }
        };

        let item_weight = |item_type: &str, primary: bool| -> f64 {
            if primary {
                return 1.0;
            }
            match item_type {
                "album" => 0.7,
                "artist" => 0.5,
                "track" => 0.85,
                _ => 0.6,
            }
        };

        let build_scores = |favorites_only: bool| {
            let mut tracks: HashMap<u64, f64> = HashMap::new();
            let mut albums: HashMap<String, f64> = HashMap::new();
            let mut artists: HashMap<u64, f64> = HashMap::new();

            for event in &events {
                if favorites_only && event.event_type != "favorite" {
                    continue;
                }
                let age_secs = (now - event.created_at).max(0);
                let base_weight = event_weight(&event.event_type) * decay_factor(age_secs);

                if let Some(track_id) = event.track_id {
                    let weight = base_weight * item_weight("track", event.item_type == "track");
                    *tracks.entry(track_id).or_insert(0.0) += weight;
                }
                if let Some(album_id) = event.album_id.as_ref() {
                    let weight = base_weight * item_weight("album", event.item_type == "album");
                    *albums.entry(album_id.clone()).or_insert(0.0) += weight;
                }
                if let Some(artist_id) = event.artist_id {
                    let weight = base_weight * item_weight("artist", event.item_type == "artist");
                    *artists.entry(artist_id).or_insert(0.0) += weight;
                }
            }
            (tracks, albums, artists)
        };

        let max_per_type = params.max_per_type as usize;
        let build_track_entries = |scores: HashMap<u64, f64>| {
            let mut entries: Vec<(u64, f64)> = scores.into_iter().collect();
            entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            entries
                .into_iter()
                .take(max_per_type)
                .map(|(track_id, score)| RecoScoreEntry {
                    track_id: Some(track_id),
                    album_id: None,
                    artist_id: None,
                    score,
                })
                .collect::<Vec<_>>()
        };
        let build_album_entries = |scores: HashMap<String, f64>| {
            let mut entries: Vec<(String, f64)> = scores.into_iter().collect();
            entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            entries
                .into_iter()
                .take(max_per_type)
                .map(|(album_id, score)| RecoScoreEntry {
                    track_id: None,
                    album_id: Some(album_id),
                    artist_id: None,
                    score,
                })
                .collect::<Vec<_>>()
        };
        let build_artist_entries = |scores: HashMap<u64, f64>| {
            let mut entries: Vec<(u64, f64)> = scores.into_iter().collect();
            entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            entries
                .into_iter()
                .take(max_per_type)
                .map(|(artist_id, score)| RecoScoreEntry {
                    track_id: None,
                    album_id: None,
                    artist_id: Some(artist_id),
                    score,
                })
                .collect::<Vec<_>>()
        };

        let (all_tracks, all_albums, all_artists) = build_scores(false);
        let (fav_tracks, fav_albums, fav_artists) = build_scores(true);

        self.replace_scores("all", "track", &build_track_entries(all_tracks))?;
        self.replace_scores("all", "album", &build_album_entries(all_albums))?;
        self.replace_scores("all", "artist", &build_artist_entries(all_artists))?;
        self.replace_scores("favorite", "track", &build_track_entries(fav_tracks))?;
        self.replace_scores("favorite", "album", &build_album_entries(fav_albums))?;
        self.replace_scores("favorite", "artist", &build_artist_entries(fav_artists))?;

        Ok(())
    }

    /// Upsert an album-meta row (only needed so `get_top_genres` can resolve a
    /// genre name; mirrors the relevant columns of Tauri's `set_album_meta`).
    pub fn set_album_genre_name(
        &self,
        album_id: &str,
        genre_name: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO reco_album_meta
                       (album_id, title, artist_name, genre_name, updated_at)
                   VALUES (?, '', '', ?, ?)
                   ON CONFLICT(album_id) DO UPDATE SET genre_name = excluded.genre_name"#,
                params![album_id, genre_name, now_ts()],
            )
            .map_err(|e| format!("Failed to upsert album genre meta: {}", e))?;
        Ok(())
    }

    /// Backfill `genre_id` onto every still-NULL event of an album once its
    /// genre is known (ported from Tauri `db.rs:321-331`). Plays log
    /// `genre_id = None`, so the frontend calls this when it resolves an
    /// album's genre — this is what makes `get_top_genres` non-empty.
    pub fn update_genre_for_album(&self, album_id: &str, genre_id: u64) -> Result<u64, String> {
        let affected = self
            .conn
            .execute(
                "UPDATE reco_events SET genre_id = ? WHERE album_id = ? AND genre_id IS NULL",
                params![genre_id as i64, album_id],
            )
            .map_err(|e| format!("Failed to update genre for album: {}", e))?;
        Ok(affected as u64)
    }
}

/// Merge two lists preserving order: fresh items first, then scored items
/// (excluding duplicates) — verbatim from `helpers::merge_unique_preserve_order`.
fn merge_unique_preserve_order<T: Eq + std::hash::Hash + Clone>(
    fresh: Vec<T>,
    scored: Vec<T>,
    limit: usize,
) -> Vec<T> {
    use std::collections::HashSet;
    let mut seen: HashSet<T> = HashSet::new();
    let mut result = Vec::with_capacity(limit);
    for item in fresh {
        if seen.insert(item.clone()) {
            result.push(item);
            if result.len() >= limit {
                return result;
            }
        }
    }
    for item in scored {
        if seen.insert(item.clone()) {
            result.push(item);
            if result.len() >= limit {
                return result;
            }
        }
    }
    result
}

pub type RecoStoreState = Arc<Mutex<Option<RecoStore>>>;

pub fn create_empty_reco_store_state() -> RecoStoreState {
    Arc::new(Mutex::new(None))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("qbz-app-{name}-{}-{nonce}", std::process::id()))
    }

    /// Insert an event at an explicit timestamp (test-only; the public log
    /// helpers always stamp `now`).
    fn insert_at(
        store: &RecoStore,
        event_type: &str,
        item_type: &str,
        track_id: Option<u64>,
        album_id: Option<&str>,
        artist_id: Option<u64>,
        genre_id: Option<u64>,
        created_at: i64,
    ) {
        store
            .conn
            .execute(
                r#"INSERT INTO reco_events
                   (event_type, item_type, track_id, album_id, artist_id, playlist_id, genre_id, created_at)
                   VALUES (?, ?, ?, ?, ?, NULL, ?, ?)"#,
                params![event_type, item_type, track_id.map(|v| v as i64), album_id, artist_id.map(|v| v as i64), genre_id.map(|v| v as i64), created_at],
            )
            .expect("insert event at ts");
    }

    #[test]
    fn schema_creation_is_idempotent() {
        let dir = unique_test_dir("reco-idempotent");
        {
            let store = RecoStore::new_at(&dir).expect("open");
            store.log_play_event(1, Some("a1".into()), Some(10), Some(5)).unwrap();
        }
        // Reopen the SAME db file — init() must not error on existing tables.
        {
            let store = RecoStore::new_at(&dir).expect("reopen");
            // Data survives and is readable.
            assert_eq!(store.get_recent_track_ids(10).unwrap(), vec![1]);
        }
        // The file lives at <base>/reco/events.db (shared with Tauri).
        assert!(dir.join("reco").join("events.db").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn log_and_read_recent_and_favorite() {
        let dir = unique_test_dir("reco-logread");
        let store = RecoStore::new_at(&dir).expect("open");

        store.log_play_event(100, Some("alb".into()), Some(7), Some(3)).unwrap();
        store.log_play_event(200, Some("alb".into()), Some(7), Some(3)).unwrap();
        store.log_favorite_event(300, Some("alb2".into()), Some(9), Some(4)).unwrap();

        let recent = store.get_recent_track_ids(10).unwrap();
        assert!(recent.contains(&100) && recent.contains(&200));
        assert!(!recent.contains(&300)); // favorite is not a play

        let favs = store.get_favorite_track_ids(10).unwrap();
        assert_eq!(favs, vec![300]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn windowed_recent_query_respects_window() {
        let dir = unique_test_dir("reco-window");
        let store = RecoStore::new_at(&dir).expect("open");
        let now = now_ts();
        let day = 86_400;
        // track 1 played 2 days ago (inside 7d window), track 2 played 10 days ago (outside).
        insert_at(&store, "play", "track", Some(1), Some("a"), Some(11), Some(2), now - 2 * day);
        insert_at(&store, "play", "track", Some(2), Some("b"), Some(12), Some(2), now - 10 * day);

        let week = store.get_recent_track_ids_since(7 * day, 50).unwrap();
        assert_eq!(week, vec![1]);
        // The non-windowed query still sees both.
        let all = store.get_recent_track_ids(50).unwrap();
        assert!(all.contains(&1) && all.contains(&2));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn top_genres_ranking_and_names() {
        let dir = unique_test_dir("reco-genres");
        let store = RecoStore::new_at(&dir).expect("open");
        let now = now_ts();
        // genre 5 played 3x, genre 6 played 1x; genre 0 ignored (> 0 filter).
        insert_at(&store, "play", "track", Some(1), Some("alb5"), Some(1), Some(5), now);
        insert_at(&store, "play", "track", Some(2), Some("alb5"), Some(1), Some(5), now);
        insert_at(&store, "play", "track", Some(3), Some("alb5"), Some(1), Some(5), now);
        insert_at(&store, "play", "track", Some(4), Some("alb6"), Some(2), Some(6), now);
        insert_at(&store, "play", "track", Some(5), Some("albz"), Some(3), Some(0), now);
        // Provide a genre name for the album associated with genre 5.
        store.set_album_genre_name("alb5", "Jazz").unwrap();

        let genres = store.get_top_genres(10).unwrap();
        assert_eq!(genres.len(), 2); // genre 0 excluded
        assert_eq!(genres[0].0, 5); // most played first
        assert_eq!(genres[0].1, "Jazz"); // name resolved from reco_album_meta
        assert_eq!(genres[1].0, 6);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn home_seeds_shape_fallback_and_trained() {
        let dir = unique_test_dir("reco-homeseeds");
        let mut store = RecoStore::new_at(&dir).expect("open");
        let now = now_ts();
        insert_at(&store, "play", "track", Some(1), Some("alb1"), Some(10), Some(2), now - 100);
        insert_at(&store, "play", "track", Some(2), Some("alb2"), Some(11), Some(2), now - 50);
        insert_at(&store, "favorite", "track", Some(9), Some("alb9"), Some(20), Some(3), now - 10);

        // No scores yet -> fallback path.
        let seeds = store.get_home_seeds(HomeSeedLimits::default()).unwrap();
        assert!(seeds.continue_listening_track_ids.contains(&1));
        assert!(seeds.continue_listening_track_ids.contains(&2));
        assert!(seeds.favorite_track_ids.contains(&9));
        assert!(seeds.recently_played_album_ids.contains(&"alb1".to_string()));
        assert!(seeds.top_artist_ids.iter().any(|s| s.artist_id == 10));

        // Train -> scores now exist; seeds still return a coherent shape.
        store.train(TrainParams::default()).unwrap();
        let seeds2 = store.get_home_seeds(HomeSeedLimits::default()).unwrap();
        assert!(!seeds2.continue_listening_track_ids.is_empty());
        assert!(!seeds2.favorite_track_ids.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn train_favorite_outranks_play() {
        let dir = unique_test_dir("reco-train");
        let mut store = RecoStore::new_at(&dir).expect("open");
        let now = now_ts();
        // track 1: a single play (weight 1.0). track 2: a favorite (weight 3.0).
        insert_at(&store, "play", "track", Some(1), Some("a"), Some(1), Some(2), now);
        insert_at(&store, "favorite", "track", Some(2), Some("b"), Some(2), Some(2), now);

        store.train(TrainParams::default()).unwrap();

        // The "all" track scoring should rank the favorited track first.
        let scored = store.get_scored_track_ids("all", 10).unwrap();
        assert_eq!(scored.first(), Some(&2));
        // The "favorite" bucket only contains the favorited track.
        let fav_scored = store.get_scored_track_ids("favorite", 10).unwrap();
        assert_eq!(fav_scored, vec![2]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn forgotten_favorites_excludes_recently_played() {
        let dir = unique_test_dir("reco-forgotten");
        let store = RecoStore::new_at(&dir).unwrap();
        // Favorite albums A and B; only A was played (now).
        store.log_favorite_event(1, Some("A".into()), Some(10), None).unwrap();
        store.log_favorite_event(2, Some("B".into()), Some(11), None).unwrap();
        store.log_play_event(1, Some("A".into()), Some(10), None).unwrap();
        let forgotten = store.get_forgotten_favorite_album_ids(10, 30).unwrap();
        assert!(forgotten.contains(&"B".to_string())); // never played -> forgotten
        assert!(!forgotten.contains(&"A".to_string())); // played now -> not forgotten
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn genre_backfill_makes_top_genres_non_empty() {
        let dir = unique_test_dir("reco-genre-backfill");
        let store = RecoStore::new_at(&dir).unwrap();
        // Plays log no genre_id, so top genres is empty until backfill.
        store.log_play_event(1, Some("jz".into()), Some(10), None).unwrap();
        store.log_play_event(2, Some("jz".into()), Some(10), None).unwrap();
        store.log_play_event(3, Some("rk".into()), Some(11), None).unwrap();
        assert!(store.get_top_genres(10).unwrap().is_empty());
        // The frontend backfills genre on album resolution (id + name).
        store.update_genre_for_album("jz", 5).unwrap();
        store.set_album_genre_name("jz", "Jazz").unwrap();
        store.update_genre_for_album("rk", 6).unwrap();
        store.set_album_genre_name("rk", "Rock").unwrap();
        let genres = store.get_top_genres(10).unwrap();
        assert_eq!(genres.len(), 2);
        assert_eq!(genres[0].0, 5); // jazz played 2x -> ranked first
        assert_eq!(genres[0].1, "Jazz"); // name resolved from album-meta
        assert_eq!(genres[1].0, 6);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn known_artists_includes_played_and_favorited() {
        let dir = unique_test_dir("reco-known-artists");
        let store = RecoStore::new_at(&dir).unwrap();
        // artist 10: 3 plays (> threshold 2). artist 20: a single favorite, no
        // plays. artist 30: 1 play, not favorited -> excluded.
        store.log_play_event(1, Some("a".into()), Some(10), None).unwrap();
        store.log_play_event(2, Some("a".into()), Some(10), None).unwrap();
        store.log_play_event(3, Some("a".into()), Some(10), None).unwrap();
        store
            .insert_event(&RecoEventInput {
                event_type: RecoEventType::Favorite,
                item_type: RecoItemType::Artist,
                track_id: None,
                album_id: None,
                artist_id: Some(20),
                playlist_id: None,
                genre_id: None,
            })
            .unwrap();
        store.log_play_event(4, Some("c".into()), Some(30), None).unwrap();
        let known = store.get_known_artist_ids(2).unwrap();
        assert!(known.contains(&10)); // 3 plays > 2
        assert!(known.contains(&20)); // favorited
        assert!(!known.contains(&30)); // 1 play, not favorited
        let _ = std::fs::remove_dir_all(dir);
    }
}
