//! Portable Plex credential + library-selection storage.
//!
//! Owns the persisted Plex connection settings for the Slint frontend:
//! the master enable + collapse UI flags, the resolved base URL, auth token,
//! the manual-token toggle, the experimental metadata-write toggle, the list
//! of selected music-library section keys (plus a legacy single-key mirror),
//! the captured `machine_identifier`, and a stable generated `client_id`.
//! Runtime concerns — PIN auth, ping, browse, media resolution — live in the
//! `qbz-plex` core crate and the host application layer.
//!
//! Mirrors the [`super::tray`] store: a single-row SQLite table, opened
//! globally via [`PlexSettingsStore::new`] and re-pointed at the active
//! user's data directory via [`PlexSettingsState::init_at`] at login, so
//! credentials are scoped per Qobuz user (matching the Tauri frontend's
//! per-user `qbz-plex-poc-*` localStorage scoping).
//!
//! NOTE: this is a FRESH Slint-only store. Tauri is intentionally not wired
//! to it — the Tauri code is slated for deletion once the Slint port is
//! complete, so credentials are not shared and the user authenticates Plex
//! once in Slint. See
//! `qbz-nix-docs/plex-integration/2026-06-07-plex-slint-build-spec.md` §2.

use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlexSettings {
    /// Master toggle. When false, the whole Plex section body is hidden.
    /// Default OFF — integrations are opt-in.
    pub enabled: bool,
    /// Collapse chevron state. Body renders only when `enabled && !ui_collapsed`.
    pub ui_collapsed: bool,
    /// Plex server base URL, e.g. `http://127.0.0.1:32400`. Empty = not
    /// configured. This is the RESOLVED `proto://host:port`, not the raw
    /// host-form text the user typed.
    pub base_url: String,
    /// `X-Plex-Token`. Empty = not authenticated.
    pub token: String,
    /// When true, the user entered the token manually instead of using the
    /// PIN flow (controls which Settings affordance is shown).
    pub manual_token_mode: bool,
    /// Experimental metadata write-back toggle. Persisted but not consumed by
    /// slice 2 logic. Default OFF.
    pub metadata_write_enabled: bool,
    /// The list of picked music-library section keys (Plex `Directory@key`).
    /// Empty = none chosen yet (default-select ALL on first fetch).
    pub selected_section_keys: Vec<String>,
    /// Legacy single-section mirror = first element of `selected_section_keys`.
    /// Kept for backward/parallel compat; not the source of truth anymore.
    pub selected_section_key: String,
    /// `machine_identifier` captured from `plex_ping`; threaded as `server_id`
    /// into the cache save calls. Empty until first successful ping.
    pub machine_id: String,
    /// Generated `qbz-{uuid}` ONCE, persisted, reused for every PIN call.
    /// Empty until first authorize.
    pub client_id: String,
}

impl PlexSettings {
    /// A connection is usable only when both the base URL and token are set.
    pub fn is_configured(&self) -> bool {
        !self.base_url.is_empty() && !self.token.is_empty()
    }
}

pub struct PlexSettingsStore {
    conn: Connection,
}

impl PlexSettingsStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open Plex settings database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to enable WAL for Plex settings database: {}", e))?;

        // Base (slice 2a) schema. New columns are added by idempotent ALTERs
        // below so an existing 4-column 2a DB migrates without dropping creds.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS plex_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                base_url TEXT NOT NULL DEFAULT '',
                token TEXT NOT NULL DEFAULT '',
                manual_token_mode INTEGER NOT NULL DEFAULT 0,
                selected_section_key TEXT NOT NULL DEFAULT ''
            );",
        )
        .map_err(|e| format!("Failed to create Plex settings table: {}", e))?;

        // Idempotent ALTER-based migrations for the 6 new columns (mirrors the
        // tray.rs migration style: PRAGMA table_info presence check per column).
        Self::ensure_column(
            &conn,
            "enabled",
            "ALTER TABLE plex_settings ADD COLUMN enabled INTEGER NOT NULL DEFAULT 0;",
        )?;
        Self::ensure_column(
            &conn,
            "ui_collapsed",
            "ALTER TABLE plex_settings ADD COLUMN ui_collapsed INTEGER NOT NULL DEFAULT 0;",
        )?;
        Self::ensure_column(
            &conn,
            "metadata_write_enabled",
            "ALTER TABLE plex_settings ADD COLUMN metadata_write_enabled INTEGER NOT NULL DEFAULT 0;",
        )?;
        Self::ensure_column(
            &conn,
            "selected_section_keys",
            "ALTER TABLE plex_settings ADD COLUMN selected_section_keys TEXT NOT NULL DEFAULT '[]';",
        )?;
        Self::ensure_column(
            &conn,
            "machine_id",
            "ALTER TABLE plex_settings ADD COLUMN machine_id TEXT NOT NULL DEFAULT '';",
        )?;
        Self::ensure_column(
            &conn,
            "client_id",
            "ALTER TABLE plex_settings ADD COLUMN client_id TEXT NOT NULL DEFAULT '';",
        )?;

        conn.execute(
            "INSERT OR IGNORE INTO plex_settings
                (id, base_url, token, manual_token_mode, selected_section_key,
                 enabled, ui_collapsed, metadata_write_enabled, selected_section_keys,
                 machine_id, client_id)
             VALUES (1, '', '', 0, '', 0, 0, 0, '[]', '', '')",
            [],
        )
        .map_err(|e| format!("Failed to insert default Plex settings: {}", e))?;

        info!("[PlexSettings] Database initialized");

        Ok(Self { conn })
    }

    /// Add `alter_sql`'s column when it is not already present (idempotent).
    fn ensure_column(conn: &Connection, name: &str, alter_sql: &str) -> Result<(), String> {
        let present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('plex_settings') WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check Plex column '{}': {}", name, e))?;
        if present == 0 {
            conn.execute_batch(alter_sql)
                .map_err(|e| format!("Failed to add Plex column '{}': {}", name, e))?;
        }
        Ok(())
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "plex_settings.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "plex_settings.db")
    }

    pub fn get_settings(&self) -> Result<PlexSettings, String> {
        self.conn
            .query_row(
                "SELECT base_url, token, manual_token_mode, selected_section_key,
                        enabled, ui_collapsed, metadata_write_enabled,
                        selected_section_keys, machine_id, client_id
                 FROM plex_settings WHERE id = 1",
                [],
                |row| {
                    let base_url: String = row.get(0)?;
                    let token: String = row.get(1)?;
                    let manual_token_mode: i32 = row.get(2)?;
                    let selected_section_key: String = row.get(3)?;
                    let enabled: i32 = row.get(4)?;
                    let ui_collapsed: i32 = row.get(5)?;
                    let metadata_write_enabled: i32 = row.get(6)?;
                    let selected_section_keys_json: String = row.get(7)?;
                    let machine_id: String = row.get(8)?;
                    let client_id: String = row.get(9)?;
                    let selected_section_keys: Vec<String> =
                        serde_json::from_str(&selected_section_keys_json).unwrap_or_default();
                    Ok(PlexSettings {
                        enabled: enabled != 0,
                        ui_collapsed: ui_collapsed != 0,
                        base_url,
                        token,
                        manual_token_mode: manual_token_mode != 0,
                        metadata_write_enabled: metadata_write_enabled != 0,
                        selected_section_keys,
                        selected_section_key,
                        machine_id,
                        client_id,
                    })
                },
            )
            .map_err(|e| format!("Failed to get Plex settings: {}", e))
    }

    /// Persist base URL + token together (the common case after PIN auth or a
    /// manual-token save).
    pub fn set_credentials(&self, base_url: &str, token: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings SET base_url = ?1, token = ?2 WHERE id = 1",
                params![base_url.trim(), token.trim()],
            )
            .map_err(|e| format!("Failed to set Plex credentials: {}", e))?;
        Ok(())
    }

    pub fn set_base_url(&self, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings SET base_url = ?1 WHERE id = 1",
                params![value.trim()],
            )
            .map_err(|e| format!("Failed to set Plex base_url: {}", e))?;
        Ok(())
    }

    pub fn set_token(&self, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings SET token = ?1 WHERE id = 1",
                params![value.trim()],
            )
            .map_err(|e| format!("Failed to set Plex token: {}", e))?;
        Ok(())
    }

    pub fn set_manual_token_mode(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings SET manual_token_mode = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set Plex manual_token_mode: {}", e))?;
        Ok(())
    }

    pub fn set_enabled(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings SET enabled = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set Plex enabled: {}", e))?;
        Ok(())
    }

    pub fn set_ui_collapsed(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings SET ui_collapsed = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set Plex ui_collapsed: {}", e))?;
        Ok(())
    }

    pub fn set_metadata_write_enabled(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings SET metadata_write_enabled = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set Plex metadata_write_enabled: {}", e))?;
        Ok(())
    }

    pub fn set_machine_id(&self, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings SET machine_id = ?1 WHERE id = 1",
                params![value.trim()],
            )
            .map_err(|e| format!("Failed to set Plex machine_id: {}", e))?;
        Ok(())
    }

    /// Persist the selected section keys as a JSON array, AND mirror the first
    /// element into the legacy `selected_section_key` column in one UPDATE.
    pub fn set_selected_section_keys(&self, keys: &[String]) -> Result<(), String> {
        let json = serde_json::to_string(keys)
            .map_err(|e| format!("Failed to serialize Plex section keys: {}", e))?;
        let legacy = keys.first().cloned().unwrap_or_default();
        self.conn
            .execute(
                "UPDATE plex_settings
                 SET selected_section_keys = ?1, selected_section_key = ?2
                 WHERE id = 1",
                params![json, legacy],
            )
            .map_err(|e| format!("Failed to set Plex selected_section_keys: {}", e))?;
        Ok(())
    }

    /// Used internally by the keys setter for back-compat; kept public so a
    /// caller can still write the legacy single-key directly if needed.
    pub fn set_selected_section_key(&self, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings SET selected_section_key = ?1 WHERE id = 1",
                params![value.trim()],
            )
            .map_err(|e| format!("Failed to set Plex selected_section_key: {}", e))?;
        Ok(())
    }

    /// Returns the existing non-empty `client_id`, else generates a stable
    /// `qbz-{uuid}`, persists it, and returns it. (`ensurePlexClientId`.)
    pub fn get_or_create_client_id(&self) -> Result<String, String> {
        let existing: String = self
            .conn
            .query_row(
                "SELECT client_id FROM plex_settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read Plex client_id: {}", e))?;
        if !existing.is_empty() {
            return Ok(existing);
        }
        let generated = format!("qbz-{}", uuid::Uuid::new_v4());
        self.conn
            .execute(
                "UPDATE plex_settings SET client_id = ?1 WHERE id = 1",
                params![generated],
            )
            .map_err(|e| format!("Failed to persist Plex client_id: {}", e))?;
        Ok(generated)
    }

    /// Reset the connection state (sign out of Plex), keeping `enabled`,
    /// `client_id`, and `metadata_write_enabled`. The Plex-cache DB purge
    /// (`plex_cache_clear`) is orchestrated by the caller, not the store.
    pub fn disconnect(&self) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE plex_settings
                 SET base_url = '', token = '', manual_token_mode = 0,
                     selected_section_keys = '[]', selected_section_key = '',
                     machine_id = ''
                 WHERE id = 1",
                [],
            )
            .map_err(|e| format!("Failed to disconnect Plex: {}", e))?;
        Ok(())
    }

    /// Back-compat alias for [`disconnect`]. The single legacy `clear` is no
    /// longer one-size; new callers should use `disconnect`.
    pub fn clear(&self) -> Result<(), String> {
        self.disconnect()
    }
}

pub struct PlexSettingsState {
    pub store: Arc<Mutex<Option<PlexSettingsStore>>>,
}

impl Default for PlexSettingsState {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl PlexSettingsState {
    pub fn new() -> Result<Self, String> {
        let store = PlexSettingsStore::new()?;
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
        let new_store = PlexSettingsStore::new_at(base_dir)?;
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock Plex settings store".to_string())?;
        *guard = Some(new_store);
        Ok(())
    }

    pub fn teardown(&self) -> Result<(), String> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock Plex settings store".to_string())?;
        *guard = None;
        Ok(())
    }

    fn with_store<T>(
        &self,
        f: impl FnOnce(&PlexSettingsStore) -> Result<T, String>,
    ) -> Result<T, String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock Plex settings store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        f(store)
    }

    pub fn get_settings(&self) -> Result<PlexSettings, String> {
        self.with_store(|s| s.get_settings())
    }

    pub fn set_credentials(&self, base_url: &str, token: &str) -> Result<(), String> {
        self.with_store(|s| s.set_credentials(base_url, token))
    }

    pub fn set_base_url(&self, value: &str) -> Result<(), String> {
        self.with_store(|s| s.set_base_url(value))
    }

    pub fn set_token(&self, value: &str) -> Result<(), String> {
        self.with_store(|s| s.set_token(value))
    }

    pub fn set_manual_token_mode(&self, value: bool) -> Result<(), String> {
        self.with_store(|s| s.set_manual_token_mode(value))
    }

    pub fn set_enabled(&self, value: bool) -> Result<(), String> {
        self.with_store(|s| s.set_enabled(value))
    }

    pub fn set_ui_collapsed(&self, value: bool) -> Result<(), String> {
        self.with_store(|s| s.set_ui_collapsed(value))
    }

    pub fn set_metadata_write_enabled(&self, value: bool) -> Result<(), String> {
        self.with_store(|s| s.set_metadata_write_enabled(value))
    }

    pub fn set_machine_id(&self, value: &str) -> Result<(), String> {
        self.with_store(|s| s.set_machine_id(value))
    }

    pub fn set_selected_section_keys(&self, keys: &[String]) -> Result<(), String> {
        self.with_store(|s| s.set_selected_section_keys(keys))
    }

    pub fn set_selected_section_key(&self, value: &str) -> Result<(), String> {
        self.with_store(|s| s.set_selected_section_key(value))
    }

    pub fn get_or_create_client_id(&self) -> Result<String, String> {
        self.with_store(|s| s.get_or_create_client_id())
    }

    pub fn disconnect(&self) -> Result<(), String> {
        self.with_store(|s| s.disconnect())
    }

    pub fn clear(&self) -> Result<(), String> {
        self.with_store(|s| s.clear())
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
    fn plex_settings_default_is_unconfigured() {
        let settings = PlexSettings::default();
        assert!(!settings.enabled);
        assert!(!settings.ui_collapsed);
        assert!(settings.base_url.is_empty());
        assert!(settings.token.is_empty());
        assert!(!settings.manual_token_mode);
        assert!(!settings.metadata_write_enabled);
        assert!(settings.selected_section_keys.is_empty());
        assert!(settings.selected_section_key.is_empty());
        assert!(settings.machine_id.is_empty());
        assert!(settings.client_id.is_empty());
        assert!(!settings.is_configured());
    }

    #[test]
    fn plex_settings_store_returns_defaults() {
        let dir = unique_test_dir("plex-default");
        let store = PlexSettingsStore::new_at(&dir).expect("open store");
        let settings = store.get_settings().expect("get settings");
        assert!(!settings.is_configured());
        assert!(!settings.enabled);
        assert!(!settings.ui_collapsed);
        assert!(!settings.metadata_write_enabled);
        assert!(settings.selected_section_keys.is_empty());
        assert!(settings.machine_id.is_empty());
        assert!(settings.client_id.is_empty());
        assert!(settings.base_url.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plex_settings_persist_all_fields() {
        let dir = unique_test_dir("plex-persist");
        {
            let store = PlexSettingsStore::new_at(&dir).expect("open store");
            store
                .set_credentials("http://127.0.0.1:32400/", "  abc123  ")
                .expect("set creds");
            store.set_manual_token_mode(true).expect("set manual");
            store.set_enabled(true).expect("set enabled");
            store.set_ui_collapsed(true).expect("set collapsed");
            store
                .set_metadata_write_enabled(true)
                .expect("set metadata write");
            store.set_machine_id("  mid-123  ").expect("set machine id");
            store
                .set_selected_section_keys(&["5".to_string()])
                .expect("set section");
        }

        let reopened = PlexSettingsStore::new_at(&dir).expect("reopen store");
        let settings = reopened.get_settings().expect("get settings");

        // set_credentials trims whitespace.
        assert_eq!(settings.base_url, "http://127.0.0.1:32400/");
        assert_eq!(settings.token, "abc123");
        assert!(settings.manual_token_mode);
        assert!(settings.enabled);
        assert!(settings.ui_collapsed);
        assert!(settings.metadata_write_enabled);
        assert_eq!(settings.machine_id, "mid-123");
        assert_eq!(settings.selected_section_keys, vec!["5".to_string()]);
        assert_eq!(settings.selected_section_key, "5");
        assert!(settings.is_configured());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plex_selected_section_keys_round_trip_and_legacy_mirror() {
        let dir = unique_test_dir("plex-sections");
        let store = PlexSettingsStore::new_at(&dir).expect("open store");
        store
            .set_selected_section_keys(&["3".to_string(), "5".to_string()])
            .expect("set keys");
        let settings = store.get_settings().expect("get settings");
        assert_eq!(
            settings.selected_section_keys,
            vec!["3".to_string(), "5".to_string()]
        );
        // Legacy mirror = first element.
        assert_eq!(settings.selected_section_key, "3");

        // Empty list clears the mirror.
        store
            .set_selected_section_keys(&[])
            .expect("clear keys");
        let cleared = store.get_settings().expect("get settings");
        assert!(cleared.selected_section_keys.is_empty());
        assert!(cleared.selected_section_key.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plex_client_id_is_stable_across_calls_and_reopen() {
        let dir = unique_test_dir("plex-clientid");
        let first = {
            let store = PlexSettingsStore::new_at(&dir).expect("open store");
            let a = store.get_or_create_client_id().expect("create id");
            let b = store.get_or_create_client_id().expect("reuse id");
            assert_eq!(a, b);
            assert!(a.starts_with("qbz-"));
            a
        };
        // Across reopen.
        let reopened = PlexSettingsStore::new_at(&dir).expect("reopen store");
        let again = reopened.get_or_create_client_id().expect("reuse id");
        assert_eq!(first, again);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plex_migrates_legacy_four_column_schema_keeping_creds() {
        let dir = unique_test_dir("plex-migrate");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("plex_settings.db");
        {
            // Simulate the slice-2a 4-column table with creds present.
            let conn = Connection::open(&db_path).expect("open legacy db");
            conn.execute_batch(
                "CREATE TABLE plex_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    base_url TEXT NOT NULL DEFAULT '',
                    token TEXT NOT NULL DEFAULT '',
                    manual_token_mode INTEGER NOT NULL DEFAULT 0,
                    selected_section_key TEXT NOT NULL DEFAULT ''
                );
                INSERT INTO plex_settings (id, base_url, token, manual_token_mode, selected_section_key)
                VALUES (1, 'http://127.0.0.1:32400', 'legacy-tok', 1, '7');",
            )
            .expect("create legacy schema");
        }

        // Confirm only 5 columns before migration.
        {
            let conn = Connection::open(&db_path).expect("reopen raw");
            let cols: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('plex_settings')",
                    [],
                    |r| r.get(0),
                )
                .expect("count cols");
            assert_eq!(cols, 5);
        }

        let store = PlexSettingsStore::new_at(&dir).expect("migrate store");
        let settings = store.get_settings().expect("get settings");

        // Old creds intact.
        assert_eq!(settings.base_url, "http://127.0.0.1:32400");
        assert_eq!(settings.token, "legacy-tok");
        assert!(settings.manual_token_mode);
        assert_eq!(settings.selected_section_key, "7");
        // New columns defaulted.
        assert!(!settings.enabled);
        assert!(!settings.ui_collapsed);
        assert!(!settings.metadata_write_enabled);
        assert!(settings.selected_section_keys.is_empty());
        assert!(settings.machine_id.is_empty());
        assert!(settings.client_id.is_empty());

        // All 6 new columns added (11 total).
        {
            let conn = Connection::open(&db_path).expect("reopen raw");
            let cols: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('plex_settings')",
                    [],
                    |r| r.get(0),
                )
                .expect("count cols");
            assert_eq!(cols, 11);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plex_disconnect_keeps_enabled_client_id_and_metadata_write() {
        let dir = unique_test_dir("plex-disconnect");
        let store = PlexSettingsStore::new_at(&dir).expect("open store");
        store.set_enabled(true).expect("enable");
        store.set_metadata_write_enabled(true).expect("meta");
        let client_id = store.get_or_create_client_id().expect("client id");
        store
            .set_credentials("http://host:32400", "tok")
            .expect("set creds");
        store.set_machine_id("mid").expect("set machine id");
        store
            .set_selected_section_keys(&["3".to_string(), "4".to_string()])
            .expect("set sections");

        store.disconnect().expect("disconnect");
        let settings = store.get_settings().expect("get after disconnect");

        // Creds + sections + machine_id reset.
        assert!(settings.base_url.is_empty());
        assert!(settings.token.is_empty());
        assert!(!settings.manual_token_mode);
        assert!(settings.selected_section_keys.is_empty());
        assert!(settings.selected_section_key.is_empty());
        assert!(settings.machine_id.is_empty());
        // Preserved.
        assert!(settings.enabled);
        assert!(settings.metadata_write_enabled);
        assert_eq!(settings.client_id, client_id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plex_settings_state_requires_init() {
        let state = PlexSettingsState::new_empty();
        // No active session yet.
        assert!(state.get_settings().is_err());

        let dir = unique_test_dir("plex-state");
        state.init_at(&dir).expect("init at temp dir");
        state
            .set_credentials("http://127.0.0.1:32400", "xyz")
            .expect("set creds via state");
        let settings = state.get_settings().expect("get via state");
        assert!(settings.is_configured());
        assert_eq!(settings.token, "xyz");

        state.teardown().expect("teardown");
        assert!(state.get_settings().is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
