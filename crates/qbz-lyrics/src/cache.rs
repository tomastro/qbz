//! SQLite cache for lyrics.
//!
//! Same schema/path Tauri uses (`src-tauri/src/lyrics/cache.rs`, DB at
//! `<per-user cache dir>/lyrics/lyrics.db`): both frontends share one cache —
//! a track fetched in Tauri is warm in Slint and vice versa. WAL +
//! `synchronous=NORMAL` per ADR-002. No TTL, no eviction (clear-only),
//! upsert = `INSERT OR REPLACE`.
//!
//! ADDITIVE delta (amended Q5): column `qobuz_wsync_json` stores the native
//! word-synced document for `provider='qobuz'` entries. Tauri-era DBs are
//! migrated in place (`ALTER TABLE … ADD COLUMN`); old rows stay intact and
//! today's Tauri build keeps reading/writing the shared columns it knows.

use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

use crate::model::{LyricsPayload, LyricsProvider};

/// One cached row: the wire-compatible payload plus the native wsync column.
#[derive(Debug, Clone)]
pub struct CachedLyrics {
    pub payload: LyricsPayload,
    pub qobuz_wsync_json: Option<String>,
}

/// Cache stats (entries + on-disk size). Size is measured from the ACTUAL
/// per-user path this DB was opened at — fix-forward of defect F1 (Tauri's
/// `v2_lyrics_get_cache_stats` measured the stale pre-migration global path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LyricsCacheStats {
    pub entries: u64,
    pub size_bytes: u64,
}

/// Database wrapper for lyrics cache
pub struct LyricsCacheDb {
    conn: Connection,
    path: PathBuf,
}

impl LyricsCacheDb {
    /// Open or create the database
    pub fn new(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path)
            .map_err(|e| format!("Failed to open lyrics cache database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to enable WAL for lyrics cache database: {}", e))?;

        let db = Self {
            conn,
            path: path.to_path_buf(),
        };
        db.init_schema()?;
        db.migrate_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<(), String> {
        // Identical to Tauri's CREATE statements (`cache.rs:31-47`) so a
        // shared DB created by either side converges; the additive column is
        // handled uniformly by migrate_schema() for fresh AND existing DBs.
        self.conn
            .execute_batch(
                "
            CREATE TABLE IF NOT EXISTS lyrics_cache (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                track_id INTEGER,
                cache_key TEXT UNIQUE NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT,
                duration_secs INTEGER,
                plain TEXT,
                synced_lrc TEXT,
                provider TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_lyrics_track_id ON lyrics_cache(track_id);
            CREATE INDEX IF NOT EXISTS idx_lyrics_cache_key ON lyrics_cache(cache_key);
            ",
            )
            .map_err(|e| format!("Failed to initialize lyrics cache schema: {}", e))?;

        Ok(())
    }

    /// Additive migration: bring a Tauri-era DB up to the current schema.
    fn migrate_schema(&self) -> Result<(), String> {
        let has_wsync_column = self
            .conn
            .prepare("PRAGMA table_info(lyrics_cache)")
            .and_then(|mut stmt| {
                let names = stmt
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(names.iter().any(|name| name == "qobuz_wsync_json"))
            })
            .map_err(|e| format!("Failed to inspect lyrics cache schema: {}", e))?;

        if !has_wsync_column {
            self.conn
                .execute("ALTER TABLE lyrics_cache ADD COLUMN qobuz_wsync_json TEXT", [])
                .map_err(|e| format!("Failed to add qobuz_wsync_json column: {}", e))?;
        }

        Ok(())
    }

    fn row_to_cached(row: &rusqlite::Row<'_>) -> rusqlite::Result<CachedLyrics> {
        Ok(CachedLyrics {
            payload: LyricsPayload {
                track_id: row.get::<_, Option<i64>>(0)?.map(|v| v as u64),
                title: row.get(1)?,
                artist: row.get(2)?,
                album: row.get(3)?,
                duration_secs: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                plain: row.get(5)?,
                synced_lrc: row.get(6)?,
                provider: LyricsProvider::from_str(&row.get::<_, String>(7)?),
                cached: true,
            },
            qobuz_wsync_json: row.get(8)?,
        })
    }

    pub fn get_by_track_id(&self, track_id: u64) -> Result<Option<CachedLyrics>, String> {
        let result = self.conn.query_row(
            "SELECT track_id, title, artist, album, duration_secs, plain, synced_lrc, provider, qobuz_wsync_json
             FROM lyrics_cache WHERE track_id = ?1",
            params![track_id as i64],
            Self::row_to_cached,
        );

        match result {
            Ok(cached) => Ok(Some(cached)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to read cached lyrics: {}", e)),
        }
    }

    pub fn get_by_cache_key(&self, cache_key: &str) -> Result<Option<CachedLyrics>, String> {
        let result = self.conn.query_row(
            "SELECT track_id, title, artist, album, duration_secs, plain, synced_lrc, provider, qobuz_wsync_json
             FROM lyrics_cache WHERE cache_key = ?1",
            params![cache_key],
            Self::row_to_cached,
        );

        match result {
            Ok(cached) => Ok(Some(cached)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to read cached lyrics: {}", e)),
        }
    }

    pub fn upsert(
        &self,
        cache_key: &str,
        payload: &LyricsPayload,
        qobuz_wsync_json: Option<&str>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO lyrics_cache
                 (track_id, cache_key, title, artist, album, duration_secs, plain, synced_lrc, provider, qobuz_wsync_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'), datetime('now'))",
                params![
                    payload.track_id.map(|v| v as i64),
                    cache_key,
                    payload.title,
                    payload.artist,
                    payload.album,
                    payload.duration_secs.map(|v| v as i64),
                    payload.plain,
                    payload.synced_lrc,
                    payload.provider.as_str(),
                    qobuz_wsync_json,
                ],
            )
            .map_err(|e| format!("Failed to write lyrics cache: {}", e))?;

        Ok(())
    }

    pub fn clear(&self) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM lyrics_cache", [])
            .map_err(|e| format!("Failed to clear lyrics cache: {}", e))?;
        Ok(())
    }

    pub fn count_entries(&self) -> Result<u64, String> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM lyrics_cache", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count lyrics cache entries: {}", e))?;
        Ok(count.max(0) as u64)
    }

    /// On-disk size of THIS database (db + WAL + shm sidecars) — always the
    /// per-user path the DB was opened at (F1 fix-forward).
    pub fn size_bytes(&self) -> u64 {
        let mut total = 0u64;
        for suffix in ["", "-wal", "-shm"] {
            let mut candidate = self.path.clone().into_os_string();
            candidate.push(suffix);
            if let Ok(meta) = std::fs::metadata(PathBuf::from(candidate)) {
                total = total.saturating_add(meta.len());
            }
        }
        total
    }

    pub fn stats(&self) -> Result<LyricsCacheStats, String> {
        Ok(LyricsCacheStats {
            entries: self.count_entries()?,
            size_bytes: self.size_bytes(),
        })
    }

    /// Path this DB is rooted at (diagnostics).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(track_id: Option<u64>, provider: LyricsProvider) -> LyricsPayload {
        LyricsPayload {
            track_id,
            title: "Title".into(),
            artist: "Artist".into(),
            album: Some("Album".into()),
            duration_secs: Some(215),
            plain: Some("plain text".into()),
            synced_lrc: Some("[00:01.00] hi".into()),
            provider,
            cached: false,
        }
    }

    fn temp_db_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("lyrics.db")
    }

    #[test]
    fn upsert_and_get_round_trip_by_id_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = LyricsCacheDb::new(&temp_db_path(&dir)).unwrap();

        let p = payload(Some(42), LyricsProvider::Lrclib);
        db.upsert("artist::title::215", &p, None).unwrap();

        let by_id = db.get_by_track_id(42).unwrap().expect("hit by track_id");
        assert_eq!(by_id.payload.title, "Title");
        assert!(by_id.payload.cached); // cache reads flag themselves
        assert!(by_id.qobuz_wsync_json.is_none());

        let by_key = db
            .get_by_cache_key("artist::title::215")
            .unwrap()
            .expect("hit by key");
        assert_eq!(by_key.payload.track_id, Some(42));
        assert_eq!(by_key.payload.provider, LyricsProvider::Lrclib);

        assert!(db.get_by_track_id(999).unwrap().is_none());
        assert!(db.get_by_cache_key("nope").unwrap().is_none());
    }

    #[test]
    fn wsync_column_persists_and_replace_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = LyricsCacheDb::new(&temp_db_path(&dir)).unwrap();

        let p = payload(Some(7), LyricsProvider::Qobuz);
        let wsync = r#"{"type":"wsync","lines":[{"line":"hi","start":1,"end":2}]}"#;
        db.upsert("k", &p, Some(wsync)).unwrap();

        let hit = db.get_by_track_id(7).unwrap().unwrap();
        assert_eq!(hit.qobuz_wsync_json.as_deref(), Some(wsync));
        assert_eq!(hit.payload.provider, LyricsProvider::Qobuz);

        // INSERT OR REPLACE without wsync (e.g. an lrclib upgrade) clears it.
        let p2 = payload(Some(7), LyricsProvider::Lrclib);
        db.upsert("k", &p2, None).unwrap();
        let hit = db.get_by_track_id(7).unwrap().unwrap();
        assert!(hit.qobuz_wsync_json.is_none());
        assert_eq!(db.count_entries().unwrap(), 1);
    }

    #[test]
    fn migrates_tauri_era_schema_additively() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_db_path(&dir);

        // Build the EXACT Tauri-era schema (src-tauri/src/lyrics/cache.rs:31-47,
        // no qobuz_wsync_json) and seed a row with raw SQL.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE lyrics_cache (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    track_id INTEGER,
                    cache_key TEXT UNIQUE NOT NULL,
                    title TEXT NOT NULL,
                    artist TEXT NOT NULL,
                    album TEXT,
                    duration_secs INTEGER,
                    plain TEXT,
                    synced_lrc TEXT,
                    provider TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE INDEX idx_lyrics_track_id ON lyrics_cache(track_id);
                CREATE INDEX idx_lyrics_cache_key ON lyrics_cache(cache_key);
                ",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO lyrics_cache (track_id, cache_key, title, artist, album, duration_secs, plain, synced_lrc, provider)
                 VALUES (1, 'old::row::100', 'Old', 'Row', NULL, 100, 'plain', '[00:01.00] x', 'lrclib')",
                [],
            )
            .unwrap();
        }

        // Reopen through the crate: additive column appears, old rows intact.
        let db = LyricsCacheDb::new(&path).unwrap();
        let names: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(lyrics_cache)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(names.iter().any(|n| n == "qobuz_wsync_json"));

        let old = db.get_by_track_id(1).unwrap().expect("old row readable");
        assert_eq!(old.payload.title, "Old");
        assert_eq!(old.payload.synced_lrc.as_deref(), Some("[00:01.00] x"));
        assert!(old.qobuz_wsync_json.is_none());
        assert_eq!(db.count_entries().unwrap(), 1);

        // New writes can use the column on the migrated DB.
        db.upsert("new::row::1", &payload(Some(2), LyricsProvider::Qobuz), Some("{}"))
            .unwrap();
        let new = db.get_by_track_id(2).unwrap().unwrap();
        assert_eq!(new.qobuz_wsync_json.as_deref(), Some("{}"));

        // Reopening again is a no-op (migration is idempotent).
        drop(db);
        let db = LyricsCacheDb::new(&path).unwrap();
        assert_eq!(db.count_entries().unwrap(), 2);
    }

    #[test]
    fn clear_and_stats() {
        let dir = tempfile::tempdir().unwrap();
        let db = LyricsCacheDb::new(&temp_db_path(&dir)).unwrap();
        db.upsert("a", &payload(Some(1), LyricsProvider::Ovh), None)
            .unwrap();
        db.upsert("b", &payload(Some(2), LyricsProvider::Lrclib), None)
            .unwrap();

        let stats = db.stats().unwrap();
        assert_eq!(stats.entries, 2);
        assert!(stats.size_bytes > 0); // measured at the real per-user path (F1)

        db.clear().unwrap();
        assert_eq!(db.count_entries().unwrap(), 0);
    }
}
