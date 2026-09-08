use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Radio sessions idle for longer than this are garbage-collected on open.
const STALE_SESSION_SECS: i64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioSeed {
    Artist { artist_id: u64 },
    Track { track_id: u64, artist_id: u64 },
}

impl RadioSeed {
    pub fn seed_type(&self) -> &'static str {
        match self {
            RadioSeed::Artist { .. } => "artist",
            RadioSeed::Track { .. } => "track",
        }
    }

    pub fn seed_id(&self) -> String {
        match self {
            RadioSeed::Artist { artist_id } => artist_id.to_string(),
            RadioSeed::Track { track_id, .. } => track_id.to_string(),
        }
    }

    pub fn seed_artist_id(&self) -> u64 {
        match self {
            RadioSeed::Artist { artist_id } => *artist_id,
            RadioSeed::Track { artist_id, .. } => *artist_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RadioSession {
    pub id: String,
    pub seed: RadioSeed,
    pub rng_seed: u64,
    pub selection_count: u32,
    pub artist_spacing: u32,
    pub reseed_every: u32,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct RadioTrackRef {
    pub track_id: u64,
    pub artist_id: u64,
    pub source: String,
    pub distance: u8,
}

pub struct RadioDb {
    conn: Connection,
    #[allow(dead_code)]
    path: Option<PathBuf>,
}

impl RadioDb {
    pub fn open_default() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz")
            .join("radio");

        std::fs::create_dir_all(&data_dir)
            .map_err(|e| format!("Failed to create radio data directory: {}", e))?;

        let db_path = data_dir.join("radio_engine.db");
        Self::open(&db_path)
    }

    pub fn open_at(base_dir: &Path) -> Result<Self, String> {
        let radio_dir = base_dir.join("radio");
        std::fs::create_dir_all(&radio_dir)
            .map_err(|e| format!("Failed to create radio data directory: {}", e))?;
        let db_path = radio_dir.join("radio_engine.db");
        Self::open(&db_path)
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        let conn =
            Connection::open(path).map_err(|e| format!("Failed to open radio database: {}", e))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to set WAL mode: {}", e))?;
        let db = Self {
            conn,
            path: Some(path.to_path_buf()),
        };
        db.init()?;
        // Best-effort GC on every open of the on-disk DB: `radio_history`
        // had no delete path at all, so the file only ever grew.
        match db.gc_stale_sessions(STALE_SESSION_SECS) {
            Ok(0) => {}
            Ok(n) => log::info!("[radio] GC dropped {n} radio session(s) idle > 30 d"),
            Err(e) => log::debug!("[radio] GC skipped: {e}"),
        }
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory()
            .map_err(|e| format!("Failed to open in-memory radio database: {}", e))?;
        let db = Self { conn, path: None };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS radio_session (
                    id TEXT PRIMARY KEY,
                    seed_type TEXT NOT NULL,
                    seed_id TEXT NOT NULL,
                    seed_artist_id INTEGER NOT NULL,
                    rng_seed INTEGER NOT NULL,
                    selection_count INTEGER NOT NULL DEFAULT 0,
                    artist_spacing INTEGER NOT NULL DEFAULT 5,
                    reseed_every INTEGER NOT NULL DEFAULT 25,
                    created_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS radio_pool (
                    session_id TEXT NOT NULL,
                    track_id INTEGER NOT NULL,
                    artist_id INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    distance INTEGER NOT NULL,
                    used INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (session_id, track_id)
                );
                CREATE INDEX IF NOT EXISTS idx_radio_pool_session_used_distance
                    ON radio_pool(session_id, used, distance);
                CREATE INDEX IF NOT EXISTS idx_radio_pool_session_artist
                    ON radio_pool(session_id, artist_id);

                CREATE TABLE IF NOT EXISTS radio_history (
                    session_id TEXT NOT NULL,
                    seq INTEGER NOT NULL,
                    track_id INTEGER NOT NULL,
                    artist_id INTEGER NOT NULL,
                    played_at INTEGER NOT NULL,
                    PRIMARY KEY (session_id, seq)
                );
                CREATE INDEX IF NOT EXISTS idx_radio_history_session_track
                    ON radio_history(session_id, track_id);
                "#,
            )
            .map_err(|e| format!("Failed to initialize radio database: {}", e))?;
        Ok(())
    }

    /// Delete every session (and its pool + history rows) whose LAST activity
    /// — the newest `radio_history.played_at`, or `created_at` for a session
    /// that never played — is older than `max_idle_secs`. Keyed on activity,
    /// not creation, so a long-running radio is never pulled from under the
    /// engine. Returns the number of sessions removed.
    pub fn gc_stale_sessions(&self, max_idle_secs: i64) -> Result<usize, String> {
        let cutoff = Self::now_ts() - max_idle_secs;
        let stale: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT s.id FROM radio_session s
                     LEFT JOIN (SELECT session_id, MAX(played_at) AS last_at
                                FROM radio_history GROUP BY session_id) h
                       ON h.session_id = s.id
                     WHERE COALESCE(h.last_at, s.created_at) < ?",
                )
                .map_err(|e| format!("Failed to prepare radio GC: {}", e))?;
            let rows = stmt
                .query_map(params![cutoff], |row| row.get::<_, String>(0))
                .map_err(|e| format!("Failed to query stale radio sessions: {}", e))?;
            rows.flatten().collect()
        };
        for id in &stale {
            for sql in [
                "DELETE FROM radio_history WHERE session_id = ?",
                "DELETE FROM radio_pool WHERE session_id = ?",
                "DELETE FROM radio_session WHERE id = ?",
            ] {
                self.conn
                    .execute(sql, params![id])
                    .map_err(|e| format!("Failed to GC radio session: {}", e))?;
            }
        }
        Ok(stale.len())
    }

    fn now_ts() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    pub fn create_session(
        &self,
        seed: RadioSeed,
        rng_seed: u64,
        artist_spacing: u32,
        reseed_every: u32,
    ) -> Result<RadioSession, String> {
        let created_at = Self::now_ts();
        let id = format!("radio_{}_{}", created_at, rng_seed);

        self.conn
            .execute(
                r#"
                INSERT INTO radio_session (
                    id, seed_type, seed_id, seed_artist_id, rng_seed, selection_count, artist_spacing, reseed_every, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8)
                "#,
                params![
                    id,
                    seed.seed_type(),
                    seed.seed_id(),
                    seed.seed_artist_id() as i64,
                    rng_seed as i64,
                    artist_spacing as i64,
                    reseed_every as i64,
                    created_at,
                ],
            )
            .map_err(|e| format!("Failed to insert radio session: {}", e))?;

        Ok(RadioSession {
            id,
            seed,
            rng_seed,
            selection_count: 0,
            artist_spacing,
            reseed_every,
            created_at,
        })
    }

    pub fn load_session(&self, session_id: &str) -> Result<RadioSession, String> {
        let row = self
            .conn
            .query_row(
                r#"
                SELECT seed_type, seed_id, seed_artist_id, rng_seed, selection_count, artist_spacing, reseed_every, created_at
                FROM radio_session
                WHERE id = ?
                "#,
                params![session_id],
                |row| {
                    let seed_type: String = row.get(0)?;
                    let seed_id: String = row.get(1)?;
                    let seed_artist_id: i64 = row.get(2)?;
                    let rng_seed: i64 = row.get(3)?;
                    let selection_count: i64 = row.get(4)?;
                    let artist_spacing: i64 = row.get(5)?;
                    let reseed_every: i64 = row.get(6)?;
                    let created_at: i64 = row.get(7)?;

                    let seed = match seed_type.as_str() {
                        "artist" => RadioSeed::Artist {
                            artist_id: seed_id.parse::<u64>().unwrap_or(seed_artist_id as u64),
                        },
                        "track" => RadioSeed::Track {
                            track_id: seed_id.parse::<u64>().unwrap_or(0),
                            artist_id: seed_artist_id as u64,
                        },
                        _ => RadioSeed::Artist {
                            artist_id: seed_artist_id as u64,
                        },
                    };

                    Ok(RadioSession {
                        id: session_id.to_string(),
                        seed,
                        rng_seed: rng_seed as u64,
                        selection_count: selection_count as u32,
                        artist_spacing: artist_spacing as u32,
                        reseed_every: reseed_every as u32,
                        created_at,
                    })
                },
            )
            .map_err(|e| format!("Failed to load radio session: {}", e))?;
        Ok(row)
    }

    pub fn pool_size(&self, session_id: &str) -> Result<u32, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM radio_pool WHERE session_id = ?",
                params![session_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v as u32)
            .map_err(|e| format!("Failed to query radio pool size: {}", e))
    }

    pub fn pool_used_count(&self, session_id: &str) -> Result<u32, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM radio_pool WHERE session_id = ? AND used = 1",
                params![session_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v as u32)
            .map_err(|e| format!("Failed to query radio pool used count: {}", e))
    }

    pub fn pool_unused_count(&self, session_id: &str) -> Result<u32, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM radio_pool WHERE session_id = ? AND used = 0",
                params![session_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v as u32)
            .map_err(|e| format!("Failed to query radio pool unused count: {}", e))
    }

    pub fn history_len(&self, session_id: &str) -> Result<u32, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM radio_history WHERE session_id = ?",
                params![session_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v as u32)
            .map_err(|e| format!("Failed to query radio history length: {}", e))
    }

    pub fn insert_pool_track(
        &self,
        session_id: &str,
        track_id: u64,
        artist_id: u64,
        source: &str,
        distance: u8,
    ) -> Result<(), String> {
        if distance > 2 {
            return Ok(());
        }

        self.conn
            .execute(
                r#"
                INSERT OR IGNORE INTO radio_pool (session_id, track_id, artist_id, source, distance, used)
                VALUES (?1, ?2, ?3, ?4, ?5, 0)
                "#,
                params![
                    session_id,
                    track_id as i64,
                    artist_id as i64,
                    source,
                    distance as i64
                ],
            )
            .map_err(|e| format!("Failed to insert radio pool track: {}", e))?;

        // If the track already exists, keep the smallest distance (and update source accordingly).
        self.conn
            .execute(
                r#"
                UPDATE radio_pool
                SET distance = ?3, source = ?4
                WHERE session_id = ?1 AND track_id = ?2 AND distance > ?3
                "#,
                params![session_id, track_id as i64, distance as i64, source],
            )
            .map_err(|e| format!("Failed to update radio pool track distance: {}", e))?;

        Ok(())
    }

    #[cfg(test)]
    pub fn insert_pool_track_unchecked(
        &self,
        session_id: &str,
        track_id: u64,
        artist_id: u64,
        source: &str,
        distance: u8,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"
                INSERT OR REPLACE INTO radio_pool (session_id, track_id, artist_id, source, distance, used)
                VALUES (?1, ?2, ?3, ?4, ?5, 0)
                "#,
                params![
                    session_id,
                    track_id as i64,
                    artist_id as i64,
                    source,
                    distance as i64
                ],
            )
            .map_err(|e| format!("Failed to insert unchecked radio pool track: {}", e))?;
        Ok(())
    }

    pub fn get_recent_artist_ids(&self, session_id: &str, n: u32) -> Result<Vec<u64>, String> {
        if n == 0 {
            return Ok(Vec::new());
        }

        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT artist_id
                FROM radio_history
                WHERE session_id = ?
                ORDER BY seq DESC
                LIMIT ?
                "#,
            )
            .map_err(|e| format!("Failed to prepare recent artists query: {}", e))?;

        let rows = stmt
            .query_map(params![session_id, n as i64], |row| row.get::<_, i64>(0))
            .map_err(|e| format!("Failed to query recent artists: {}", e))?;

        let mut artists = Vec::new();
        for row in rows {
            artists
                .push(row.map_err(|e| format!("Failed to read recent artist row: {}", e))? as u64);
        }
        Ok(artists)
    }

    pub fn get_unused_candidates(
        &self,
        session_id: &str,
        exclude_artist_ids: &[u64],
    ) -> Result<Vec<RadioTrackRef>, String> {
        let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
        params_vec.push(rusqlite::types::Value::Text(session_id.to_string()));

        let mut sql = String::from(
            r#"
            SELECT track_id, artist_id, source, distance
            FROM radio_pool
            WHERE session_id = ? AND used = 0 AND distance <= 2
            "#,
        );

        if !exclude_artist_ids.is_empty() {
            sql.push_str(" AND artist_id NOT IN (");
            for (idx, artist_id) in exclude_artist_ids.iter().enumerate() {
                if idx > 0 {
                    sql.push(',');
                }
                sql.push('?');
                params_vec.push(rusqlite::types::Value::Integer(*artist_id as i64));
            }
            sql.push(')');
        }

        sql.push_str(" ORDER BY distance ASC, track_id ASC");

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare candidates query: {}", e))?;

        let rows = stmt
            .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
                Ok(RadioTrackRef {
                    track_id: row.get::<_, i64>(0)? as u64,
                    artist_id: row.get::<_, i64>(1)? as u64,
                    source: row.get::<_, String>(2)?,
                    distance: row.get::<_, i64>(3)? as u8,
                })
            })
            .map_err(|e| format!("Failed to query candidates: {}", e))?;

        let mut candidates = Vec::new();
        for row in rows {
            candidates.push(row.map_err(|e| format!("Failed to read candidate row: {}", e))?);
        }
        Ok(candidates)
    }

    pub fn mark_played(
        &self,
        session_id: &str,
        session_selection_count: u32,
        track: &RadioTrackRef,
    ) -> Result<(), String> {
        let played_at = Self::now_ts();
        let seq = (session_selection_count as i64) + 1;

        self.conn
            .execute("BEGIN TRANSACTION", [])
            .map_err(|e| format!("Failed to begin radio transaction: {}", e))?;

        let mark_used = self.conn.execute(
            "UPDATE radio_pool SET used = 1 WHERE session_id = ? AND track_id = ?",
            params![session_id, track.track_id as i64],
        );
        if let Err(e) = mark_used {
            let _ = self.conn.execute("ROLLBACK", []);
            return Err(format!("Failed to mark radio track used: {}", e));
        }

        let insert_history = self.conn.execute(
            r#"
            INSERT INTO radio_history (session_id, seq, track_id, artist_id, played_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                session_id,
                seq,
                track.track_id as i64,
                track.artist_id as i64,
                played_at
            ],
        );
        if let Err(e) = insert_history {
            let _ = self.conn.execute("ROLLBACK", []);
            return Err(format!("Failed to insert radio history: {}", e));
        }

        let update_session = self.conn.execute(
            "UPDATE radio_session SET selection_count = ?2 WHERE id = ?1",
            params![session_id, seq],
        );
        if let Err(e) = update_session {
            let _ = self.conn.execute("ROLLBACK", []);
            return Err(format!("Failed to update radio session: {}", e));
        }

        self.conn
            .execute("COMMIT", [])
            .map_err(|e| format!("Failed to commit radio transaction: {}", e))?;

        Ok(())
    }

    pub fn has_track_been_played(&self, session_id: &str, track_id: u64) -> Result<bool, String> {
        let result: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM radio_history WHERE session_id = ? AND track_id = ? LIMIT 1",
                params![session_id, track_id as i64],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query radio history: {}", e))?;

        Ok(result.is_some())
    }
}
