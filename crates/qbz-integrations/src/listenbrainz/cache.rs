//! ListenBrainz cache for offline listen queue
//!
//! SQLite-based persistence for:
//! - User credentials (token, username)
//! - Queued listens for offline submission
//! - Enabled state

use rusqlite::{Connection, Result as SqlResult};
use std::path::Path;

use super::models::QueuedListen;

/// ListenBrainz cache for offline support
pub struct ListenBrainzCache {
    conn: Connection,
}

impl ListenBrainzCache {
    /// Create a new cache at the given path
    pub fn new(db_path: &Path) -> Result<Self, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open ListenBrainz cache: {}", e))?;

        // Enable WAL mode for concurrent read/write (ADR-002)
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to enable WAL mode: {}", e))?;

        let cache = Self { conn };
        cache.init_schema()?;

        Ok(cache)
    }

    fn init_schema(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS credentials (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    token TEXT,
                    user_name TEXT
                );

                CREATE TABLE IF NOT EXISTS settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS listen_queue (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    listened_at INTEGER NOT NULL,
                    artist_name TEXT NOT NULL,
                    track_name TEXT NOT NULL,
                    release_name TEXT,
                    recording_mbid TEXT,
                    release_mbid TEXT,
                    artist_mbids TEXT,
                    isrc TEXT,
                    duration_ms INTEGER,
                    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                    attempts INTEGER DEFAULT 0,
                    sent INTEGER DEFAULT 0
                );

                CREATE INDEX IF NOT EXISTS idx_listen_queue_sent ON listen_queue(sent);
            ",
            )
            .map_err(|e| format!("Failed to init ListenBrainz schema: {}", e))
    }

    /// Save credentials
    pub fn save_credentials(&self, token: &str, user_name: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO credentials (id, token, user_name) VALUES (1, ?, ?)",
                [token, user_name],
            )
            .map_err(|e| format!("Failed to save credentials: {}", e))?;
        Ok(())
    }

    /// Get saved credentials
    pub fn get_credentials(&self) -> Result<(Option<String>, Option<String>), String> {
        let result: SqlResult<(Option<String>, Option<String>)> = self.conn.query_row(
            "SELECT token, user_name FROM credentials WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        match result {
            Ok((token, user_name)) => Ok((token, user_name)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok((None, None)),
            Err(e) => Err(format!("Failed to get credentials: {}", e)),
        }
    }

    /// Clear credentials
    pub fn clear_credentials(&self) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM credentials", [])
            .map_err(|e| format!("Failed to clear credentials: {}", e))?;
        Ok(())
    }

    /// Check if enabled
    pub fn is_enabled(&self) -> Result<bool, String> {
        let result: SqlResult<String> = self.conn.query_row(
            "SELECT value FROM settings WHERE key = 'enabled'",
            [],
            |row| row.get(0),
        );

        match result {
            Ok(val) => Ok(val != "0"),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(true), // Default enabled
            Err(e) => Err(format!("Failed to get enabled state: {}", e)),
        }
    }

    /// Set enabled state
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let value = if enabled { "1" } else { "0" };
        self.conn
            .execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('enabled', ?)",
                [value],
            )
            .map_err(|e| format!("Failed to set enabled: {}", e))?;
        Ok(())
    }

    /// Queue a listen for later submission
    pub fn queue_listen(
        &self,
        listened_at: i64,
        artist: &str,
        track: &str,
        album: Option<&str>,
        recording_mbid: Option<&str>,
        release_mbid: Option<&str>,
        artist_mbids: Option<&[String]>,
        isrc: Option<&str>,
        duration_ms: Option<u64>,
    ) -> Result<i64, String> {
        let artist_mbids_json =
            artist_mbids.map(|ids| serde_json::to_string(ids).unwrap_or_default());

        self.conn
            .execute(
                "INSERT INTO listen_queue (listened_at, artist_name, track_name, release_name, recording_mbid, release_mbid, artist_mbids, isrc, duration_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    listened_at,
                    artist,
                    track,
                    album,
                    recording_mbid,
                    release_mbid,
                    artist_mbids_json,
                    isrc,
                    duration_ms.map(|d| d as i64),
                ],
            )
            .map_err(|e| format!("Failed to queue listen: {}", e))?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get pending listens (not yet sent)
    pub fn get_pending_listens(&self, limit: u32) -> Result<Vec<QueuedListen>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, listened_at, artist_name, track_name, release_name,
                        recording_mbid, release_mbid, artist_mbids, isrc, duration_ms,
                        created_at, attempts, sent
                 FROM listen_queue
                 WHERE sent = 0
                 ORDER BY listened_at ASC
                 LIMIT ?",
            )
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let listens = stmt
            .query_map([limit], |row| {
                let artist_mbids_json: Option<String> = row.get(7)?;
                let artist_mbids =
                    artist_mbids_json.and_then(|json| serde_json::from_str(&json).ok());

                Ok(QueuedListen {
                    id: row.get(0)?,
                    listened_at: row.get(1)?,
                    artist_name: row.get(2)?,
                    track_name: row.get(3)?,
                    release_name: row.get(4)?,
                    recording_mbid: row.get(5)?,
                    release_mbid: row.get(6)?,
                    artist_mbids,
                    isrc: row.get(8)?,
                    duration_ms: row.get::<_, Option<i64>>(9)?.map(|d| d as u64),
                    created_at: row.get(10)?,
                    attempts: row.get(11)?,
                    sent: row.get::<_, i32>(12)? != 0,
                })
            })
            .map_err(|e| format!("Failed to query listens: {}", e))?
            .collect::<SqlResult<Vec<_>>>()
            .map_err(|e| format!("Failed to collect listens: {}", e))?;

        Ok(listens)
    }

    /// Mark a listen as sent
    pub fn mark_sent(&self, id: i64) -> Result<(), String> {
        self.conn
            .execute("UPDATE listen_queue SET sent = 1 WHERE id = ?", [id])
            .map_err(|e| format!("Failed to mark sent: {}", e))?;
        Ok(())
    }

    /// Increment attempt count
    pub fn increment_attempts(&self, id: i64) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE listen_queue SET attempts = attempts + 1 WHERE id = ?",
                [id],
            )
            .map_err(|e| format!("Failed to increment attempts: {}", e))?;
        Ok(())
    }

    /// Batch mark multiple listens as sent
    pub fn mark_listens_sent(&self, ids: &[i64]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "UPDATE listen_queue SET sent = 1 WHERE id IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        self.conn
            .execute(&sql, params_refs.as_slice())
            .map_err(|e| format!("Failed to batch mark sent: {}", e))?;
        Ok(())
    }

    /// Get count of unsent listens in queue
    pub fn get_queue_count(&self) -> Result<u32, String> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM listen_queue WHERE sent = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count as u32)
    }

    /// Clear all queued listens
    pub fn clear_queue(&self) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM listen_queue", [])
            .map_err(|e| format!("Failed to clear queue: {}", e))?;
        Ok(())
    }

    /// Delete old sent listens
    pub fn cleanup_sent(&self, older_than_days: u32) -> Result<u64, String> {
        let cutoff = chrono::Utc::now().timestamp() - (older_than_days as i64 * 86400);
        let deleted = self
            .conn
            .execute(
                "DELETE FROM listen_queue WHERE sent = 1 AND created_at < ?",
                [cutoff],
            )
            .map_err(|e| format!("Failed to cleanup: {}", e))?;
        Ok(deleted as u64)
    }
}
