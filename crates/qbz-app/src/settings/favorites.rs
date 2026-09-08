use chrono::Utc;
use md5::{Digest, Md5};
use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavoritesPreferences {
    pub custom_icon_path: Option<String>,
    pub custom_icon_preset: Option<String>,
    pub icon_background: Option<String>,
    pub tab_order: Vec<String>,
}

impl Default for FavoritesPreferences {
    fn default() -> Self {
        Self {
            custom_icon_path: None,
            custom_icon_preset: Some("heart".to_string()),
            icon_background: None,
            tab_order: vec![
                "tracks".to_string(),
                "albums".to_string(),
                "artists".to_string(),
                "playlists".to_string(),
            ],
        }
    }
}

pub struct FavoritesPreferencesStore {
    conn: Connection,
}

impl FavoritesPreferencesStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open favorites preferences database: {}", e))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to set WAL mode on favorites preferences: {}", e))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS favorites_preferences (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                custom_icon_path TEXT,
                custom_icon_preset TEXT,
                tab_order TEXT NOT NULL
            )",
            [],
        )
        .map_err(|e| format!("Failed to create favorites preferences table: {}", e))?;

        // Migration: Add icon_background column if it doesn't exist
        let has_icon_background = conn
            .prepare("SELECT icon_background FROM favorites_preferences LIMIT 1")
            .is_ok();

        if !has_icon_background {
            conn.execute(
                "ALTER TABLE favorites_preferences ADD COLUMN icon_background TEXT",
                [],
            )
            .map_err(|e| format!("Failed to add icon_background column: {}", e))?;
        }

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "favorites_preferences.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "favorites_preferences.db")
    }

    fn favorites_icon_dir() -> Result<PathBuf, String> {
        let cache_dir = dirs::cache_dir()
            .ok_or("Could not determine cache directory")?
            .join("qbz")
            .join("favorites");

        fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create favorites icon directory: {}", e))?;

        Ok(cache_dir)
    }

    fn normalize_custom_icon_path(&self, path: String) -> Result<String, String> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Ok(String::new());
        }

        let source = Path::new(trimmed);
        if !source.exists() {
            return Err(format!("Source icon does not exist: {}", trimmed));
        }

        let icon_dir = Self::favorites_icon_dir()?;
        if source.starts_with(&icon_dir) {
            return Ok(trimmed.to_string());
        }

        let extension = source.extension().and_then(|e| e.to_str()).unwrap_or("png");

        let mut hasher = Md5::new();
        hasher.update(trimmed.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        let filename = format!(
            "favorites_icon_{}_{}.{}",
            hash,
            Utc::now().timestamp(),
            extension
        );
        let dest_path = icon_dir.join(filename);

        fs::copy(source, &dest_path)
            .map_err(|e| format!("Failed to copy favorites icon: {}", e))?;

        Ok(dest_path.to_string_lossy().to_string())
    }

    pub fn get_preferences(&self) -> Result<FavoritesPreferences, String> {
        let mut stmt = self.conn.prepare("SELECT custom_icon_path, custom_icon_preset, icon_background, tab_order FROM favorites_preferences WHERE id = 1")
            .map_err(|e| format!("Failed to prepare select: {}", e))?;

        let result = stmt.query_row([], |row| {
            let custom_icon_path: Option<String> = row.get(0)?;
            let custom_icon_preset: Option<String> = row.get(1)?;
            let icon_background: Option<String> = row.get(2)?;
            let tab_order_str: String = row.get(3)?;

            let custom_icon_path = custom_icon_path.and_then(|value| {
                if value.trim().is_empty() {
                    None
                } else {
                    Some(value)
                }
            });

            let tab_order: Vec<String> =
                serde_json::from_str(&tab_order_str).unwrap_or_else(|_| {
                    vec![
                        "tracks".to_string(),
                        "albums".to_string(),
                        "artists".to_string(),
                        "playlists".to_string(),
                    ]
                });

            Ok(FavoritesPreferences {
                custom_icon_path,
                custom_icon_preset,
                icon_background,
                tab_order,
            })
        });

        match result {
            Ok(mut prefs) => {
                if let Some(path) = prefs.custom_icon_path.clone() {
                    match self.normalize_custom_icon_path(path) {
                        Ok(resolved) => {
                            let normalized = if resolved.trim().is_empty() {
                                None
                            } else {
                                Some(resolved)
                            };
                            if normalized != prefs.custom_icon_path {
                                prefs.custom_icon_path = normalized;
                                let _ = self.save_preferences(prefs.clone());
                            }
                        }
                        Err(_) => {
                            prefs.custom_icon_path = None;
                            let _ = self.save_preferences(prefs.clone());
                        }
                    }
                }
                Ok(prefs)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(FavoritesPreferences::default()),
            Err(e) => Err(format!("Failed to query preferences: {}", e)),
        }
    }

    pub fn save_preferences(
        &self,
        mut prefs: FavoritesPreferences,
    ) -> Result<FavoritesPreferences, String> {
        if let Some(path) = prefs.custom_icon_path.clone() {
            let resolved = self.normalize_custom_icon_path(path)?;
            if resolved.is_empty() {
                prefs.custom_icon_path = None;
            } else {
                prefs.custom_icon_path = Some(resolved);
            }
        }

        let tab_order_str = serde_json::to_string(&prefs.tab_order)
            .map_err(|e| format!("Failed to serialize tab_order: {}", e))?;

        self.conn.execute(
            "INSERT OR REPLACE INTO favorites_preferences (id, custom_icon_path, custom_icon_preset, icon_background, tab_order)
             VALUES (1, ?1, ?2, ?3, ?4)",
            params![prefs.custom_icon_path, prefs.custom_icon_preset, prefs.icon_background, tab_order_str],
        )
        .map_err(|e| format!("Failed to save preferences: {}", e))?;
        Ok(prefs)
    }
}

pub struct FavoritesPreferencesState {
    pub store: Arc<Mutex<Option<FavoritesPreferencesStore>>>,
}

impl FavoritesPreferencesState {
    pub fn new() -> Result<Self, String> {
        let store = FavoritesPreferencesStore::new()?;
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
        let new_store = FavoritesPreferencesStore::new_at(base_dir)?;
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock favorites preferences store".to_string())?;
        *guard = Some(new_store);
        Ok(())
    }

    pub fn teardown(&self) -> Result<(), String> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Failed to lock favorites preferences store".to_string())?;
        *guard = None;
        Ok(())
    }
}

pub fn create_table(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS favorites_preferences (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            custom_icon_path TEXT,
            custom_icon_preset TEXT,
            tab_order TEXT NOT NULL
        )",
        [],
    )?;

    // Migration: Add icon_background column if it doesn't exist
    let has_icon_background = conn
        .prepare("SELECT icon_background FROM favorites_preferences LIMIT 1")
        .is_ok();

    if !has_icon_background {
        conn.execute(
            "ALTER TABLE favorites_preferences ADD COLUMN icon_background TEXT",
            [],
        )?;
    }

    Ok(())
}

pub fn load_preferences(conn: &Connection) -> Result<FavoritesPreferences> {
    let mut stmt = conn.prepare("SELECT custom_icon_path, custom_icon_preset, icon_background, tab_order FROM favorites_preferences WHERE id = 1")?;

    let result = stmt.query_row([], |row| {
        let custom_icon_path: Option<String> = row.get(0)?;
        let custom_icon_preset: Option<String> = row.get(1)?;
        let icon_background: Option<String> = row.get(2)?;
        let tab_order_str: String = row.get(3)?;

        let custom_icon_path = custom_icon_path.and_then(|value| {
            if value.trim().is_empty() {
                None
            } else {
                Some(value)
            }
        });

        let tab_order: Vec<String> = serde_json::from_str(&tab_order_str).unwrap_or_else(|_| {
            vec![
                "tracks".to_string(),
                "albums".to_string(),
                "artists".to_string(),
                "playlists".to_string(),
            ]
        });

        Ok(FavoritesPreferences {
            custom_icon_path,
            custom_icon_preset,
            icon_background,
            tab_order,
        })
    });

    match result {
        Ok(prefs) => Ok(prefs),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(FavoritesPreferences::default()),
        Err(e) => Err(e),
    }
}

pub fn save_preferences(conn: &Connection, prefs: &FavoritesPreferences) -> Result<()> {
    let tab_order_str = serde_json::to_string(&prefs.tab_order).unwrap();

    conn.execute(
        "INSERT OR REPLACE INTO favorites_preferences (id, custom_icon_path, custom_icon_preset, icon_background, tab_order)
         VALUES (1, ?1, ?2, ?3, ?4)",
        params![prefs.custom_icon_path, prefs.custom_icon_preset, prefs.icon_background, tab_order_str],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_dir(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("qbz-app-{name}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn favorites_preferences_default_tab_order_is_stable() {
        let prefs = FavoritesPreferences::default();

        assert_eq!(
            prefs.tab_order,
            vec![
                "tracks".to_string(),
                "albums".to_string(),
                "artists".to_string(),
                "playlists".to_string()
            ]
        );
        assert_eq!(prefs.custom_icon_preset, Some("heart".to_string()));
    }

    #[test]
    fn favorites_preferences_store_returns_default_when_empty() {
        let dir = unique_test_dir("favorites-default");
        let store = FavoritesPreferencesStore::new_at(&dir).unwrap();

        let prefs = store.get_preferences().unwrap();

        assert_eq!(prefs.tab_order, FavoritesPreferences::default().tab_order);
        assert_eq!(
            prefs.custom_icon_preset,
            FavoritesPreferences::default().custom_icon_preset
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn favorites_preferences_persist_without_custom_icon() {
        let dir = unique_test_dir("favorites-persist");
        let store = FavoritesPreferencesStore::new_at(&dir).unwrap();
        let prefs = FavoritesPreferences {
            custom_icon_path: None,
            custom_icon_preset: Some("star".to_string()),
            icon_background: Some("#112233".to_string()),
            tab_order: vec!["artists".to_string(), "tracks".to_string()],
        };

        let saved = store.save_preferences(prefs.clone()).unwrap();
        let loaded = store.get_preferences().unwrap();

        assert_eq!(saved.custom_icon_preset, prefs.custom_icon_preset);
        assert_eq!(loaded.custom_icon_preset, prefs.custom_icon_preset);
        assert_eq!(loaded.icon_background, prefs.icon_background);
        assert_eq!(loaded.tab_order, prefs.tab_order);
        let _ = std::fs::remove_dir_all(dir);
    }
}
