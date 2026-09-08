//! Portable tray preference storage.
//!
//! This module owns persisted tray preferences only. Runtime tray creation,
//! icon updates, window hiding/showing, and emitted events remain in the
//! host application layer.

use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraySettings {
    /// Show system tray icon (requires restart to take effect).
    pub enable_tray: bool,
    /// Hide window to tray when clicking minimize.
    pub minimize_to_tray: bool,
    /// Hide window to tray instead of quitting when clicking close.
    /// Opt-out: enabled by default (closing sends the window to the tray and
    /// keeps the app running, like Spotify/Discord).
    pub close_to_tray: bool,
    /// Tray icon variant override:
    /// - "auto" follows system color scheme,
    /// - "mono-light" uses a light glyph for dark panels,
    /// - "mono-dark" uses a dark glyph for light panels,
    /// - "color" uses the full-color vinyl logo.
    #[serde(default = "default_tray_icon_theme")]
    pub tray_icon_theme: String,
    /// macOS only: when closed to the menu bar, switch the activation policy
    /// to `.accessory` (no Dock icon, menu-bar-only). Off keeps the Dock icon
    /// (Spotify-style). Ignored on Linux/Windows.
    #[serde(default)]
    pub mac_hide_dock: bool,
}

fn default_tray_icon_theme() -> String {
    "auto".to_string()
}

/// Coerce free-form values to the supported set. Anything outside the
/// supported list falls back to "auto".
///
/// Legacy 1.2.9-pre values "light"/"dark" had inverted semantics
/// relative to the labels users saw. They are remapped here to the value
/// that matches the user's original intent.
pub fn normalize_tray_icon_theme(input: &str) -> String {
    match input {
        "mono-light" | "mono-dark" | "color" | "auto" => input.to_string(),
        "light" => "mono-light".to_string(),
        "dark" => "mono-dark".to_string(),
        _ => "auto".to_string(),
    }
}

impl Default for TraySettings {
    fn default() -> Self {
        Self {
            enable_tray: true,
            minimize_to_tray: false,
            close_to_tray: true,
            tray_icon_theme: default_tray_icon_theme(),
            mac_hide_dock: false,
        }
    }
}

pub struct TraySettingsStore {
    conn: Connection,
}

impl TraySettingsStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open tray settings database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to enable WAL for tray settings database: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tray_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                enable_tray INTEGER NOT NULL DEFAULT 1,
                minimize_to_tray INTEGER NOT NULL DEFAULT 0,
                close_to_tray INTEGER NOT NULL DEFAULT 1
            );",
        )
        .map_err(|e| format!("Failed to create tray settings table: {}", e))?;

        let has_theme_column: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tray_settings') WHERE name = 'tray_icon_theme'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check tray_icon_theme column: {}", e))?;
        if has_theme_column == 0 {
            conn.execute_batch(
                "ALTER TABLE tray_settings ADD COLUMN tray_icon_theme TEXT NOT NULL DEFAULT 'auto';",
            )
            .map_err(|e| format!("Failed to add tray_icon_theme column: {}", e))?;
        }

        let has_mac_hide_dock_column: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tray_settings') WHERE name = 'mac_hide_dock'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check mac_hide_dock column: {}", e))?;
        if has_mac_hide_dock_column == 0 {
            conn.execute_batch(
                "ALTER TABLE tray_settings ADD COLUMN mac_hide_dock INTEGER NOT NULL DEFAULT 0;",
            )
            .map_err(|e| format!("Failed to add mac_hide_dock column: {}", e))?;
        }

        conn.execute(
            "INSERT OR IGNORE INTO tray_settings (id, enable_tray, minimize_to_tray, close_to_tray, tray_icon_theme, mac_hide_dock)
            VALUES (1, 1, 0, 1, 'auto', 0)",
            [],
        )
        .map_err(|e| format!("Failed to insert default tray settings: {}", e))?;

        info!("[TraySettings] Database initialized");

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "tray_settings.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "tray_settings.db")
    }

    pub fn get_settings(&self) -> Result<TraySettings, String> {
        self.conn
            .query_row(
                "SELECT enable_tray, minimize_to_tray, close_to_tray, tray_icon_theme, mac_hide_dock FROM tray_settings WHERE id = 1",
                [],
                |row| {
                    let enable_tray: i32 = row.get(0)?;
                    let minimize_to_tray: i32 = row.get(1)?;
                    let close_to_tray: i32 = row.get(2)?;
                    let tray_icon_theme: String = row.get(3)?;
                    let mac_hide_dock: i32 = row.get(4)?;
                    Ok(TraySettings {
                        enable_tray: enable_tray != 0,
                        minimize_to_tray: minimize_to_tray != 0,
                        close_to_tray: close_to_tray != 0,
                        tray_icon_theme: normalize_tray_icon_theme(&tray_icon_theme),
                        mac_hide_dock: mac_hide_dock != 0,
                    })
                },
            )
            .map_err(|e| format!("Failed to get tray settings: {}", e))
    }

    pub fn set_enable_tray(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE tray_settings SET enable_tray = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set enable_tray: {}", e))?;
        Ok(())
    }

    pub fn set_minimize_to_tray(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE tray_settings SET minimize_to_tray = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set minimize_to_tray: {}", e))?;
        Ok(())
    }

    pub fn set_close_to_tray(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE tray_settings SET close_to_tray = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set close_to_tray: {}", e))?;
        Ok(())
    }

    pub fn set_tray_icon_theme(&self, value: &str) -> Result<(), String> {
        let normalized = normalize_tray_icon_theme(value);
        self.conn
            .execute(
                "UPDATE tray_settings SET tray_icon_theme = ?1 WHERE id = 1",
                params![normalized],
            )
            .map_err(|e| format!("Failed to set tray_icon_theme: {}", e))?;
        Ok(())
    }

    pub fn set_mac_hide_dock(&self, value: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE tray_settings SET mac_hide_dock = ?1 WHERE id = 1",
                params![if value { 1 } else { 0 }],
            )
            .map_err(|e| format!("Failed to set mac_hide_dock: {}", e))?;
        Ok(())
    }
}

pub struct TraySettingsState {
    pub store: Arc<Mutex<Option<TraySettingsStore>>>,
}

impl TraySettingsState {
    pub fn new() -> Result<Self, String> {
        let store = TraySettingsStore::new()?;
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
        let new_store = TraySettingsStore::new_at(base_dir)?;
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock tray settings store".to_string())?;
        *guard = Some(new_store);
        Ok(())
    }

    pub fn teardown(&self) -> Result<(), String> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock tray settings store".to_string())?;
        *guard = None;
        Ok(())
    }

    pub fn get_settings(&self) -> Result<TraySettings, String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock tray settings store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.get_settings()
    }

    pub fn set_enable_tray(&self, value: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock tray settings store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_enable_tray(value)
    }

    pub fn set_minimize_to_tray(&self, value: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock tray settings store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_minimize_to_tray(value)
    }

    pub fn set_close_to_tray(&self, value: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock tray settings store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_close_to_tray(value)
    }

    pub fn set_tray_icon_theme(&self, value: &str) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock tray settings store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_tray_icon_theme(value)
    }

    pub fn set_mac_hide_dock(&self, value: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock tray settings store".to_string())?;
        let store = guard.as_ref().ok_or("No active session - please log in")?;
        store.set_mac_hide_dock(value)
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

    fn fresh_store(name: &str) -> (std::path::PathBuf, TraySettingsStore) {
        let dir = unique_test_dir(name);
        let store = TraySettingsStore::new_at(&dir).expect("open store in temp dir");
        (dir, store)
    }

    #[test]
    fn tray_settings_default_values_are_stable() {
        let settings = TraySettings::default();

        assert!(settings.enable_tray);
        assert!(!settings.minimize_to_tray);
        assert!(settings.close_to_tray);
        assert_eq!(settings.tray_icon_theme, "auto");
    }

    #[test]
    fn tray_settings_store_returns_defaults() {
        let (dir, store) = fresh_store("tray-default");

        let settings = store.get_settings().expect("get settings");

        assert!(settings.enable_tray);
        assert!(!settings.minimize_to_tray);
        assert!(settings.close_to_tray);
        assert_eq!(settings.tray_icon_theme, "auto");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tray_settings_persist_all_fields() {
        let dir = unique_test_dir("tray-persist");
        {
            let store = TraySettingsStore::new_at(&dir).expect("open store");
            store.set_enable_tray(false).expect("set enable");
            store.set_minimize_to_tray(true).expect("set minimize");
            store.set_close_to_tray(true).expect("set close");
            store
                .set_tray_icon_theme("mono-light")
                .expect("set icon theme");
        }

        let reopened = TraySettingsStore::new_at(&dir).expect("reopen store");
        let settings = reopened.get_settings().expect("get settings");

        assert!(!settings.enable_tray);
        assert!(settings.minimize_to_tray);
        assert!(settings.close_to_tray);
        assert_eq!(settings.tray_icon_theme, "mono-light");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tray_icon_theme_normalizes_supported_legacy_and_unknown_values() {
        assert_eq!(normalize_tray_icon_theme("auto"), "auto");
        assert_eq!(normalize_tray_icon_theme("mono-light"), "mono-light");
        assert_eq!(normalize_tray_icon_theme("mono-dark"), "mono-dark");
        assert_eq!(normalize_tray_icon_theme("color"), "color");
        assert_eq!(normalize_tray_icon_theme("light"), "mono-light");
        assert_eq!(normalize_tray_icon_theme("dark"), "mono-dark");
        assert_eq!(normalize_tray_icon_theme("invalid"), "auto");
    }

    #[test]
    fn tray_settings_migrates_legacy_schema() {
        let dir = unique_test_dir("tray-migrate");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("tray_settings.db");
        {
            let conn = Connection::open(&db_path).expect("open legacy db");
            conn.execute_batch(
                "CREATE TABLE tray_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    enable_tray INTEGER NOT NULL DEFAULT 1,
                    minimize_to_tray INTEGER NOT NULL DEFAULT 0,
                    close_to_tray INTEGER NOT NULL DEFAULT 0
                );
                INSERT INTO tray_settings (id, enable_tray, minimize_to_tray, close_to_tray)
                VALUES (1, 0, 1, 1);",
            )
            .expect("create legacy schema");
        }

        let store = TraySettingsStore::new_at(&dir).expect("migrate store");
        let settings = store.get_settings().expect("get settings");

        assert!(!settings.enable_tray);
        assert!(settings.minimize_to_tray);
        assert!(settings.close_to_tray);
        assert_eq!(settings.tray_icon_theme, "auto");
        let _ = std::fs::remove_dir_all(dir);
    }
}
