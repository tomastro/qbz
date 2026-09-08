//! Image cache service
//!
//! LRU disk cache for Qobuz album/artist images.
//! - Stores images keyed by MD5 hash of URL
//! - Tracks last-access time for LRU eviction
//! - Respects a configurable max size
//! - Framework-agnostic: shared by the Tauri app and the Slint shell so
//!   both read and write the same `~/.cache/qbz/images` cache.

use md5::{Digest, Md5};
use rusqlite::{params, Connection};
use std::path::PathBuf;

/// Cache statistics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImageCacheStats {
    pub total_bytes: u64,
    pub file_count: u64,
}

pub struct ImageCacheService {
    cache_dir: PathBuf,
    conn: Connection,
}

impl ImageCacheService {
    pub fn new() -> Result<Self, String> {
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| "Could not find cache directory".to_string())?
            .join("qbz")
            .join("images");

        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create image cache dir: {}", e))?;

        let db_path = cache_dir.join("image_cache.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open image cache database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to enable WAL: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cached_images (
                hash TEXT PRIMARY KEY,
                url TEXT NOT NULL,
                file_size INTEGER NOT NULL DEFAULT 0,
                last_accessed INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_last_accessed ON cached_images (last_accessed);",
        )
        .map_err(|e| format!("Failed to create image cache table: {}", e))?;

        Ok(Self { cache_dir, conn })
    }

    fn url_hash(url: &str) -> String {
        let mut hasher = Md5::new();
        hasher.update(url.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn cache_path(&self, hash: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.img", hash))
    }

    /// Get a cached image path, updating last-access time.
    /// Returns None if not cached.
    pub fn get(&self, url: &str) -> Option<PathBuf> {
        let hash = Self::url_hash(url);
        let path = self.cache_path(&hash);

        if !path.exists() {
            // File missing — clean up stale DB entry
            let _ = self
                .conn
                .execute("DELETE FROM cached_images WHERE hash = ?1", params![hash]);
            return None;
        }

        // Update last-accessed time
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let _ = self.conn.execute(
            "UPDATE cached_images SET last_accessed = ?1 WHERE hash = ?2",
            params![now, hash],
        );

        Some(path)
    }

    /// Store image bytes in the cache.
    /// Returns the local file path on success.
    pub fn store(&self, url: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        let hash = Self::url_hash(url);
        let path = self.cache_path(&hash);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        std::fs::write(&path, bytes).map_err(|e| format!("Failed to write cached image: {}", e))?;

        let file_size = bytes.len() as i64;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO cached_images (hash, url, file_size, last_accessed)
                 VALUES (?1, ?2, ?3, ?4)",
                params![hash, url, file_size, now],
            )
            .map_err(|e| format!("Failed to insert image cache entry: {}", e))?;

        Ok(path)
    }

    /// Evict least-recently-accessed entries until total size is under max_bytes.
    pub fn evict(&self, max_bytes: u64) -> Result<u64, String> {
        let total: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(file_size), 0) FROM cached_images",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to query cache size: {}", e))?;

        if (total as u64) <= max_bytes {
            return Ok(0);
        }

        let mut to_free = (total as u64) - max_bytes;
        let mut freed: u64 = 0;

        // Get LRU entries (oldest access first)
        let mut stmt = self
            .conn
            .prepare("SELECT hash, file_size FROM cached_images ORDER BY last_accessed ASC")
            .map_err(|e| format!("Failed to prepare eviction query: {}", e))?;

        let entries: Vec<(String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("Failed to query LRU entries: {}", e))?
            .filter_map(|r| r.ok())
            .collect();

        for (hash, file_size) in entries {
            if to_free == 0 {
                break;
            }
            let path = self.cache_path(&hash);
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
            let _ = self
                .conn
                .execute("DELETE FROM cached_images WHERE hash = ?1", params![hash]);
            let size = file_size as u64;
            freed += size;
            to_free = to_free.saturating_sub(size);
        }

        Ok(freed)
    }

    /// Get cache statistics.
    pub fn stats(&self) -> Result<ImageCacheStats, String> {
        let (total_bytes, file_count): (i64, i64) = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(file_size), 0), COUNT(*) FROM cached_images",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| format!("Failed to query image cache stats: {}", e))?;

        Ok(ImageCacheStats {
            total_bytes: total_bytes as u64,
            file_count: file_count as u64,
        })
    }

    /// Clear the entire cache.
    pub fn clear(&self) -> Result<u64, String> {
        let stats = self.stats()?;

        // Delete all files
        if let Ok(entries) = std::fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "img").unwrap_or(false) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        // Clear database
        self.conn
            .execute("DELETE FROM cached_images", [])
            .map_err(|e| format!("Failed to clear image cache table: {}", e))?;

        Ok(stats.total_bytes)
    }
}
