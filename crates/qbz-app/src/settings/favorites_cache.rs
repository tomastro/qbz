//! Local cache for favorite track / album / artist / label / award IDs.
//!
//! Hoisted from `src-tauri/src/config/favorites_cache.rs` (frontend-agnostic,
//! ADR-006) so non-Tauri frontends can read favorite status offline. The db
//! filename and schema are kept IDENTICAL to the Tauri store — both frontends
//! open the same per-user `favorites_cache.db`.
//!
//! Sync strategy (mirrors Tauri):
//! - On login: fetch all favorites from the API and replace the cache
//! - On toggle: API call first, then update the local cache on success

use rusqlite::{params, Connection};
use std::path::Path;

pub struct FavoritesCacheStore {
    conn: Connection,
}

impl FavoritesCacheStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open favorites cache database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to enable WAL for favorites cache database: {}", e))?;

        // Create tables
        conn.execute(
            "CREATE TABLE IF NOT EXISTS favorite_tracks (
                track_id INTEGER PRIMARY KEY,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .map_err(|e| format!("Failed to create favorite_tracks table: {}", e))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS favorite_albums (
                album_id TEXT PRIMARY KEY,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .map_err(|e| format!("Failed to create favorite_albums table: {}", e))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS favorite_artists (
                artist_id INTEGER PRIMARY KEY,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .map_err(|e| format!("Failed to create favorite_artists table: {}", e))?;

        // Labels — added as part of the Follow Label feature; same shape as
        // favorite_artists. CREATE IF NOT EXISTS is the migration story
        // for existing databases (no separate ALTER needed).
        conn.execute(
            "CREATE TABLE IF NOT EXISTS favorite_labels (
                label_id INTEGER PRIMARY KEY,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .map_err(|e| format!("Failed to create favorite_labels table: {}", e))?;

        // Awards — added as part of the Follow Award feature. award_id
        // is TEXT because /favorite/create?award_ids=... takes string
        // identifiers and the Android DTO declares id as String?.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS favorite_awards (
                award_id TEXT PRIMARY KEY,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .map_err(|e| format!("Failed to create favorite_awards table: {}", e))?;

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "favorites_cache.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "favorites_cache.db")
    }

    // ============ Track favorites ============

    pub fn get_favorite_track_ids(&self) -> Result<Vec<i64>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT track_id FROM favorite_tracks")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("Failed to query favorite tracks: {}", e))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| format!("Failed to read row: {}", e))?);
        }
        Ok(ids)
    }

    pub fn is_track_favorite(&self, track_id: i64) -> Result<bool, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM favorite_tracks WHERE track_id = ?1")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let exists = stmt
            .exists(params![track_id])
            .map_err(|e| format!("Failed to check favorite: {}", e))?;

        Ok(exists)
    }

    pub fn add_favorite_track(&self, track_id: i64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO favorite_tracks (track_id) VALUES (?1)",
                params![track_id],
            )
            .map_err(|e| format!("Failed to add favorite track: {}", e))?;
        Ok(())
    }

    pub fn remove_favorite_track(&self, track_id: i64) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM favorite_tracks WHERE track_id = ?1",
                params![track_id],
            )
            .map_err(|e| format!("Failed to remove favorite track: {}", e))?;
        Ok(())
    }

    pub fn sync_favorite_tracks(&self, track_ids: &[i64]) -> Result<(), String> {
        // Clear existing and insert new
        self.conn
            .execute("DELETE FROM favorite_tracks", [])
            .map_err(|e| format!("Failed to clear favorite tracks: {}", e))?;

        for &track_id in track_ids {
            self.conn
                .execute(
                    "INSERT INTO favorite_tracks (track_id) VALUES (?1)",
                    params![track_id],
                )
                .map_err(|e| format!("Failed to insert favorite track: {}", e))?;
        }
        Ok(())
    }

    // ============ Album favorites ============

    pub fn get_favorite_album_ids(&self) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT album_id FROM favorite_albums")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("Failed to query favorite albums: {}", e))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| format!("Failed to read row: {}", e))?);
        }
        Ok(ids)
    }

    pub fn is_album_favorite(&self, album_id: &str) -> Result<bool, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM favorite_albums WHERE album_id = ?1")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let exists = stmt
            .exists(params![album_id])
            .map_err(|e| format!("Failed to check favorite: {}", e))?;

        Ok(exists)
    }

    pub fn add_favorite_album(&self, album_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO favorite_albums (album_id) VALUES (?1)",
                params![album_id],
            )
            .map_err(|e| format!("Failed to add favorite album: {}", e))?;
        Ok(())
    }

    pub fn remove_favorite_album(&self, album_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM favorite_albums WHERE album_id = ?1",
                params![album_id],
            )
            .map_err(|e| format!("Failed to remove favorite album: {}", e))?;
        Ok(())
    }

    pub fn sync_favorite_albums(&self, album_ids: &[String]) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM favorite_albums", [])
            .map_err(|e| format!("Failed to clear favorite albums: {}", e))?;

        for album_id in album_ids {
            self.conn
                .execute(
                    "INSERT INTO favorite_albums (album_id) VALUES (?1)",
                    params![album_id],
                )
                .map_err(|e| format!("Failed to insert favorite album: {}", e))?;
        }
        Ok(())
    }

    // ============ Artist favorites ============

    pub fn get_favorite_artist_ids(&self) -> Result<Vec<i64>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT artist_id FROM favorite_artists")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("Failed to query favorite artists: {}", e))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| format!("Failed to read row: {}", e))?);
        }
        Ok(ids)
    }

    pub fn is_artist_favorite(&self, artist_id: i64) -> Result<bool, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM favorite_artists WHERE artist_id = ?1")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let exists = stmt
            .exists(params![artist_id])
            .map_err(|e| format!("Failed to check favorite: {}", e))?;

        Ok(exists)
    }

    pub fn add_favorite_artist(&self, artist_id: i64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO favorite_artists (artist_id) VALUES (?1)",
                params![artist_id],
            )
            .map_err(|e| format!("Failed to add favorite artist: {}", e))?;
        Ok(())
    }

    pub fn remove_favorite_artist(&self, artist_id: i64) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM favorite_artists WHERE artist_id = ?1",
                params![artist_id],
            )
            .map_err(|e| format!("Failed to remove favorite artist: {}", e))?;
        Ok(())
    }

    pub fn sync_favorite_artists(&self, artist_ids: &[i64]) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM favorite_artists", [])
            .map_err(|e| format!("Failed to clear favorite artists: {}", e))?;

        for &artist_id in artist_ids {
            self.conn
                .execute(
                    "INSERT INTO favorite_artists (artist_id) VALUES (?1)",
                    params![artist_id],
                )
                .map_err(|e| format!("Failed to insert favorite artist: {}", e))?;
        }
        Ok(())
    }

    // ============ Label favorites ============

    pub fn get_favorite_label_ids(&self) -> Result<Vec<i64>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT label_id FROM favorite_labels")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("Failed to query favorite labels: {}", e))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| format!("Failed to read row: {}", e))?);
        }
        Ok(ids)
    }

    pub fn is_label_favorite(&self, label_id: i64) -> Result<bool, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM favorite_labels WHERE label_id = ?1")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let exists = stmt
            .exists(params![label_id])
            .map_err(|e| format!("Failed to check favorite: {}", e))?;

        Ok(exists)
    }

    pub fn add_favorite_label(&self, label_id: i64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO favorite_labels (label_id) VALUES (?1)",
                params![label_id],
            )
            .map_err(|e| format!("Failed to add favorite label: {}", e))?;
        Ok(())
    }

    pub fn remove_favorite_label(&self, label_id: i64) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM favorite_labels WHERE label_id = ?1",
                params![label_id],
            )
            .map_err(|e| format!("Failed to remove favorite label: {}", e))?;
        Ok(())
    }

    pub fn sync_favorite_labels(&self, label_ids: &[i64]) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM favorite_labels", [])
            .map_err(|e| format!("Failed to clear favorite labels: {}", e))?;

        for &label_id in label_ids {
            self.conn
                .execute(
                    "INSERT INTO favorite_labels (label_id) VALUES (?1)",
                    params![label_id],
                )
                .map_err(|e| format!("Failed to insert favorite label: {}", e))?;
        }
        Ok(())
    }

    // ============ Award favorites ============

    pub fn get_favorite_award_ids(&self) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT award_id FROM favorite_awards")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query favorite awards: {}", e))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| format!("Failed to read row: {}", e))?);
        }
        Ok(ids)
    }

    pub fn is_award_favorite(&self, award_id: &str) -> Result<bool, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM favorite_awards WHERE award_id = ?1")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let exists = stmt
            .exists(params![award_id])
            .map_err(|e| format!("Failed to check favorite: {}", e))?;

        Ok(exists)
    }

    pub fn add_favorite_award(&self, award_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO favorite_awards (award_id) VALUES (?1)",
                params![award_id],
            )
            .map_err(|e| format!("Failed to add favorite award: {}", e))?;
        Ok(())
    }

    pub fn remove_favorite_award(&self, award_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM favorite_awards WHERE award_id = ?1",
                params![award_id],
            )
            .map_err(|e| format!("Failed to remove favorite award: {}", e))?;
        Ok(())
    }

    pub fn sync_favorite_awards(&self, award_ids: &[String]) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM favorite_awards", [])
            .map_err(|e| format!("Failed to clear favorite awards: {}", e))?;

        for award_id in award_ids {
            self.conn
                .execute(
                    "INSERT INTO favorite_awards (award_id) VALUES (?1)",
                    params![award_id],
                )
                .map_err(|e| format!("Failed to insert favorite award: {}", e))?;
        }
        Ok(())
    }

    // ============ Clear all (for logout) ============

    pub fn clear_all(&self) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM favorite_tracks", [])
            .map_err(|e| format!("Failed to clear favorite tracks: {}", e))?;
        self.conn
            .execute("DELETE FROM favorite_albums", [])
            .map_err(|e| format!("Failed to clear favorite albums: {}", e))?;
        self.conn
            .execute("DELETE FROM favorite_artists", [])
            .map_err(|e| format!("Failed to clear favorite artists: {}", e))?;
        self.conn
            .execute("DELETE FROM favorite_labels", [])
            .map_err(|e| format!("Failed to clear favorite labels: {}", e))?;
        self.conn
            .execute("DELETE FROM favorite_awards", [])
            .map_err(|e| format!("Failed to clear favorite awards: {}", e))?;
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
    fn favorites_cache_track_ids_roundtrip() {
        let dir = unique_test_dir("favcache-roundtrip");
        let store = FavoritesCacheStore::new_at(&dir).unwrap();

        store.add_favorite_track(1).unwrap();
        store.add_favorite_track(2).unwrap();

        let mut ids = store.get_favorite_track_ids().unwrap();
        ids.sort();
        assert_eq!(ids, vec![1, 2]);
        assert!(store.is_track_favorite(1).unwrap());

        store.remove_favorite_track(1).unwrap();
        assert!(!store.is_track_favorite(1).unwrap());
        assert!(store.is_track_favorite(2).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn favorites_cache_add_track_is_idempotent() {
        let dir = unique_test_dir("favcache-idempotent");
        let store = FavoritesCacheStore::new_at(&dir).unwrap();

        store.add_favorite_track(7).unwrap();
        store.add_favorite_track(7).unwrap();

        assert_eq!(store.get_favorite_track_ids().unwrap(), vec![7]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn favorites_cache_sync_replaces_existing_track_set() {
        let dir = unique_test_dir("favcache-sync");
        let store = FavoritesCacheStore::new_at(&dir).unwrap();

        store.add_favorite_track(1).unwrap();
        store.add_favorite_track(2).unwrap();
        store.sync_favorite_tracks(&[3, 4]).unwrap();

        let mut ids = store.get_favorite_track_ids().unwrap();
        ids.sort();
        assert_eq!(ids, vec![3, 4]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn favorites_cache_other_entities_roundtrip() {
        let dir = unique_test_dir("favcache-entities");
        let store = FavoritesCacheStore::new_at(&dir).unwrap();

        store.add_favorite_album("abc").unwrap();
        store.add_favorite_artist(11).unwrap();
        store.add_favorite_label(22).unwrap();
        store.add_favorite_award("aw1").unwrap();

        assert!(store.is_album_favorite("abc").unwrap());
        assert!(store.is_artist_favorite(11).unwrap());
        assert!(store.is_label_favorite(22).unwrap());
        assert!(store.is_award_favorite("aw1").unwrap());

        store.clear_all().unwrap();
        assert!(store.get_favorite_track_ids().unwrap().is_empty());
        assert!(store.get_favorite_album_ids().unwrap().is_empty());
        assert!(store.get_favorite_artist_ids().unwrap().is_empty());
        assert!(store.get_favorite_label_ids().unwrap().is_empty());
        assert!(store.get_favorite_award_ids().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
