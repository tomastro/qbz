//! Developer settings persistence.
//!
//! This module stores portable developer-mode toggles only. Tauri command
//! wrappers and restart messaging stay outside `qbz-app`.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeveloperSettings {
    pub force_dmabuf: bool,
}

impl Default for DeveloperSettings {
    fn default() -> Self {
        Self {
            force_dmabuf: false,
        }
    }
}

pub struct DeveloperSettingsStore {
    conn: Connection,
}

impl DeveloperSettingsStore {
    /// Lightweight read-only open for startup before host-managed state exists.
    /// Opens existing DB without creating tables or running migrations.
    pub fn new_readonly() -> Result<Self, String> {
        let db_path = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz")
            .join("developer_settings.db");
        Self::new_readonly_at_path(&db_path)
    }

    pub fn new_readonly_at_path(db_path: &Path) -> Result<Self, String> {
        if !db_path.exists() {
            return Err("Developer settings DB does not exist yet".to_string());
        }

        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| {
            format!(
                "Failed to open developer settings database (readonly): {}",
                e
            )
        })?;

        Ok(Self { conn })
    }

    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open developer settings database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| {
                format!(
                    "Failed to enable WAL for developer settings database: {}",
                    e
                )
            })?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS developer_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                force_dmabuf INTEGER NOT NULL DEFAULT 0
            );
            INSERT OR IGNORE INTO developer_settings (id, force_dmabuf) VALUES (1, 0);",
        )
        .map_err(|e| format!("Failed to create developer settings table: {}", e))?;

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "developer_settings.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "developer_settings.db")
    }

    pub fn get_settings(&self) -> Result<DeveloperSettings, String> {
        self.conn
            .query_row(
                "SELECT force_dmabuf FROM developer_settings WHERE id = 1",
                [],
                |row| {
                    Ok(DeveloperSettings {
                        force_dmabuf: row.get::<_, i64>(0)? != 0,
                    })
                },
            )
            .map_err(|e| format!("Failed to get developer settings: {}", e))
    }

    pub fn set_force_dmabuf(&self, enabled: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE developer_settings SET force_dmabuf = ?1 WHERE id = 1",
                params![enabled as i64],
            )
            .map_err(|e| format!("Failed to set force_dmabuf: {}", e))?;
        Ok(())
    }
}

/// Thread-safe wrapper for host state management.
pub struct DeveloperSettingsState {
    pub store: Arc<Mutex<Option<DeveloperSettingsStore>>>,
}

impl DeveloperSettingsState {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            store: Arc::new(Mutex::new(Some(DeveloperSettingsStore::new()?))),
        })
    }

    pub fn new_empty() -> Self {
        Self {
            store: Arc::new(Mutex::new(None)),
        }
    }
}

impl Default for DeveloperSettingsState {
    fn default() -> Self {
        Self::new_empty()
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

    fn fresh_store(name: &str) -> (std::path::PathBuf, DeveloperSettingsStore) {
        let dir = unique_test_dir(name);
        let store = DeveloperSettingsStore::new_at(&dir).expect("open store in temp dir");
        (dir, store)
    }

    #[test]
    fn developer_settings_default_values_are_stable() {
        let settings = DeveloperSettings::default();

        assert!(!settings.force_dmabuf);
    }

    #[test]
    fn developer_settings_store_returns_defaults() {
        let (dir, store) = fresh_store("developer-default");

        let settings = store.get_settings().expect("get settings");

        assert!(!settings.force_dmabuf);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn developer_settings_persist_force_dmabuf() {
        let dir = unique_test_dir("developer-force-dmabuf");
        {
            let store = DeveloperSettingsStore::new_at(&dir).expect("open store");
            store.set_force_dmabuf(true).expect("set force dmabuf");
        }

        let reopened = DeveloperSettingsStore::new_at(&dir).expect("reopen store");
        let settings = reopened.get_settings().expect("get settings");

        assert!(settings.force_dmabuf);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn developer_settings_readonly_opens_existing_db() {
        let dir = unique_test_dir("developer-readonly");
        let db_path = dir.join("developer_settings.db");
        {
            let store = DeveloperSettingsStore::new_at(&dir).expect("open store");
            store.set_force_dmabuf(true).expect("set force dmabuf");
        }

        let readonly = DeveloperSettingsStore::new_readonly_at_path(&db_path)
            .expect("open existing store read-only");
        let settings = readonly.get_settings().expect("get settings");

        assert!(settings.force_dmabuf);
        let _ = std::fs::remove_dir_all(dir);
    }
}
