//! Graphics settings persistence.
//!
//! This module stores portable host rendering preferences only. Startup
//! detection, environment variable application, crash recovery, and command
//! transport stay outside `qbz-app`.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphicsSettings {
    /// GPU rendering toggle. Read at startup as default; env var QBZ_HARDWARE_ACCEL overrides.
    pub hardware_acceleration: bool,
    /// Force X11/XWayland backend on Wayland sessions (requires restart).
    pub force_x11: bool,
    /// GDK_SCALE override for XWayland (None = auto). Integer values: "1", "2".
    pub gdk_scale: Option<String>,
    /// GDK_DPI_SCALE override for XWayland (None = auto). Float values: "0.5", "1", "1.5".
    pub gdk_dpi_scale: Option<String>,
    /// GSK_RENDERER override (None = auto). Values: "gl", "ngl", "vulkan", "cairo".
    pub gsk_renderer: Option<String>,
    /// Rendering GPU selection.
    ///
    /// Valid values are "auto", "integrated", "discrete", "software", or a
    /// host-specific GPU id such as a PCI slot.
    pub preferred_gpu: String,
    /// Opt-in NVIDIA Wayland compatibility mode. The host applies the runtime
    /// environment changes before graphics initialization.
    pub nvidia_compat_mode: bool,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            hardware_acceleration: true,
            force_x11: false,
            gdk_scale: None,
            gdk_dpi_scale: None,
            gsk_renderer: None,
            preferred_gpu: "auto".to_string(),
            nvidia_compat_mode: false,
        }
    }
}

pub struct GraphicsSettingsStore {
    conn: Connection,
}

impl GraphicsSettingsStore {
    /// Lightweight read-only open for startup before host-managed state exists.
    /// Opens existing DB without creating tables or running migrations.
    pub fn new_readonly() -> Result<Self, String> {
        let db_path = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz")
            .join("graphics_settings.db");
        Self::new_readonly_at_path(&db_path)
    }

    pub fn new_readonly_at_path(db_path: &Path) -> Result<Self, String> {
        if !db_path.exists() {
            return Err("Graphics settings DB does not exist yet".to_string());
        }

        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| {
            format!(
                "Failed to open graphics settings database (readonly): {}",
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
            .map_err(|e| format!("Failed to open graphics settings database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to enable WAL for graphics settings database: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS graphics_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                hardware_acceleration INTEGER NOT NULL DEFAULT 1
            );
            INSERT OR IGNORE INTO graphics_settings (id, hardware_acceleration) VALUES (1, 1);",
        )
        .map_err(|e| format!("Failed to create graphics settings table: {}", e))?;

        let _ = conn.execute_batch(
            "ALTER TABLE graphics_settings ADD COLUMN force_x11 INTEGER NOT NULL DEFAULT 0;",
        );
        let _ = conn.execute_batch("ALTER TABLE graphics_settings ADD COLUMN gdk_scale TEXT;");
        let _ = conn.execute_batch("ALTER TABLE graphics_settings ADD COLUMN gdk_dpi_scale TEXT;");
        let _ = conn.execute_batch("ALTER TABLE graphics_settings ADD COLUMN gsk_renderer TEXT;");
        let _ = conn.execute_batch(
            "ALTER TABLE graphics_settings ADD COLUMN preferred_gpu TEXT NOT NULL DEFAULT 'auto';",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE graphics_settings ADD COLUMN nvidia_compat_mode INTEGER NOT NULL DEFAULT 0;",
        );

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "graphics_settings.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "graphics_settings.db")
    }

    pub fn get_settings(&self) -> Result<GraphicsSettings, String> {
        self.conn
            .query_row(
                "SELECT hardware_acceleration, force_x11, gdk_scale, gdk_dpi_scale, gsk_renderer, preferred_gpu, nvidia_compat_mode FROM graphics_settings WHERE id = 1",
                [],
                |row| {
                    Ok(GraphicsSettings {
                        hardware_acceleration: row.get::<_, i64>(0)? != 0,
                        force_x11: row.get::<_, i64>(1)? != 0,
                        gdk_scale: row.get::<_, Option<String>>(2)?,
                        gdk_dpi_scale: row.get::<_, Option<String>>(3)?,
                        gsk_renderer: row.get::<_, Option<String>>(4)?,
                        preferred_gpu: row
                            .get::<_, Option<String>>(5)?
                            .unwrap_or_else(|| "auto".to_string()),
                        nvidia_compat_mode: row.get::<_, i64>(6).unwrap_or(0) != 0,
                    })
                },
            )
            .map_err(|e| format!("Failed to get graphics settings: {}", e))
    }

    pub fn set_hardware_acceleration(&self, enabled: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE graphics_settings SET hardware_acceleration = ?1 WHERE id = 1",
                params![enabled as i64],
            )
            .map_err(|e| format!("Failed to set hardware_acceleration: {}", e))?;
        Ok(())
    }

    pub fn set_force_x11(&self, enabled: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE graphics_settings SET force_x11 = ?1 WHERE id = 1",
                params![enabled as i64],
            )
            .map_err(|e| format!("Failed to set force_x11: {}", e))?;
        Ok(())
    }

    pub fn set_gdk_scale(&self, value: Option<String>) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE graphics_settings SET gdk_scale = ?1 WHERE id = 1",
                params![value],
            )
            .map_err(|e| format!("Failed to set gdk_scale: {}", e))?;
        Ok(())
    }

    pub fn set_gdk_dpi_scale(&self, value: Option<String>) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE graphics_settings SET gdk_dpi_scale = ?1 WHERE id = 1",
                params![value],
            )
            .map_err(|e| format!("Failed to set gdk_dpi_scale: {}", e))?;
        Ok(())
    }

    pub fn set_gsk_renderer(&self, value: Option<String>) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE graphics_settings SET gsk_renderer = ?1 WHERE id = 1",
                params![value],
            )
            .map_err(|e| format!("Failed to set gsk_renderer: {}", e))?;
        Ok(())
    }

    pub fn set_preferred_gpu(&self, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE graphics_settings SET preferred_gpu = ?1 WHERE id = 1",
                params![value],
            )
            .map_err(|e| format!("Failed to set preferred_gpu: {}", e))?;
        Ok(())
    }

    pub fn set_nvidia_compat_mode(&self, enabled: bool) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE graphics_settings SET nvidia_compat_mode = ?1 WHERE id = 1",
                params![enabled as i64],
            )
            .map_err(|e| format!("Failed to set nvidia_compat_mode: {}", e))?;
        Ok(())
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
    fn graphics_settings_default_values_are_stable() {
        let settings = GraphicsSettings::default();

        assert!(settings.hardware_acceleration);
        assert!(!settings.force_x11);
        assert_eq!(settings.gdk_scale, None);
        assert_eq!(settings.gdk_dpi_scale, None);
        assert_eq!(settings.gsk_renderer, None);
        assert_eq!(settings.preferred_gpu, "auto");
        assert!(!settings.nvidia_compat_mode);
    }

    #[test]
    fn graphics_settings_store_returns_defaults() {
        let dir = unique_test_dir("graphics-default");
        let store = GraphicsSettingsStore::new_at(&dir).expect("open store");

        let settings = store.get_settings().expect("get settings");

        assert!(settings.hardware_acceleration);
        assert!(!settings.force_x11);
        assert_eq!(settings.preferred_gpu, "auto");
        assert!(!settings.nvidia_compat_mode);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn graphics_settings_persist_all_fields() {
        let dir = unique_test_dir("graphics-persist");
        {
            let store = GraphicsSettingsStore::new_at(&dir).expect("open store");
            store
                .set_hardware_acceleration(false)
                .expect("set hardware acceleration");
            store.set_force_x11(true).expect("set force x11");
            store
                .set_gdk_scale(Some("2".to_string()))
                .expect("set gdk scale");
            store
                .set_gdk_dpi_scale(Some("0.5".to_string()))
                .expect("set gdk dpi scale");
            store
                .set_gsk_renderer(Some("ngl".to_string()))
                .expect("set gsk renderer");
            store
                .set_preferred_gpu("discrete")
                .expect("set preferred gpu");
            store
                .set_nvidia_compat_mode(true)
                .expect("set nvidia compat mode");
        }

        let reopened = GraphicsSettingsStore::new_at(&dir).expect("reopen store");
        let settings = reopened.get_settings().expect("get settings");

        assert!(!settings.hardware_acceleration);
        assert!(settings.force_x11);
        assert_eq!(settings.gdk_scale.as_deref(), Some("2"));
        assert_eq!(settings.gdk_dpi_scale.as_deref(), Some("0.5"));
        assert_eq!(settings.gsk_renderer.as_deref(), Some("ngl"));
        assert_eq!(settings.preferred_gpu, "discrete");
        assert!(settings.nvidia_compat_mode);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn graphics_settings_reopen_does_not_overwrite_existing_row() {
        let dir = unique_test_dir("graphics-no-overwrite");
        {
            let store = GraphicsSettingsStore::new_at(&dir).expect("open store");
            store
                .set_hardware_acceleration(false)
                .expect("set hardware acceleration");
            store
                .set_preferred_gpu("software")
                .expect("set preferred gpu");
        }

        let reopened = GraphicsSettingsStore::new_at(&dir).expect("reopen store");
        let settings = reopened.get_settings().expect("get settings");

        assert!(!settings.hardware_acceleration);
        assert_eq!(settings.preferred_gpu, "software");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn graphics_settings_readonly_opens_existing_db() {
        let dir = unique_test_dir("graphics-readonly");
        let db_path = dir.join("graphics_settings.db");
        {
            let store = GraphicsSettingsStore::new_at(&dir).expect("open store");
            store
                .set_preferred_gpu("integrated")
                .expect("set preferred gpu");
        }

        let readonly =
            GraphicsSettingsStore::new_readonly_at_path(&db_path).expect("open readonly store");
        let settings = readonly.get_settings().expect("get readonly settings");

        assert_eq!(settings.preferred_gpu, "integrated");
        let _ = std::fs::remove_dir_all(dir);
    }
}
