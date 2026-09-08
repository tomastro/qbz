//! The per-user listen-event store: `<user_dir>/listen/listen_log.db`.
//!
//! One row per listen EVENT (PK = event, never the track — the Tempo trap:
//! a track-keyed table with REPLACE can only ever count to one). Counters
//! are derived by readers, never written here.
//!
//! Synchronous `rusqlite`; the async facade (`logger.rs`) wraps every call
//! in `spawn_blocking`. WAL + `synchronous=NORMAL` (ADR-002).

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use super::rules::EndReason;
use super::tracker::{Closed, Flush, ListenMeta};

pub const SCHEMA_VERSION: i64 = 1;

/// Sub-directory + file under the user dir.
pub const DB_SUBDIR: &str = "listen";
pub const DB_FILE: &str = "listen_log.db";

pub struct ListenStore {
    conn: Connection,
    #[allow(dead_code)]
    path: Option<PathBuf>,
}

/// One persisted row, as readers see it (tests + the developer query).
#[derive(Debug, Clone, PartialEq)]
pub struct ListenRow {
    pub id: i64,
    pub app_session_id: String,
    pub origin_id: String,
    pub source: String,
    pub source_item_id: String,
    pub track_id: Option<i64>,
    pub isrc: Option<String>,
    pub recording_mbid: Option<String>,
    pub title: String,
    pub artist: String,
    pub duration_ms: u64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub played_ms: u64,
    pub end_position_ms: Option<u64>,
    pub end_reason: Option<EndReason>,
    pub context_kind: String,
    pub context_id: String,
}

impl ListenStore {
    /// Open (or create) the store under `user_dir`.
    pub fn open_at(user_dir: &Path) -> Result<Self, String> {
        let dir = user_dir.join(DB_SUBDIR);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create listen dir: {e}"))?;
        let path = dir.join(DB_FILE);
        let conn =
            Connection::open(&path).map_err(|e| format!("Failed to open listen log: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=2000;",
        )
        .map_err(|e| format!("Failed to set listen log pragmas: {e}"))?;
        let store = Self {
            conn,
            path: Some(path),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory()
            .map_err(|e| format!("Failed to open in-memory listen log: {e}"))?;
        let store = Self { conn, path: None };
        store.migrate()?;
        Ok(store)
    }

    /// Idempotent: creates the schema on a fresh file and records
    /// `schema_version`; a later version adds its `ALTER`s behind a version
    /// check here (a table is only new once — see `playlist_play_history`).
    fn migrate(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS listen_meta (
                    key   TEXT PRIMARY KEY,
                    value TEXT
                );
                CREATE TABLE IF NOT EXISTS listen_events (
                    id               INTEGER PRIMARY KEY,
                    app_session_id   TEXT    NOT NULL,
                    origin_id        TEXT    NOT NULL,
                    source           TEXT    NOT NULL,
                    source_item_id   TEXT    NOT NULL,
                    track_id         INTEGER,
                    album_id         TEXT,
                    artist_id        TEXT,
                    isrc             TEXT,
                    recording_mbid   TEXT,
                    title            TEXT    NOT NULL,
                    artist           TEXT    NOT NULL,
                    album            TEXT,
                    album_artist     TEXT,
                    artwork_key      TEXT,
                    duration_ms      INTEGER NOT NULL DEFAULT 0,
                    started_at       INTEGER NOT NULL,
                    ended_at         INTEGER,
                    played_ms        INTEGER NOT NULL DEFAULT 0,
                    end_position_ms  INTEGER,
                    end_reason       TEXT,
                    context_kind     TEXT    NOT NULL DEFAULT '',
                    context_id       TEXT    NOT NULL DEFAULT '',
                    bit_depth        INTEGER,
                    sample_rate      INTEGER,
                    output_backend   TEXT
                );
                CREATE INDEX IF NOT EXISTS ix_le_started ON listen_events(started_at);
                CREATE INDEX IF NOT EXISTS ix_le_item    ON listen_events(source, source_item_id, started_at);
                CREATE INDEX IF NOT EXISTS ix_le_isrc    ON listen_events(isrc) WHERE isrc IS NOT NULL;
                CREATE INDEX IF NOT EXISTS ix_le_open    ON listen_events(id) WHERE end_reason IS NULL;
                "#,
            )
            .map_err(|e| format!("Failed to create listen log schema: {e}"))?;

        let current = self
            .meta("schema_version")?
            .and_then(|v| v.parse::<i64>().ok());
        match current {
            None => self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?,
            Some(v) if v > SCHEMA_VERSION => {
                return Err(format!(
                    "listen log schema {v} is newer than this build ({SCHEMA_VERSION})"
                ));
            }
            // Future: `Some(v) if v < SCHEMA_VERSION` → ALTERs, then bump.
            Some(_) => {}
        }
        // Defaults that must exist from day one so readers never special-case
        // an absent key: unlimited retention (owner decision §10.4).
        if self.meta("retention_days")?.is_none() {
            self.set_meta("retention_days", "0")?;
        }
        Ok(())
    }

    // ----- meta -----------------------------------------------------------

    pub fn meta(&self, key: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT value FROM listen_meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|v| v.flatten())
            .map_err(|e| format!("Failed to read listen meta {key}: {e}"))
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO listen_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to write listen meta {key}: {e}"))
    }

    pub fn schema_version(&self) -> Result<i64, String> {
        Ok(self
            .meta("schema_version")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// The install's origin id, generated once and kept in `listen_meta`.
    /// `qbzd` passes its own fixed id instead (`qbzd:<host>`).
    pub fn origin_id_or_init(&self, generate: impl FnOnce() -> String) -> Result<String, String> {
        if let Some(id) = self.meta("origin_id")? {
            return Ok(id);
        }
        let id = generate();
        self.set_meta("origin_id", &id)?;
        Ok(id)
    }

    /// `paused = 1` means "Listening history" is OFF: nothing is written
    /// (and nothing is deleted).
    pub fn is_paused(&self) -> Result<bool, String> {
        Ok(self.meta("paused")?.as_deref() == Some("1"))
    }

    pub fn set_paused(&self, paused: bool) -> Result<(), String> {
        self.set_meta("paused", if paused { "1" } else { "0" })
    }

    pub fn retention_days(&self) -> Result<u32, String> {
        Ok(self
            .meta("retention_days")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    // ----- events ---------------------------------------------------------

    /// Insert an open row; returns its id.
    pub fn open_event(
        &self,
        app_session_id: &str,
        origin_id: &str,
        meta: &ListenMeta,
        started_at: i64,
    ) -> Result<i64, String> {
        self.conn
            .execute(
                "INSERT INTO listen_events (
                    app_session_id, origin_id, source, source_item_id, track_id,
                    album_id, artist_id, isrc, recording_mbid,
                    title, artist, album, album_artist, artwork_key,
                    duration_ms, started_at, context_kind, context_id,
                    bit_depth, sample_rate, output_backend
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                params![
                    app_session_id,
                    origin_id,
                    meta.source,
                    meta.source_item_id,
                    meta.track_id,
                    meta.album_id,
                    meta.artist_id,
                    meta.isrc,
                    meta.recording_mbid,
                    meta.title,
                    meta.artist,
                    meta.album,
                    meta.album_artist,
                    meta.artwork_key,
                    meta.duration_ms as i64,
                    started_at,
                    meta.context_kind,
                    meta.context_id,
                    meta.bit_depth,
                    meta.sample_rate,
                    meta.output_backend,
                ],
            )
            .map_err(|e| format!("Failed to open listen event: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Persist the accumulator of an OPEN row (a crash then loses at most
    /// one flush interval). Never touches a closed row.
    pub fn flush_progress(&self, f: &Flush) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE listen_events SET played_ms = ?1, end_position_ms = ?2
                 WHERE id = ?3 AND end_reason IS NULL",
                params![
                    f.played_ms as i64,
                    f.end_position_ms.map(|v| v as i64),
                    f.event_id
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to flush listen progress: {e}"))
    }

    /// Close a row. Idempotent on an already-closed row (no-op).
    pub fn close_event(&self, c: &Closed) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE listen_events
                 SET ended_at = ?1, played_ms = ?2, end_position_ms = ?3, end_reason = ?4
                 WHERE id = ?5 AND end_reason IS NULL",
                params![
                    c.ended_at,
                    c.played_ms as i64,
                    c.end_position_ms.map(|v| v as i64),
                    c.reason.as_str(),
                    c.event_id
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to close listen event: {e}"))
    }

    /// Rows left open by a crash (or a kill) close as `shutdown` with the
    /// `played_ms` that reached disk, at startup. `ended_at` is unknowable —
    /// it is set to `started_at + played_ms` as the best lower bound.
    pub fn close_orphans_as_shutdown(&self) -> Result<usize, String> {
        self.conn
            .execute(
                "UPDATE listen_events
                 SET end_reason = ?1, ended_at = started_at + (played_ms / 1000)
                 WHERE end_reason IS NULL",
                params![EndReason::Shutdown.as_str()],
            )
            .map_err(|e| format!("Failed to close orphan listen events: {e}"))
    }

    /// "Clear listening history": every event goes, the file is compacted.
    /// `listen_meta` (origin id, paused flag, retention) is kept.
    pub fn clear(&self) -> Result<(), String> {
        // Under WAL the vacuumed image lands in the -wal file first; the
        // TRUNCATE checkpoint is what makes the main file actually shrink
        // (otherwise "Clear" leaves the old size on disk until a later
        // checkpoint happens to run).
        self.conn
            .execute_batch("DELETE FROM listen_events; VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| format!("Failed to clear listen log: {e}"))
    }

    pub fn count(&self) -> Result<u64, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM listen_events", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|n| n as u64)
            .map_err(|e| format!("Failed to count listen events: {e}"))
    }

    /// Every row in id order (tests + developer inspection; never a rail).
    pub fn rows(&self) -> Result<Vec<ListenRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, app_session_id, origin_id, source, source_item_id, track_id,
                        isrc, recording_mbid, title, artist, duration_ms, started_at,
                        ended_at, played_ms, end_position_ms, end_reason,
                        context_kind, context_id
                 FROM listen_events ORDER BY id",
            )
            .map_err(|e| format!("Failed to prepare listen query: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ListenRow {
                    id: row.get(0)?,
                    app_session_id: row.get(1)?,
                    origin_id: row.get(2)?,
                    source: row.get(3)?,
                    source_item_id: row.get(4)?,
                    track_id: row.get(5)?,
                    isrc: row.get(6)?,
                    recording_mbid: row.get(7)?,
                    title: row.get(8)?,
                    artist: row.get(9)?,
                    duration_ms: row.get::<_, i64>(10)? as u64,
                    started_at: row.get(11)?,
                    ended_at: row.get(12)?,
                    played_ms: row.get::<_, i64>(13)? as u64,
                    end_position_ms: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
                    end_reason: row
                        .get::<_, Option<String>>(15)?
                        .as_deref()
                        .and_then(EndReason::parse),
                    context_kind: row.get(16)?,
                    context_id: row.get(17)?,
                })
            })
            .map_err(|e| format!("Failed to query listen events: {e}"))?;
        Ok(rows.flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> ListenMeta {
        ListenMeta {
            source: "qobuz".into(),
            source_item_id: "42".into(),
            track_id: Some(42),
            title: "Song".into(),
            artist: "Band".into(),
            album: Some("LP".into()),
            duration_ms: 200_000,
            context_kind: "album".into(),
            context_id: "abc".into(),
            isrc: Some("USRC17607839".into()),
            ..Default::default()
        }
    }

    #[test]
    fn schema_version_is_recorded_and_reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        {
            let s = ListenStore::open_at(dir.path()).unwrap();
            assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
            assert_eq!(s.retention_days().unwrap(), 0);
            assert!(!s.is_paused().unwrap());
        }
        let s = ListenStore::open_at(dir.path()).unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        assert!(dir.path().join(DB_SUBDIR).join(DB_FILE).exists());
    }

    #[test]
    fn a_newer_schema_refuses_to_open() {
        let dir = tempfile::tempdir().unwrap();
        {
            let s = ListenStore::open_at(dir.path()).unwrap();
            s.set_meta("schema_version", &(SCHEMA_VERSION + 1).to_string())
                .unwrap();
        }
        assert!(ListenStore::open_at(dir.path()).is_err());
    }

    #[test]
    fn origin_id_is_generated_once() {
        let s = ListenStore::open_in_memory().unwrap();
        let a = s.origin_id_or_init(|| "first".into()).unwrap();
        let b = s.origin_id_or_init(|| "second".into()).unwrap();
        assert_eq!(a, "first");
        assert_eq!(b, "first");
    }

    #[test]
    fn open_flush_close_round_trip() {
        let s = ListenStore::open_in_memory().unwrap();
        let id = s.open_event("sess", "origin", &meta(), 1_000).unwrap();
        s.flush_progress(&Flush {
            event_id: id,
            played_ms: 10_000,
            end_position_ms: Some(10_000),
        })
        .unwrap();
        let row = &s.rows().unwrap()[0];
        assert_eq!(row.played_ms, 10_000);
        assert_eq!(row.end_reason, None);
        assert_eq!(row.isrc.as_deref(), Some("USRC17607839"));
        assert_eq!(row.context_kind, "album");
        s.close_event(&Closed {
            event_id: id,
            reason: EndReason::Skip,
            played_ms: 12_000,
            end_position_ms: Some(12_000),
            ended_at: 1_012,
        })
        .unwrap();
        let row = &s.rows().unwrap()[0];
        assert_eq!(row.end_reason, Some(EndReason::Skip));
        assert_eq!(row.played_ms, 12_000);
        assert_eq!(row.ended_at, Some(1_012));
        // A late flush must not reopen or alter a closed row.
        s.flush_progress(&Flush {
            event_id: id,
            played_ms: 99_000,
            end_position_ms: None,
        })
        .unwrap();
        assert_eq!(s.rows().unwrap()[0].played_ms, 12_000);
    }

    #[test]
    fn orphans_close_as_shutdown_with_the_flushed_progress() {
        let s = ListenStore::open_in_memory().unwrap();
        let a = s.open_event("sess", "o", &meta(), 1_000).unwrap();
        s.flush_progress(&Flush {
            event_id: a,
            played_ms: 30_000,
            end_position_ms: Some(30_000),
        })
        .unwrap();
        let b = s.open_event("sess", "o", &meta(), 2_000).unwrap();
        s.close_event(&Closed {
            event_id: b,
            reason: EndReason::Natural,
            played_ms: 200_000,
            end_position_ms: Some(200_000),
            ended_at: 2_200,
        })
        .unwrap();
        assert_eq!(s.close_orphans_as_shutdown().unwrap(), 1);
        let rows = s.rows().unwrap();
        assert_eq!(rows[0].end_reason, Some(EndReason::Shutdown));
        assert_eq!(rows[0].played_ms, 30_000);
        assert_eq!(rows[0].ended_at, Some(1_030));
        assert_eq!(rows[1].end_reason, Some(EndReason::Natural));
    }

    #[test]
    fn clear_empties_events_and_keeps_meta() {
        let s = ListenStore::open_in_memory().unwrap();
        s.set_paused(false).unwrap();
        let _ = s.origin_id_or_init(|| "keep-me".into()).unwrap();
        s.open_event("sess", "o", &meta(), 1).unwrap();
        s.open_event("sess", "o", &meta(), 2).unwrap();
        assert_eq!(s.count().unwrap(), 2);
        s.clear().unwrap();
        assert_eq!(s.count().unwrap(), 0);
        assert_eq!(s.meta("origin_id").unwrap().as_deref(), Some("keep-me"));
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn vacuum_compacts_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = ListenStore::open_at(dir.path()).unwrap();
        for i in 0..2_000 {
            s.open_event("sess", "o", &meta(), i).unwrap();
        }
        let path = dir.path().join(DB_SUBDIR).join(DB_FILE);
        // Checkpoint so the rows are in the main file, not only the WAL.
        s.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        let before = std::fs::metadata(&path).unwrap().len();
        s.clear().unwrap();
        let after = std::fs::metadata(&path).unwrap().len();
        assert!(
            after < before,
            "vacuum did not shrink the file ({before} -> {after})"
        );
    }
}
