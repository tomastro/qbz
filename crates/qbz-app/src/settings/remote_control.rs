//! Remote control settings and allowed-origin persistence.
//!
//! This module owns portable storage only. Server startup, TLS certificate
//! management, QR generation, router/CORS application, and live restarts remain
//! host-owned behavior.

use base64::Engine;
use rand::RngExt;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

const DEFAULT_ALLOWED_ORIGINS: &[&str] = &[
    "vicrodh.github.io",
    "control.qbz.lol",
    "www.control.qbz.lol",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteControlSettings {
    pub enabled: bool,
    pub port: u16,
    pub secure: bool,
    pub token: String,
}

impl Default for RemoteControlSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 8182,
            secure: true,
            token: String::new(),
        }
    }
}

pub struct RemoteControlSettingsStore {
    conn: Connection,
}

impl RemoteControlSettingsStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open remote control settings database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| {
                format!(
                    "Failed to enable WAL for remote control settings database: {}",
                    e
                )
            })?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS remote_control_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                enabled INTEGER NOT NULL DEFAULT 0,
                port INTEGER NOT NULL DEFAULT 8182,
                secure INTEGER NOT NULL DEFAULT 1,
                token TEXT NOT NULL DEFAULT ''
            );",
        )
        .map_err(|e| format!("Failed to create remote control settings table: {}", e))?;

        ensure_secure_column(&conn)?;

        let token = generate_token();
        conn.execute(
            "INSERT OR IGNORE INTO remote_control_settings (id, enabled, port, secure, token)
            VALUES (1, 0, 8182, 0, ?1)",
            params![token],
        )
        .map_err(|e| format!("Failed to insert default remote control settings: {}", e))?;

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "remote_control_settings.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "remote_control_settings.db")
    }

    pub fn get_settings(&self) -> Result<RemoteControlSettings, String> {
        let mut settings = self
            .conn
            .query_row(
                "SELECT enabled, port, secure, token FROM remote_control_settings WHERE id = 1",
                [],
                |row| {
                    let enabled: i32 = row.get(0)?;
                    let port: i64 = row.get(1)?;
                    let secure: i32 = row.get(2)?;
                    let token: String = row.get(3)?;
                    Ok(RemoteControlSettings {
                        enabled: enabled != 0,
                        port: port as u16,
                        secure: secure != 0,
                        token,
                    })
                },
            )
            .map_err(|e| format!("Failed to get remote control settings: {}", e))?;

        if settings.token.is_empty() {
            settings.token = generate_token();
            self.set_token(&settings.token)?;
        }

        Ok(settings)
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE remote_control_settings SET enabled = ?1 WHERE id = 1",
                params![if enabled { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set remote control enabled: {}", e))?;
        Ok(())
    }

    pub fn set_port(&self, port: u16) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE remote_control_settings SET port = ?1 WHERE id = 1",
                params![port as i64],
            )
            .map_err(|e| format!("Failed to set remote control port: {}", e))?;
        Ok(())
    }

    pub fn set_secure(&self, secure: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE remote_control_settings SET secure = ?1 WHERE id = 1",
                params![if secure { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set remote control secure: {}", e))?;
        Ok(())
    }

    pub fn set_token(&self, token: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE remote_control_settings SET token = ?1 WHERE id = 1",
                params![token],
            )
            .map_err(|e| format!("Failed to set remote control token: {}", e))?;
        Ok(())
    }

    pub fn regenerate_token(&self) -> Result<String, String> {
        let token = generate_token();
        self.set_token(&token)?;
        Ok(token)
    }
}

pub struct RemoteControlSettingsState {
    pub store: Arc<Mutex<Option<RemoteControlSettingsStore>>>,
}

impl RemoteControlSettingsState {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            store: Arc::new(Mutex::new(Some(RemoteControlSettingsStore::new()?))),
        })
    }

    pub fn new_empty() -> Self {
        Self {
            store: Arc::new(Mutex::new(None)),
        }
    }

    pub fn init_at(&self, base_dir: &Path) -> Result<(), String> {
        let new_store = RemoteControlSettingsStore::new_at(base_dir)?;
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock remote control settings store".to_string())?;
        *guard = Some(new_store);
        Ok(())
    }

    pub fn teardown(&self) -> Result<(), String> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock remote control settings store".to_string())?;
        *guard = None;
        Ok(())
    }

    pub fn get_settings(&self) -> Result<RemoteControlSettings, String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.get_settings()
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_enabled(enabled)
    }

    pub fn set_port(&self, port: u16) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_port(port)
    }

    pub fn set_secure(&self, secure: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_secure(secure)
    }

    pub fn regenerate_token(&self) -> Result<String, String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.regenerate_token()
    }
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn ensure_secure_column(conn: &Connection) -> Result<(), String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(remote_control_settings)")
        .map_err(|e| format!("Failed to read settings schema: {}", e))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| format!("Failed to read settings schema: {}", e))?;

    while let Some(row) = rows
        .next()
        .map_err(|e| format!("Schema read error: {}", e))?
    {
        let name: String = row
            .get(1)
            .map_err(|e| format!("Schema read error: {}", e))?;
        if name == "secure" {
            return Ok(());
        }
    }

    conn.execute(
        "ALTER TABLE remote_control_settings ADD COLUMN secure INTEGER NOT NULL DEFAULT 0",
        [],
    )
    .map_err(|e| format!("Failed to migrate remote control settings: {}", e))?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowedOrigin {
    pub id: i64,
    pub origin: String,
    pub is_default: bool,
}

pub struct AllowedOriginsStore {
    conn: Connection,
}

impl AllowedOriginsStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open allowed origins database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to enable WAL for allowed origins database: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS allowed_origins (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                origin TEXT NOT NULL UNIQUE,
                is_default INTEGER NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| format!("Failed to create allowed_origins table: {}", e))?;

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM allowed_origins", [], |row| row.get(0))
            .unwrap_or(0);

        if count == 0 {
            for origin in DEFAULT_ALLOWED_ORIGINS {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO allowed_origins (origin, is_default) VALUES (?1, 1)",
                    params![origin],
                );
            }
        }

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "remote_control_settings.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "remote_control_settings.db")
    }

    pub fn get_origins(&self) -> Result<Vec<AllowedOrigin>, String> {
        let mut stmt = self.conn
            .prepare("SELECT id, origin, is_default FROM allowed_origins ORDER BY is_default DESC, origin ASC")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let origins = stmt
            .query_map([], |row| {
                Ok(AllowedOrigin {
                    id: row.get(0)?,
                    origin: row.get(1)?,
                    is_default: row.get::<_, i32>(2)? != 0,
                })
            })
            .map_err(|e| format!("Failed to query origins: {}", e))?;

        origins
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect origins: {}", e))
    }

    pub fn is_origin_allowed(&self, origin: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM allowed_origins WHERE origin = ?1",
                params![origin],
                |_| Ok(()),
            )
            .is_ok()
    }

    pub fn add_origin(&self, origin: &str) -> Result<AllowedOrigin, String> {
        let normalized = origin.trim().to_lowercase();

        if normalized.is_empty() {
            return Err("Origin cannot be empty".to_string());
        }

        self.conn
            .execute(
                "INSERT INTO allowed_origins (origin, is_default) VALUES (?1, 0)",
                params![normalized],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE constraint") {
                    "Origin already exists".to_string()
                } else {
                    format!("Failed to add origin: {}", e)
                }
            })?;

        let id = self.conn.last_insert_rowid();
        Ok(AllowedOrigin {
            id,
            origin: normalized,
            is_default: false,
        })
    }

    pub fn remove_origin(&self, id: i64) -> Result<(), String> {
        let affected = self
            .conn
            .execute("DELETE FROM allowed_origins WHERE id = ?1", params![id])
            .map_err(|e| format!("Failed to remove origin: {}", e))?;

        if affected == 0 {
            return Err("Origin not found".to_string());
        }
        Ok(())
    }

    pub fn restore_defaults(&self) -> Result<(), String> {
        for origin in DEFAULT_ALLOWED_ORIGINS {
            let _ = self.conn.execute(
                "INSERT OR IGNORE INTO allowed_origins (origin, is_default) VALUES (?1, 1)",
                params![origin],
            );
        }
        Ok(())
    }
}

pub struct AllowedOriginsState {
    pub store: Arc<Mutex<Option<AllowedOriginsStore>>>,
}

impl AllowedOriginsState {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            store: Arc::new(Mutex::new(Some(AllowedOriginsStore::new()?))),
        })
    }

    pub fn new_empty() -> Self {
        Self {
            store: Arc::new(Mutex::new(None)),
        }
    }

    pub fn init_at(&self, base_dir: &Path) -> Result<(), String> {
        let new_store = AllowedOriginsStore::new_at(base_dir)?;
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock allowed origins store".to_string())?;
        *guard = Some(new_store);
        Ok(())
    }

    pub fn teardown(&self) -> Result<(), String> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock allowed origins store".to_string())?;
        *guard = None;
        Ok(())
    }

    pub fn get_origins(&self) -> Result<Vec<AllowedOrigin>, String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.get_origins()
    }

    pub fn is_origin_allowed(&self, origin: &str) -> bool {
        self.store
            .lock()
            .map(|guard| {
                guard
                    .as_ref()
                    .map(|store| store.is_origin_allowed(origin))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    pub fn add_origin(&self, origin: &str) -> Result<AllowedOrigin, String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.add_origin(origin)
    }

    pub fn remove_origin(&self, id: i64) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.remove_origin(id)
    }

    pub fn restore_defaults(&self) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.restore_defaults()
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

    fn fresh_remote_store(name: &str) -> (std::path::PathBuf, RemoteControlSettingsStore) {
        let dir = unique_test_dir(name);
        let store = RemoteControlSettingsStore::new_at(&dir).expect("open store in temp dir");
        (dir, store)
    }

    fn fresh_origins_store(name: &str) -> (std::path::PathBuf, AllowedOriginsStore) {
        let dir = unique_test_dir(name);
        let store = AllowedOriginsStore::new_at(&dir).expect("open origins store in temp dir");
        (dir, store)
    }

    #[test]
    fn remote_control_default_struct_values_are_stable() {
        let defaults = RemoteControlSettings::default();

        assert!(!defaults.enabled);
        assert_eq!(defaults.port, 8182);
        assert!(defaults.secure);
        assert!(defaults.token.is_empty());
    }

    #[test]
    fn remote_control_store_creates_token_and_defaults_internal_remote_to_http() {
        let (dir, store) = fresh_remote_store("remote-default");

        let settings = store.get_settings().expect("get settings");

        assert!(!settings.enabled);
        assert_eq!(settings.port, 8182);
        assert!(!settings.secure);
        assert!(!settings.token.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remote_control_settings_persist_across_reopen() {
        let dir = unique_test_dir("remote-persist");
        let token;
        {
            let store = RemoteControlSettingsStore::new_at(&dir).expect("open store");
            token = store.get_settings().expect("get settings").token;
            store.set_enabled(true).expect("set enabled");
            store.set_port(9310).expect("set port");
            store.set_secure(true).expect("set secure");
        }

        let reopened = RemoteControlSettingsStore::new_at(&dir).expect("reopen store");
        let settings = reopened.get_settings().expect("get settings");

        assert!(settings.enabled);
        assert_eq!(settings.port, 9310);
        assert!(settings.secure);
        assert_eq!(settings.token, token);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remote_control_regenerate_token_changes_and_persists_token() {
        let dir = unique_test_dir("remote-token");
        let original;
        let regenerated;
        {
            let store = RemoteControlSettingsStore::new_at(&dir).expect("open store");
            original = store.get_settings().expect("get settings").token;
            regenerated = store.regenerate_token().expect("regenerate token");
        }

        let reopened = RemoteControlSettingsStore::new_at(&dir).expect("reopen store");
        let settings = reopened.get_settings().expect("get settings");

        assert_ne!(original, regenerated);
        assert_eq!(settings.token, regenerated);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remote_control_empty_token_is_repaired_on_read() {
        let dir = unique_test_dir("remote-empty-token");
        let db_path = dir.join("remote_control_settings.db");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        {
            let conn = Connection::open(&db_path).expect("open db");
            conn.execute_batch(
                "CREATE TABLE remote_control_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    enabled INTEGER NOT NULL DEFAULT 0,
                    port INTEGER NOT NULL DEFAULT 8182,
                    secure INTEGER NOT NULL DEFAULT 1,
                    token TEXT NOT NULL DEFAULT ''
                );
                INSERT INTO remote_control_settings (id, enabled, port, secure, token)
                VALUES (1, 0, 8182, 0, '');",
            )
            .expect("create schema");
        }

        let store = RemoteControlSettingsStore::new_at(&dir).expect("open store");
        let settings = store.get_settings().expect("get settings");

        assert!(!settings.token.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remote_control_legacy_schema_adds_secure_default_false() {
        let dir = unique_test_dir("remote-legacy");
        let db_path = dir.join("remote_control_settings.db");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        {
            let conn = Connection::open(&db_path).expect("open legacy db");
            conn.execute_batch(
                "CREATE TABLE remote_control_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    enabled INTEGER NOT NULL DEFAULT 0,
                    port INTEGER NOT NULL DEFAULT 8182,
                    token TEXT NOT NULL DEFAULT ''
                );
                INSERT INTO remote_control_settings (id, enabled, port, token)
                VALUES (1, 1, 8183, 'legacy-token');",
            )
            .expect("create legacy schema");
        }

        let store = RemoteControlSettingsStore::new_at(&dir).expect("migrate store");
        let settings = store.get_settings().expect("get settings");

        assert!(settings.enabled);
        assert_eq!(settings.port, 8183);
        assert!(!settings.secure);
        assert_eq!(settings.token, "legacy-token");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn allowed_origins_store_creates_defaults() {
        let (dir, store) = fresh_origins_store("origins-default");

        let origins = store.get_origins().expect("get origins");
        let names = origins
            .iter()
            .map(|origin| origin.origin.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "control.qbz.lol",
                "vicrodh.github.io",
                "www.control.qbz.lol"
            ]
        );
        assert!(origins.iter().all(|origin| origin.is_default));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn allowed_origins_add_normalizes_and_remove_works() {
        let (dir, store) = fresh_origins_store("origins-add-remove");

        let origin = store
            .add_origin("  EXAMPLE.com  ")
            .expect("add custom origin");
        assert_eq!(origin.origin, "example.com");
        assert!(!origin.is_default);
        assert!(store.is_origin_allowed("example.com"));

        store.remove_origin(origin.id).expect("remove origin");
        assert!(!store.is_origin_allowed("example.com"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn allowed_origins_rejects_empty_and_duplicate_origins() {
        let (dir, store) = fresh_origins_store("origins-invalid");

        assert!(store.add_origin("   ").is_err());
        store.add_origin("example.com").expect("add origin");
        let duplicate = store.add_origin("example.com").expect_err("duplicate");

        assert_eq!(duplicate, "Origin already exists");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn allowed_origins_restore_defaults_adds_missing_defaults() {
        let (dir, store) = fresh_origins_store("origins-restore");
        let origin = store.add_origin("example.com").expect("add origin");
        let default_id = store
            .get_origins()
            .expect("get origins")
            .into_iter()
            .find(|entry| entry.origin == "control.qbz.lol")
            .expect("find default")
            .id;
        store.remove_origin(default_id).expect("remove default");

        store.restore_defaults().expect("restore defaults");
        let origins = store.get_origins().expect("get origins");

        assert!(origins.iter().any(|entry| entry.origin == "control.qbz.lol"));
        assert!(origins.iter().any(|entry| entry.id == origin.id));
        let _ = std::fs::remove_dir_all(dir);
    }
}
