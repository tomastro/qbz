//! Per-user offline-mode settings store.
//!
//! Opens the SAME `offline_settings.db` Tauri's `src-tauri/src/offline/mod.rs`
//! uses (identical creation SQL + additive migrations), so the file stays
//! frontend-portable like `library.db`/`index.db`. This shared store exposes
//! the subset the current desktop frontends consume:
//!
//! - `manual_offline_mode` — the induced-offline flag (persisted; D1).
//! - `show_network_folders_in_manual_offline` — network-mount policy (D9).
//! - `pre_offline_stream_first_track` — the issue #279 snapshot of
//!   `audio_settings.stream_first_track` taken on entering induced offline.
//! - the Tauri-era immediate/accumulated scrobbling policy flags.
//!
//! The remaining legacy columns/tables (`allow_cast_while_offline`,
//! `pending_playlist_sync`, `cache_limit_bytes`) are still CREATED for
//! byte-level compatibility with the Tauri schema. Offline playlist creation
//! is replaced by first-class local playlists (D7/D8).

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The offline-mode settings the port consumes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineModeSettings {
    pub manual_offline_mode: bool,
    pub show_network_folders_in_manual_offline: bool,
    /// Permit network submission while induced/manual offline is active.
    pub allow_immediate_scrobbling: bool,
    /// Retain scrobbles when they cannot be sent and flush them later.
    pub allow_accumulated_scrobbling: bool,
}

impl Default for OfflineModeSettings {
    fn default() -> Self {
        Self {
            manual_offline_mode: false,
            show_network_folders_in_manual_offline: false,
            // Historical Tauri defaults (f9ca1bd25).
            allow_immediate_scrobbling: false,
            allow_accumulated_scrobbling: true,
        }
    }
}

/// One row of the Last.fm offline scrobble queue (`scrobble_queue`). Mirrors
/// Tauri's `offline::QueuedScrobble` — same table, same per-user file, so
/// scrobbles queued by one frontend flush from the other.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedScrobble {
    pub id: i64,
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub timestamp: i64,
    pub created_at: i64,
    pub sent: bool,
}

pub struct OfflineModeStore {
    conn: Connection,
}

impl OfflineModeStore {
    /// Open (or create) `offline_settings.db` under `base_dir` — the per-user
    /// data directory, same location Tauri's `OfflineState::init_at` uses.
    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(base_dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;
        let db_path = base_dir.join("offline_settings.db");

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open offline settings database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to enable WAL for offline settings database: {}", e))?;

        // Base tables — kept IDENTICAL to the Tauri module so both frontends
        // can open the same per-user file.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS offline_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                manual_offline_mode INTEGER NOT NULL DEFAULT 0,
                show_partial_playlists INTEGER NOT NULL DEFAULT 1
            );
            INSERT OR IGNORE INTO offline_settings (id, manual_offline_mode, show_partial_playlists)
            VALUES (1, 0, 1);

            CREATE TABLE IF NOT EXISTS pending_playlist_sync (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                description TEXT,
                is_public INTEGER NOT NULL DEFAULT 0,
                track_ids TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                synced INTEGER NOT NULL DEFAULT 0,
                qobuz_playlist_id INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_pending_playlist_synced ON pending_playlist_sync(synced);

            CREATE TABLE IF NOT EXISTS scrobble_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                artist TEXT NOT NULL,
                track TEXT NOT NULL,
                album TEXT,
                timestamp INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                sent INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_scrobble_queue_sent ON scrobble_queue(sent);",
        )
        .map_err(|e| format!("Failed to create offline settings table: {}", e))?;

        // Additive migrations — same list as Tauri's; errors ignored because
        // the column may already exist.
        let migrations = [
            "ALTER TABLE offline_settings ADD COLUMN allow_cast_while_offline INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE offline_settings ADD COLUMN allow_immediate_scrobbling INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE offline_settings ADD COLUMN allow_accumulated_scrobbling INTEGER NOT NULL DEFAULT 1",
            "ALTER TABLE offline_settings ADD COLUMN show_network_folders_in_manual_offline INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE offline_settings ADD COLUMN pre_offline_stream_first_track INTEGER",
            "ALTER TABLE pending_playlist_sync ADD COLUMN local_track_ids TEXT",
            "ALTER TABLE pending_playlist_sync ADD COLUMN local_track_paths TEXT",
            "ALTER TABLE offline_settings ADD COLUMN cache_limit_bytes INTEGER",
        ];
        for migration in migrations {
            let _ = conn.execute(migration, []);
        }

        Ok(Self { conn })
    }

    pub fn get_settings(&self) -> Result<OfflineModeSettings, String> {
        self.conn
            .query_row(
                "SELECT manual_offline_mode,
                        COALESCE(show_network_folders_in_manual_offline, 0),
                        COALESCE(allow_immediate_scrobbling, 0),
                        COALESCE(allow_accumulated_scrobbling, 1)
                 FROM offline_settings WHERE id = 1",
                [],
                |row| {
                    Ok(OfflineModeSettings {
                        manual_offline_mode: row.get::<_, i64>(0)? != 0,
                        show_network_folders_in_manual_offline: row.get::<_, i64>(1)? != 0,
                        allow_immediate_scrobbling: row.get::<_, i64>(2)? != 0,
                        allow_accumulated_scrobbling: row.get::<_, i64>(3)? != 0,
                    })
                },
            )
            .map_err(|e| format!("Failed to get offline settings: {}", e))
    }

    pub fn set_manual_offline_mode(&self, enabled: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE offline_settings SET manual_offline_mode = ?1 WHERE id = 1",
                params![enabled as i64],
            )
            .map_err(|e| format!("Failed to set manual offline mode: {}", e))?;
        Ok(())
    }

    pub fn set_show_network_folders_in_manual_offline(&self, enabled: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE offline_settings SET show_network_folders_in_manual_offline = ?1 WHERE id = 1",
                params![enabled as i64],
            )
            .map_err(|e| format!("Failed to set show network folders in manual offline: {}", e))?;
        Ok(())
    }

    /// Tauri `set_allow_immediate_scrobbling`, with the later a68230fad
    /// mutual-exclusion rule made atomic instead of two UI-side writes.
    pub fn set_allow_immediate_scrobbling(&self, enabled: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE offline_settings
                 SET allow_immediate_scrobbling = ?1,
                     allow_accumulated_scrobbling = CASE
                         WHEN ?1 != 0 THEN 0 ELSE allow_accumulated_scrobbling END
                 WHERE id = 1",
                params![enabled as i64],
            )
            .map_err(|e| format!("Failed to set allow immediate scrobbling: {}", e))?;
        Ok(())
    }

    /// Tauri `set_allow_accumulated_scrobbling`, with the later a68230fad
    /// mutual-exclusion rule made atomic instead of two UI-side writes.
    pub fn set_allow_accumulated_scrobbling(&self, enabled: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE offline_settings
                 SET allow_accumulated_scrobbling = ?1,
                     allow_immediate_scrobbling = CASE
                         WHEN ?1 != 0 THEN 0 ELSE allow_immediate_scrobbling END
                 WHERE id = 1",
                params![enabled as i64],
            )
            .map_err(|e| format!("Failed to set allow accumulated scrobbling: {}", e))?;
        Ok(())
    }

    /// Issue #279 snapshot: the user's `stream_first_track` preference stashed
    /// when entering induced offline. `None` = no snapshot active.
    pub fn get_pre_offline_stream_first_track(&self) -> Result<Option<bool>, String> {
        self.conn
            .query_row(
                "SELECT pre_offline_stream_first_track FROM offline_settings WHERE id = 1",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map(|opt| opt.map(|v| v != 0))
            .map_err(|e| format!("Failed to read pre_offline_stream_first_track: {}", e))
    }

    /// Store (`Some`) on entering induced offline, clear (`None`) on exit.
    pub fn set_pre_offline_stream_first_track(&self, value: Option<bool>) -> Result<(), String> {
        let param: Option<i64> = value.map(|v| v as i64);
        self.conn
            .execute(
                "UPDATE offline_settings SET pre_offline_stream_first_track = ?1 WHERE id = 1",
                params![param],
            )
            .map_err(|e| format!("Failed to set pre_offline_stream_first_track: {}", e))?;
        Ok(())
    }

    // === Last.fm scrobble queue ===
    //
    // Minimal API over the `scrobble_queue` table the base schema already
    // creates. SQL kept IDENTICAL to Tauri's `src-tauri/src/offline/mod.rs`
    // scrobble-queue methods so both frontends interoperate on the same rows.

    /// Queue a scrobble for later submission to Last.fm.
    pub fn queue_scrobble(
        &self,
        artist: &str,
        track: &str,
        album: Option<&str>,
        timestamp: i64,
    ) -> Result<i64, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        self.conn
            .execute(
                "INSERT INTO scrobble_queue (artist, track, album, timestamp, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![artist, track, album, timestamp, now],
            )
            .map_err(|e| format!("Failed to queue scrobble: {}", e))?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Unsent scrobbles, oldest first (cap at 50 — the Last.fm batch limit).
    pub fn get_queued_scrobbles(&self, limit: u32) -> Result<Vec<QueuedScrobble>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, artist, track, album, timestamp, created_at, sent
                 FROM scrobble_queue WHERE sent = 0 ORDER BY timestamp ASC LIMIT ?1",
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let scrobbles = stmt
            .query_map(params![limit], |row| {
                Ok(QueuedScrobble {
                    id: row.get(0)?,
                    artist: row.get(1)?,
                    track: row.get(2)?,
                    album: row.get(3)?,
                    timestamp: row.get(4)?,
                    created_at: row.get(5)?,
                    sent: row.get::<_, i64>(6)? != 0,
                })
            })
            .map_err(|e| format!("Failed to query queued scrobbles: {}", e))?;

        scrobbles
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect queued scrobbles: {}", e))
    }

    /// Batch mark scrobbles as sent.
    pub fn mark_scrobbles_sent(&self, ids: &[i64]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }

        let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "UPDATE scrobble_queue SET sent = 1 WHERE id IN ({})",
            placeholders.join(",")
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let sql_params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        stmt.execute(sql_params.as_slice())
            .map_err(|e| format!("Failed to mark scrobbles as sent: {}", e))?;

        Ok(())
    }

    /// Delete sent scrobbles older than `older_than_days` (post-flush cleanup).
    pub fn cleanup_sent_scrobbles(&self, older_than_days: u32) -> Result<u32, String> {
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - (older_than_days as i64 * 24 * 60 * 60);

        let deleted = self
            .conn
            .execute(
                "DELETE FROM scrobble_queue WHERE sent = 1 AND created_at < ?1",
                params![cutoff],
            )
            .map_err(|e| format!("Failed to cleanup sent scrobbles: {}", e))?;

        Ok(deleted as u32)
    }

    /// Count of queued (unsent) scrobbles.
    pub fn queued_scrobble_count(&self) -> Result<u32, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM scrobble_queue WHERE sent = 0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count as u32)
            .map_err(|e| format!("Failed to count queued scrobbles: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("qbz-app-{name}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn defaults_are_online_and_no_network_folders() {
        let dir = unique_test_dir("offline-store-defaults");
        let store = OfflineModeStore::new_at(&dir).unwrap();

        let settings = store.get_settings().unwrap();
        assert!(!settings.manual_offline_mode);
        assert!(!settings.show_network_folders_in_manual_offline);
        assert!(!settings.allow_immediate_scrobbling);
        assert!(settings.allow_accumulated_scrobbling);
        assert_eq!(store.get_pre_offline_stream_first_track().unwrap(), None);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_flag_round_trips() {
        let dir = unique_test_dir("offline-store-manual");
        let store = OfflineModeStore::new_at(&dir).unwrap();

        store.set_manual_offline_mode(true).unwrap();
        assert!(store.get_settings().unwrap().manual_offline_mode);
        store.set_manual_offline_mode(false).unwrap();
        assert!(!store.get_settings().unwrap().manual_offline_mode);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn network_folders_flag_round_trips() {
        let dir = unique_test_dir("offline-store-netfolders");
        let store = OfflineModeStore::new_at(&dir).unwrap();

        store.set_show_network_folders_in_manual_offline(true).unwrap();
        assert!(
            store
                .get_settings()
                .unwrap()
                .show_network_folders_in_manual_offline
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scrobbling_policies_round_trip_and_are_mutually_exclusive() {
        let dir = unique_test_dir("offline-store-scrobble-policy");
        let store = OfflineModeStore::new_at(&dir).unwrap();

        store.set_allow_immediate_scrobbling(true).unwrap();
        let settings = store.get_settings().unwrap();
        assert!(settings.allow_immediate_scrobbling);
        assert!(!settings.allow_accumulated_scrobbling);

        // Turning a mode OFF does not silently opt into the other one.
        store.set_allow_immediate_scrobbling(false).unwrap();
        let settings = store.get_settings().unwrap();
        assert!(!settings.allow_immediate_scrobbling);
        assert!(!settings.allow_accumulated_scrobbling);

        store.set_allow_accumulated_scrobbling(true).unwrap();
        let settings = store.get_settings().unwrap();
        assert!(!settings.allow_immediate_scrobbling);
        assert!(settings.allow_accumulated_scrobbling);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scrobble_queue_round_trips_and_marks_sent() {
        let dir = unique_test_dir("offline-store-scrobbles");
        let store = OfflineModeStore::new_at(&dir).unwrap();

        let id1 = store
            .queue_scrobble("Artist A", "Track 1", Some("Album X"), 1000)
            .unwrap();
        let id2 = store.queue_scrobble("Artist B", "Track 2", None, 2000).unwrap();
        assert_ne!(id1, id2);
        assert_eq!(store.queued_scrobble_count().unwrap(), 2);

        let pending = store.get_queued_scrobbles(50).unwrap();
        assert_eq!(pending.len(), 2);
        // Oldest first.
        assert_eq!(pending[0].timestamp, 1000);
        assert_eq!(pending[0].artist, "Artist A");
        assert_eq!(pending[0].album.as_deref(), Some("Album X"));
        assert_eq!(pending[1].album, None);

        store.mark_scrobbles_sent(&[pending[0].id]).unwrap();
        assert_eq!(store.queued_scrobble_count().unwrap(), 1);
        let remaining = store.get_queued_scrobbles(50).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].timestamp, 2000);

        // Cleanup only touches SENT rows older than the cutoff — the fresh
        // unsent row always survives (the just-sent row's created_at is "now",
        // so it is not older than any cutoff either).
        let _ = store.cleanup_sent_scrobbles(7).unwrap();
        assert_eq!(store.queued_scrobble_count().unwrap(), 1);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stream_first_snapshot_round_trips() {
        let dir = unique_test_dir("offline-store-snapshot");
        let store = OfflineModeStore::new_at(&dir).unwrap();

        store.set_pre_offline_stream_first_track(Some(true)).unwrap();
        assert_eq!(store.get_pre_offline_stream_first_track().unwrap(), Some(true));
        store.set_pre_offline_stream_first_track(None).unwrap();
        assert_eq!(store.get_pre_offline_stream_first_track().unwrap(), None);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reopens_tauri_era_database_without_data_loss() {
        // Simulate a DB created by the original Tauri schema (pre-migration
        // base tables only), then reopen with this store: migrations must be
        // additive and the existing flag must survive.
        let dir = unique_test_dir("offline-store-compat");
        std::fs::create_dir_all(&dir).unwrap();
        {
            let conn = Connection::open(dir.join("offline_settings.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE offline_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    manual_offline_mode INTEGER NOT NULL DEFAULT 0,
                    show_partial_playlists INTEGER NOT NULL DEFAULT 1
                );
                INSERT INTO offline_settings (id, manual_offline_mode, show_partial_playlists)
                VALUES (1, 1, 1);",
            )
            .unwrap();
        }

        let store = OfflineModeStore::new_at(&dir).unwrap();
        let settings = store.get_settings().unwrap();
        assert!(settings.manual_offline_mode, "Tauri-era flag must survive");
        assert!(!settings.show_network_folders_in_manual_offline);
        assert!(!settings.allow_immediate_scrobbling);
        assert!(settings.allow_accumulated_scrobbling);

        let _ = std::fs::remove_dir_all(dir);
    }
}
