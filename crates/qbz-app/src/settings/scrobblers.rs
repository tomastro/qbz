//! Portable scrobbler credential storage (Last.fm + ListenBrainz).
//!
//! Owns the persisted scrobbler settings for the Slint frontend, the part the
//! Tauri build kept in per-user localStorage:
//!   - the master enable + collapse UI flags,
//!   - Last.fm: session key + username + per-service enable flag
//!     (replaces the `qbz-lastfm-session-key` / `qbz-lastfm-scrobbling`
//!     localStorage keys — webview localStorage is unreachable from Slint,
//!     so Last.fm credentials cannot be shared with the Tauri build),
//!   - ListenBrainz: token + username + per-service enable flag. The token is
//!     ALSO written through to the shared `ListenBrainzCache.credentials` row
//!     by the Slint `scrobble` controller, so LB credentials DO stay shared
//!     with the Tauri build; this copy is the Slint UI's fast read.
//!
//! The Last.fm offline queue does NOT live here: it is the `scrobble_queue`
//! table in the shared per-user `offline_settings.db`
//! ([`crate::offline_mode::store::OfflineModeStore`]) — same rows Tauri queues
//! into and flushes from. The ListenBrainz offline queue is the
//! `ListenBrainzCache.listen_queue` (qbz-integrations).
//!
//! Mirrors the [`super::plex`] store: a single-row SQLite settings table
//! re-pointed at the active user's data directory via
//! [`ScrobblerSettingsState::init_at`] at login, so credentials are scoped per
//! Qobuz user.
//!
//! Runtime concerns — the auth flows, the now-playing + scrobble fire, the
//! `min(50% of duration, 240s)` timer, the offline flush — live in the Slint
//! `scrobble` controller and call the `qbz-integrations` clients directly.

use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrobblerSettings {
    /// Master toggle. When false, the whole scrobblers section body is hidden.
    /// Default OFF — integrations are opt-in.
    pub enabled: bool,
    /// Collapse chevron state. Body renders only when `enabled && !ui_collapsed`.
    pub ui_collapsed: bool,

    // --- Last.fm ---
    /// Per-service scrobbling enable flag (replaces `qbz-lastfm-scrobbling`).
    pub lastfm_enabled: bool,
    /// Last.fm session key (`get_session().key`). Empty = not authenticated.
    pub lastfm_session_key: String,
    /// Last.fm username (`LastFmSession.name`), for the "Signed in as …" label.
    pub lastfm_username: String,

    // --- ListenBrainz ---
    /// Per-service enable flag.
    pub listenbrainz_enabled: bool,
    /// ListenBrainz user token. Empty = not authenticated.
    pub listenbrainz_token: String,
    /// ListenBrainz username (`UserInfo.user_name`).
    pub listenbrainz_username: String,

    /// Keep the independent scrobblers active without a Qobuz session.
    /// Default ON: Qobuz authentication is unrelated to Last.fm/LB auth.
    pub allow_logged_out_scrobbling: bool,
}

impl Default for ScrobblerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ui_collapsed: false,
            lastfm_enabled: false,
            lastfm_session_key: String::new(),
            lastfm_username: String::new(),
            listenbrainz_enabled: false,
            listenbrainz_token: String::new(),
            listenbrainz_username: String::new(),
            allow_logged_out_scrobbling: true,
        }
    }
}

impl ScrobblerSettings {
    /// Last.fm has credentials (independent of the enable flags).
    pub fn lastfm_is_authed(&self) -> bool {
        !self.lastfm_session_key.is_empty()
    }

    /// ListenBrainz has credentials (independent of the enable flags).
    pub fn listenbrainz_is_authed(&self) -> bool {
        !self.listenbrainz_token.is_empty()
    }

    /// Last.fm should actually scrobble: master + service on + authed.
    pub fn lastfm_active(&self) -> bool {
        self.enabled && self.lastfm_enabled && self.lastfm_is_authed()
    }

    /// ListenBrainz should actually scrobble: master + service on + authed.
    pub fn listenbrainz_active(&self) -> bool {
        self.enabled && self.listenbrainz_enabled && self.listenbrainz_is_authed()
    }

    /// Whether QBZ may READ the user's listening history from an external
    /// service (Recommendations). ONE rule, the same as sending: connected
    /// AND scrobbling on. A connected-but-disabled account used to be read
    /// silently through `*_is_authed()`; the scrobbling toggle now governs
    /// both directions, and its Settings text says so.
    pub fn history_read_allowed(&self, service: HistoryService) -> bool {
        match service {
            HistoryService::LastFm => self.lastfm_active(),
            HistoryService::ListenBrainz => self.listenbrainz_active(),
        }
    }
}

/// The external services whose history Recommendations may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryService {
    LastFm,
    ListenBrainz,
}

pub struct ScrobblerSettingsStore {
    conn: Connection,
}

impl ScrobblerSettingsStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open scrobbler settings database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to enable WAL for scrobbler settings database: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scrobbler_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                enabled INTEGER NOT NULL DEFAULT 0,
                ui_collapsed INTEGER NOT NULL DEFAULT 0,
                lastfm_enabled INTEGER NOT NULL DEFAULT 0,
                lastfm_session_key TEXT NOT NULL DEFAULT '',
                lastfm_username TEXT NOT NULL DEFAULT '',
                listenbrainz_enabled INTEGER NOT NULL DEFAULT 0,
                listenbrainz_token TEXT NOT NULL DEFAULT '',
                listenbrainz_username TEXT NOT NULL DEFAULT '',
                allow_logged_out_scrobbling INTEGER NOT NULL DEFAULT 1
            );",
        )
        .map_err(|e| format!("Failed to create scrobbler settings table: {}", e))?;

        // Existing Slint/Qt profiles predate logged-out mode. Additive and
        // default-on, so their established scrobbler setup keeps working.
        let _ = conn.execute(
            "ALTER TABLE scrobbler_settings
             ADD COLUMN allow_logged_out_scrobbling INTEGER NOT NULL DEFAULT 1",
            [],
        );

        conn.execute(
            "INSERT OR IGNORE INTO scrobbler_settings
                (id, enabled, ui_collapsed, lastfm_enabled, lastfm_session_key,
                 lastfm_username, listenbrainz_enabled, listenbrainz_token,
                 listenbrainz_username)
             VALUES (1, 0, 0, 0, '', '', 0, '', '')",
            [],
        )
        .map_err(|e| format!("Failed to insert default scrobbler settings: {}", e))?;

        info!("[ScrobblerSettings] Database initialized");

        Ok(Self { conn })
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "scrobbler_settings.db")
    }

    pub fn get_settings(&self) -> Result<ScrobblerSettings, String> {
        self.conn
            .query_row(
                "SELECT enabled, ui_collapsed, lastfm_enabled, lastfm_session_key,
                        lastfm_username, listenbrainz_enabled, listenbrainz_token,
                        listenbrainz_username, allow_logged_out_scrobbling
                 FROM scrobbler_settings WHERE id = 1",
                [],
                |row| {
                    let enabled: i32 = row.get(0)?;
                    let ui_collapsed: i32 = row.get(1)?;
                    let lastfm_enabled: i32 = row.get(2)?;
                    let lastfm_session_key: String = row.get(3)?;
                    let lastfm_username: String = row.get(4)?;
                    let listenbrainz_enabled: i32 = row.get(5)?;
                    let listenbrainz_token: String = row.get(6)?;
                    let listenbrainz_username: String = row.get(7)?;
                    let allow_logged_out_scrobbling: i32 = row.get(8)?;
                    Ok(ScrobblerSettings {
                        enabled: enabled != 0,
                        ui_collapsed: ui_collapsed != 0,
                        lastfm_enabled: lastfm_enabled != 0,
                        lastfm_session_key,
                        lastfm_username,
                        listenbrainz_enabled: listenbrainz_enabled != 0,
                        listenbrainz_token,
                        listenbrainz_username,
                        allow_logged_out_scrobbling: allow_logged_out_scrobbling != 0,
                    })
                },
            )
            .map_err(|e| format!("Failed to get scrobbler settings: {}", e))
    }

    pub fn set_enabled(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE scrobbler_settings SET enabled = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set scrobbler enabled: {}", e))?;
        Ok(())
    }

    pub fn set_ui_collapsed(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE scrobbler_settings SET ui_collapsed = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set scrobbler ui_collapsed: {}", e))?;
        Ok(())
    }

    pub fn set_allow_logged_out_scrobbling(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE scrobbler_settings
                 SET allow_logged_out_scrobbling = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set logged-out scrobbling: {}", e))?;
        Ok(())
    }

    // --- Last.fm ---

    pub fn set_lastfm_enabled(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE scrobbler_settings SET lastfm_enabled = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set lastfm_enabled: {}", e))?;
        Ok(())
    }

    /// Persist the Last.fm session key + username together (after `get_session`).
    pub fn set_lastfm_session(&self, key: &str, username: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE scrobbler_settings
                 SET lastfm_session_key = ?1, lastfm_username = ?2 WHERE id = 1",
                params![key.trim(), username.trim()],
            )
            .map_err(|e| format!("Failed to set lastfm session: {}", e))?;
        Ok(())
    }

    /// Sign out of Last.fm: clear key + username, keep `lastfm_enabled` flag.
    pub fn disconnect_lastfm(&self) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE scrobbler_settings
                 SET lastfm_session_key = '', lastfm_username = '' WHERE id = 1",
                [],
            )
            .map_err(|e| format!("Failed to disconnect Last.fm: {}", e))?;
        Ok(())
    }

    // --- ListenBrainz ---

    pub fn set_listenbrainz_enabled(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE scrobbler_settings SET listenbrainz_enabled = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set listenbrainz_enabled: {}", e))?;
        Ok(())
    }

    /// Persist the ListenBrainz token + username together (after `set_token`).
    pub fn set_listenbrainz_token(&self, token: &str, username: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE scrobbler_settings
                 SET listenbrainz_token = ?1, listenbrainz_username = ?2 WHERE id = 1",
                params![token.trim(), username.trim()],
            )
            .map_err(|e| format!("Failed to set listenbrainz token: {}", e))?;
        Ok(())
    }

    /// Sign out of ListenBrainz: clear token + username, keep enable flag.
    pub fn disconnect_listenbrainz(&self) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE scrobbler_settings
                 SET listenbrainz_token = '', listenbrainz_username = '' WHERE id = 1",
                [],
            )
            .map_err(|e| format!("Failed to disconnect ListenBrainz: {}", e))?;
        Ok(())
    }
}

pub struct ScrobblerSettingsState {
    pub store: Arc<Mutex<Option<ScrobblerSettingsStore>>>,
}

impl Default for ScrobblerSettingsState {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl ScrobblerSettingsState {
    pub fn new_empty() -> Self {
        Self {
            store: Arc::new(Mutex::new(None)),
        }
    }

    pub fn init_at(&self, base_dir: &Path) -> Result<(), String> {
        let new_store = ScrobblerSettingsStore::new_at(base_dir)?;
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock scrobbler settings store".to_string())?;
        *guard = Some(new_store);
        Ok(())
    }

    pub fn teardown(&self) -> Result<(), String> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock scrobbler settings store".to_string())?;
        *guard = None;
        Ok(())
    }

    fn with_store<T>(
        &self,
        f: impl FnOnce(&ScrobblerSettingsStore) -> Result<T, String>,
    ) -> Result<T, String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock scrobbler settings store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        f(store)
    }

    pub fn get_settings(&self) -> Result<ScrobblerSettings, String> {
        self.with_store(|s| s.get_settings())
    }

    pub fn set_enabled(&self, value: bool) -> Result<(), String> {
        self.with_store(|s| s.set_enabled(value))
    }

    pub fn set_ui_collapsed(&self, value: bool) -> Result<(), String> {
        self.with_store(|s| s.set_ui_collapsed(value))
    }

    pub fn set_allow_logged_out_scrobbling(&self, value: bool) -> Result<(), String> {
        self.with_store(|s| s.set_allow_logged_out_scrobbling(value))
    }

    pub fn set_lastfm_enabled(&self, value: bool) -> Result<(), String> {
        self.with_store(|s| s.set_lastfm_enabled(value))
    }

    pub fn set_lastfm_session(&self, key: &str, username: &str) -> Result<(), String> {
        self.with_store(|s| s.set_lastfm_session(key, username))
    }

    pub fn disconnect_lastfm(&self) -> Result<(), String> {
        self.with_store(|s| s.disconnect_lastfm())
    }

    pub fn set_listenbrainz_enabled(&self, value: bool) -> Result<(), String> {
        self.with_store(|s| s.set_listenbrainz_enabled(value))
    }

    pub fn set_listenbrainz_token(&self, token: &str, username: &str) -> Result<(), String> {
        self.with_store(|s| s.set_listenbrainz_token(token, username))
    }

    pub fn disconnect_listenbrainz(&self) -> Result<(), String> {
        self.with_store(|s| s.disconnect_listenbrainz())
    }
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

    #[test]
    fn scrobbler_settings_default_is_unconfigured() {
        let s = ScrobblerSettings::default();
        assert!(!s.enabled);
        assert!(!s.ui_collapsed);
        assert!(!s.lastfm_enabled);
        assert!(s.lastfm_session_key.is_empty());
        assert!(s.lastfm_username.is_empty());
        assert!(!s.listenbrainz_enabled);
        assert!(s.listenbrainz_token.is_empty());
        assert!(s.listenbrainz_username.is_empty());
        assert!(s.allow_logged_out_scrobbling);
        assert!(!s.lastfm_is_authed());
        assert!(!s.listenbrainz_is_authed());
        assert!(!s.lastfm_active());
        assert!(!s.listenbrainz_active());
    }

    #[test]
    fn scrobbler_store_returns_defaults() {
        let dir = unique_test_dir("scrobbler-default");
        let store = ScrobblerSettingsStore::new_at(&dir).expect("open store");
        let s = store.get_settings().expect("get settings");
        assert!(!s.enabled);
        assert!(s.allow_logged_out_scrobbling);
        assert!(!s.lastfm_is_authed());
        assert!(!s.listenbrainz_is_authed());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scrobbler_persists_all_fields() {
        let dir = unique_test_dir("scrobbler-persist");
        {
            let store = ScrobblerSettingsStore::new_at(&dir).expect("open store");
            store.set_enabled(true).expect("enabled");
            store.set_ui_collapsed(true).expect("collapsed");
            store
                .set_allow_logged_out_scrobbling(false)
                .expect("logged-out scrobbling");
            store.set_lastfm_enabled(true).expect("lfm enabled");
            store
                .set_lastfm_session("  sk-123  ", "  alice  ")
                .expect("lfm session");
            store.set_listenbrainz_enabled(true).expect("lb enabled");
            store
                .set_listenbrainz_token("  tok-456  ", "  bob  ")
                .expect("lb token");
        }
        let reopened = ScrobblerSettingsStore::new_at(&dir).expect("reopen store");
        let s = reopened.get_settings().expect("get settings");
        assert!(s.enabled);
        assert!(s.ui_collapsed);
        assert!(!s.allow_logged_out_scrobbling);
        assert!(s.lastfm_enabled);
        // set_lastfm_session trims whitespace.
        assert_eq!(s.lastfm_session_key, "sk-123");
        assert_eq!(s.lastfm_username, "alice");
        assert!(s.listenbrainz_enabled);
        assert_eq!(s.listenbrainz_token, "tok-456");
        assert_eq!(s.listenbrainz_username, "bob");
        assert!(s.lastfm_active());
        assert!(s.listenbrainz_active());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn existing_database_migrates_logged_out_scrobbling_default_on() {
        let dir = unique_test_dir("scrobbler-migrate-logged-out");
        std::fs::create_dir_all(&dir).unwrap();
        {
            let conn = Connection::open(dir.join("scrobbler_settings.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE scrobbler_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    enabled INTEGER NOT NULL DEFAULT 0,
                    ui_collapsed INTEGER NOT NULL DEFAULT 0,
                    lastfm_enabled INTEGER NOT NULL DEFAULT 0,
                    lastfm_session_key TEXT NOT NULL DEFAULT '',
                    lastfm_username TEXT NOT NULL DEFAULT '',
                    listenbrainz_enabled INTEGER NOT NULL DEFAULT 0,
                    listenbrainz_token TEXT NOT NULL DEFAULT '',
                    listenbrainz_username TEXT NOT NULL DEFAULT ''
                );
                INSERT INTO scrobbler_settings VALUES (1, 1, 0, 0, '', '', 0, '', '');",
            )
            .unwrap();
        }

        let store = ScrobblerSettingsStore::new_at(&dir).expect("migrate store");
        let settings = store.get_settings().expect("get migrated settings");
        assert!(settings.enabled);
        assert!(settings.allow_logged_out_scrobbling);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scrobbler_disconnect_keeps_enable_flags() {
        let dir = unique_test_dir("scrobbler-disconnect");
        let store = ScrobblerSettingsStore::new_at(&dir).expect("open store");
        store.set_enabled(true).expect("enabled");
        store.set_lastfm_enabled(true).expect("lfm enabled");
        store.set_lastfm_session("sk", "alice").expect("lfm session");
        store.set_listenbrainz_enabled(true).expect("lb enabled");
        store.set_listenbrainz_token("tok", "bob").expect("lb token");

        store.disconnect_lastfm().expect("disconnect lfm");
        store.disconnect_listenbrainz().expect("disconnect lb");

        let s = store.get_settings().expect("get settings");
        // Creds cleared.
        assert!(s.lastfm_session_key.is_empty());
        assert!(s.lastfm_username.is_empty());
        assert!(s.listenbrainz_token.is_empty());
        assert!(s.listenbrainz_username.is_empty());
        // Enable flags preserved (so re-auth resumes scrobbling).
        assert!(s.enabled);
        assert!(s.lastfm_enabled);
        assert!(s.listenbrainz_enabled);
        // No longer authed -> not active.
        assert!(!s.lastfm_active());
        assert!(!s.listenbrainz_active());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scrobbler_state_requires_init() {
        let state = ScrobblerSettingsState::new_empty();
        assert!(state.get_settings().is_err());

        let dir = unique_test_dir("scrobbler-state");
        state.init_at(&dir).expect("init at temp dir");
        state
            .set_lastfm_session("sk", "alice")
            .expect("set session via state");
        let s = state.get_settings().expect("get via state");
        assert!(s.lastfm_is_authed());
        assert_eq!(s.lastfm_username, "alice");

        state.teardown().expect("teardown");
        assert!(state.get_settings().is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
