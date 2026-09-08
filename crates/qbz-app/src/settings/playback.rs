//! Playback and small playback-adjacent UI preferences.
//!
//! `show_context_icon` is persisted here to preserve the existing settings
//! contract, but it is a portable UI preference, not playback domain logic.

use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoplayMode {
    /// Continue playing within the source (album, playlist, etc.)
    #[serde(rename = "continue")]
    ContinueWithinSource,
    /// Play only the selected track, then stop.
    #[serde(rename = "track_only")]
    PlayTrackOnly,
    /// Create infinite radio when queue ends (based on recent tracks).
    #[serde(rename = "infinite")]
    InfiniteRadio,
}

impl Default for AutoplayMode {
    fn default() -> Self {
        Self::ContinueWithinSource
    }
}

impl AutoplayMode {
    fn to_db_value(self) -> &'static str {
        match self {
            AutoplayMode::ContinueWithinSource => "continue",
            AutoplayMode::PlayTrackOnly => "track_only",
            AutoplayMode::InfiniteRadio => "infinite",
        }
    }

    fn from_db_value(value: &str) -> Self {
        match value {
            "track_only" => AutoplayMode::PlayTrackOnly,
            "infinite" => AutoplayMode::InfiniteRadio,
            _ => AutoplayMode::ContinueWithinSource,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackPreferences {
    pub autoplay_mode: AutoplayMode,
    /// Portable UI preference for showing the context-stack icon.
    /// This is not domain playback behavior.
    pub show_context_icon: bool,
    pub persist_session: bool,
    /// Sub-preference of `persist_session`. When true, restoring a
    /// session also seeks to `current_position_secs` of the saved
    /// track. When false (default), the saved track is shown paused at
    /// 0:00 and the user starts the next listen fresh.
    pub resume_playback_position: bool,
}

impl Default for PlaybackPreferences {
    fn default() -> Self {
        Self {
            autoplay_mode: AutoplayMode::ContinueWithinSource,
            // Playback preferences are opt-out: on by default.
            show_context_icon: true,
            persist_session: true,
            resume_playback_position: true,
        }
    }
}

pub struct PlaybackPreferencesStore {
    conn: Connection,
}

impl PlaybackPreferencesStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open playback preferences database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| {
                format!(
                    "Failed to enable WAL for playback preferences database: {}",
                    e
                )
            })?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS playback_preferences (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                autoplay_mode TEXT NOT NULL DEFAULT 'continue'
            );",
        )
        .map_err(|e| format!("Failed to create playback preferences table: {}", e))?;

        let show_context_icon_exists =
            column_exists(&conn, "playback_preferences", "show_context_icon");
        info!(
            "[PlaybackPrefs] Column show_context_icon exists: {}",
            show_context_icon_exists
        );
        if !show_context_icon_exists {
            info!("[PlaybackPrefs] Migrating: adding show_context_icon column");
            conn.execute(
                "ALTER TABLE playback_preferences ADD COLUMN show_context_icon INTEGER NOT NULL DEFAULT 1",
                [],
            )
            .map_err(|e| format!("Failed to add show_context_icon column: {}", e))?;
            info!("[PlaybackPrefs] Migration successful");
        }

        if !column_exists(&conn, "playback_preferences", "persist_session") {
            info!("[PlaybackPrefs] Migrating: adding persist_session column");
            conn.execute(
                "ALTER TABLE playback_preferences ADD COLUMN persist_session INTEGER NOT NULL DEFAULT 1",
                [],
            )
            .map_err(|e| format!("Failed to add persist_session column: {}", e))?;
            info!("[PlaybackPrefs] persist_session migration successful");
        }

        if !column_exists(&conn, "playback_preferences", "resume_playback_position") {
            info!("[PlaybackPrefs] Migrating: adding resume_playback_position column");
            conn.execute(
                "ALTER TABLE playback_preferences ADD COLUMN resume_playback_position INTEGER NOT NULL DEFAULT 1",
                [],
            )
            .map_err(|e| format!("Failed to add resume_playback_position column: {}", e))?;
            info!("[PlaybackPrefs] resume_playback_position migration successful");
        }

        conn.execute(
            "INSERT OR IGNORE INTO playback_preferences (id, autoplay_mode, show_context_icon, persist_session, resume_playback_position)
            VALUES (1, 'continue', 1, 1, 1)",
            [],
        )
        .map_err(|e| format!("Failed to insert default preferences: {}", e))?;

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "playback_preferences.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "playback_preferences.db")
    }

    pub fn get_preferences(&self) -> Result<PlaybackPreferences, String> {
        self.conn
            .query_row(
                "SELECT autoplay_mode, show_context_icon, persist_session, resume_playback_position FROM playback_preferences WHERE id = 1",
                [],
                |row| {
                    let autoplay_str: String = row.get(0)?;
                    let show_icon: i32 = row.get(1)?;
                    let persist: i32 = row.get(2)?;
                    let resume_pos: i32 = row.get(3)?;
                    Ok(PlaybackPreferences {
                        autoplay_mode: AutoplayMode::from_db_value(&autoplay_str),
                        show_context_icon: show_icon != 0,
                        persist_session: persist != 0,
                        resume_playback_position: resume_pos != 0,
                    })
                },
            )
            .map_err(|e| format!("Failed to get playback preferences: {}", e))
    }

    pub fn set_autoplay_mode(&self, mode: AutoplayMode) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE playback_preferences SET autoplay_mode = ?1 WHERE id = 1",
                params![mode.to_db_value()],
            )
            .map_err(|e| format!("Failed to set autoplay mode: {}", e))?;
        Ok(())
    }

    pub fn set_show_context_icon(&self, show: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE playback_preferences SET show_context_icon = ?1 WHERE id = 1",
                params![if show { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set show context icon: {}", e))?;
        Ok(())
    }

    pub fn set_persist_session(&self, persist: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE playback_preferences SET persist_session = ?1 WHERE id = 1",
                params![if persist { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set persist session: {}", e))?;
        Ok(())
    }

    pub fn set_resume_playback_position(&self, resume: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE playback_preferences SET resume_playback_position = ?1 WHERE id = 1",
                params![if resume { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set resume playback position: {}", e))?;
        Ok(())
    }

    /// Reset all playback preferences to their default values.
    pub fn reset_all(&self) -> Result<PlaybackPreferences, String> {
        let defaults = PlaybackPreferences::default();
        self.conn
            .execute(
                "UPDATE playback_preferences SET autoplay_mode = ?1, show_context_icon = ?2, persist_session = ?3, resume_playback_position = ?4 WHERE id = 1",
                params![
                    defaults.autoplay_mode.to_db_value(),
                    if defaults.show_context_icon { 1 } else { 0 },
                    if defaults.persist_session { 1 } else { 0 },
                    if defaults.resume_playback_position { 1 } else { 0 },
                ],
            )
            .map_err(|e| format!("Failed to reset playback preferences: {}", e))?;
        Ok(defaults)
    }
}

pub struct PlaybackPreferencesState {
    pub store: Arc<Mutex<Option<PlaybackPreferencesStore>>>,
}

impl PlaybackPreferencesState {
    pub fn new() -> Result<Self, String> {
        let store = PlaybackPreferencesStore::new()?;
        Ok(Self {
            store: Arc::new(Mutex::new(Some(store))),
        })
    }

    pub fn new_empty() -> Self {
        Self {
            store: Arc::new(Mutex::new(None)),
        }
    }

    pub fn init_at(&self, base_dir: &Path) -> Result<(), String> {
        let new_store = PlaybackPreferencesStore::new_at(base_dir)?;
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock playback preferences store".to_string())?;
        *guard = Some(new_store);
        Ok(())
    }

    pub fn teardown(&self) -> Result<(), String> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock playback preferences store".to_string())?;
        *guard = None;
        Ok(())
    }

    pub fn get_preferences(&self) -> Result<PlaybackPreferences, String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock playback preferences store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.get_preferences()
    }

    pub fn set_autoplay_mode(&self, mode: AutoplayMode) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock playback preferences store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_autoplay_mode(mode)
    }

    pub fn set_show_context_icon(&self, show: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock playback preferences store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_show_context_icon(show)
    }

    pub fn set_persist_session(&self, persist: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock playback preferences store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_persist_session(persist)
    }

    pub fn set_resume_playback_position(&self, resume: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock playback preferences store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_resume_playback_position(resume)
    }
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
        [column],
        |row| {
            let count: i32 = row.get(0)?;
            Ok(count > 0)
        },
    )
    .unwrap_or(false)
}

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

    fn fresh_store(name: &str) -> (std::path::PathBuf, PlaybackPreferencesStore) {
        let dir = unique_test_dir(name);
        let store = PlaybackPreferencesStore::new_at(&dir).expect("open store in temp dir");
        (dir, store)
    }

    #[test]
    fn playback_preferences_default_values_are_stable() {
        let prefs = PlaybackPreferences::default();

        assert_eq!(prefs.autoplay_mode, AutoplayMode::ContinueWithinSource);
        assert!(prefs.show_context_icon);
        assert!(prefs.persist_session);
        assert!(prefs.resume_playback_position);
    }

    #[test]
    fn playback_preferences_store_returns_defaults() {
        let (dir, store) = fresh_store("playback-default");

        let prefs = store.get_preferences().expect("get prefs");

        assert_eq!(prefs.autoplay_mode, AutoplayMode::ContinueWithinSource);
        assert!(prefs.show_context_icon);
        assert!(prefs.persist_session);
        assert!(prefs.resume_playback_position);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn playback_preferences_persist_all_fields() {
        let dir = unique_test_dir("playback-persist");
        {
            let store = PlaybackPreferencesStore::new_at(&dir).expect("open store");
            store
                .set_autoplay_mode(AutoplayMode::InfiniteRadio)
                .expect("set autoplay");
            store.set_show_context_icon(true).expect("set context icon");
            store
                .set_persist_session(true)
                .expect("set persist session");
            store
                .set_resume_playback_position(true)
                .expect("set resume position");
        }

        let reopened = PlaybackPreferencesStore::new_at(&dir).expect("reopen store");
        let prefs = reopened.get_preferences().expect("get prefs");

        assert_eq!(prefs.autoplay_mode, AutoplayMode::InfiniteRadio);
        assert!(prefs.show_context_icon);
        assert!(prefs.persist_session);
        assert!(prefs.resume_playback_position);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn playback_preferences_migrates_legacy_schema() {
        let dir = unique_test_dir("playback-migrate");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("playback_preferences.db");
        {
            let conn = Connection::open(&db_path).expect("open legacy db");
            conn.execute_batch(
                "CREATE TABLE playback_preferences (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    autoplay_mode TEXT NOT NULL DEFAULT 'continue'
                );
                INSERT INTO playback_preferences (id, autoplay_mode) VALUES (1, 'track_only');",
            )
            .expect("create legacy schema");
        }

        let store = PlaybackPreferencesStore::new_at(&dir).expect("migrate store");
        let prefs = store.get_preferences().expect("get prefs");

        assert_eq!(prefs.autoplay_mode, AutoplayMode::PlayTrackOnly);
        assert!(prefs.show_context_icon);
        assert!(prefs.persist_session);
        assert!(prefs.resume_playback_position);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn playback_preferences_reset_all_preserves_existing_behavior() {
        let (dir, store) = fresh_store("playback-reset");
        store
            .set_autoplay_mode(AutoplayMode::InfiniteRadio)
            .expect("set autoplay");
        store.set_show_context_icon(true).expect("set context icon");
        store.set_persist_session(true).expect("set persist");
        store
            .set_resume_playback_position(true)
            .expect("set resume position");

        let defaults = store.reset_all().expect("reset prefs");
        let prefs = store.get_preferences().expect("get prefs");

        assert_eq!(defaults.autoplay_mode, AutoplayMode::ContinueWithinSource);
        assert_eq!(prefs.autoplay_mode, AutoplayMode::ContinueWithinSource);
        assert!(prefs.show_context_icon);
        assert!(prefs.persist_session);
        assert!(prefs.resume_playback_position);
        let _ = std::fs::remove_dir_all(dir);
    }
}
