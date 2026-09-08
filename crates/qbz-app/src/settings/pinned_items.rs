//! Headless pinned-items service.
//!
//! Frontend-agnostic store for the Home "Pinned" section: albums, artists and
//! playlists the user pins from card glyphs. No UI knowledge, per ADR-006 —
//! this mirrors `artist_blacklist.rs` (same pragmas, error style, in-memory
//! set seeding); the per-user lifecycle lives in the `qbz` crate wrapper.
//!
//! Provides O(1) pinned checks via an in-memory `HashSet` of `(kind, id)`
//! keys backed by SQLite persistence. Rows carry a display snapshot
//! (title/subtitle/artwork) taken at pin time so the section renders without
//! re-fetching.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::RwLock;

/// Database file name for the pinned-items store, joined onto the per-user
/// data dir by the lifecycle layer.
pub const DB_FILE_NAME: &str = "pinned_items.db";

/// A pinned entry with its display snapshot.
///
/// Ids are Strings on purpose: Qobuz album ids are alphanumeric, and artist /
/// playlist ids (numeric upstream) arrive as strings in card rows — the
/// `(kind, id)` composite TEXT key covers all three without an INTEGER axis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedItem {
    /// "album" | "artist" | "playlist".
    pub kind: String,
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub artwork_url: String,
    /// Unix seconds; the ordering key (newest first).
    pub pinned_at: i64,
}

/// Pinned-items service with O(1) lookup performance.
pub struct PinnedItemsService {
    conn: Connection,
    /// In-memory `(kind, id)` set for O(1) glyph lookups.
    pinned_keys: RwLock<HashSet<(String, String)>>,
}

impl PinnedItemsService {
    /// Create a new pinned-items service, opening or creating the database.
    pub fn new(db_path: &Path) -> Result<Self, String> {
        log::info!("[Pinned] Opening database at: {}", db_path.display());

        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open pinned items database: {}", e))?;

        // Enable WAL mode for better concurrent access (ADR-002).
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| format!("Failed to set WAL mode: {}", e))?;

        let service = Self {
            conn,
            pinned_keys: RwLock::new(HashSet::new()),
        };

        service.init_schema()?;
        service.load_from_db()?;

        Ok(service)
    }

    /// Create an in-memory pinned-items service (test/ephemeral helper).
    ///
    /// Opens a `:memory:` connection and runs schema init + load, but does not
    /// set WAL mode (not needed for an in-memory database).
    pub fn new_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory()
            .map_err(|e| format!("Failed to open in-memory pinned items database: {}", e))?;

        let service = Self {
            conn,
            pinned_keys: RwLock::new(HashSet::new()),
        };

        service.init_schema()?;
        service.load_from_db()?;

        Ok(service)
    }

    /// Initialize database schema.
    fn init_schema(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                r#"
                -- Pinned entries: (kind, id) composite key + display snapshot
                CREATE TABLE IF NOT EXISTS pinned_items (
                    kind TEXT NOT NULL CHECK (kind IN ('album','artist','playlist')),
                    id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    subtitle TEXT,
                    artwork_url TEXT,
                    pinned_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                    PRIMARY KEY (kind, id)
                );

                -- Index for the newest-first section ordering
                CREATE INDEX IF NOT EXISTS idx_pinned_items_pinned_at
                    ON pinned_items(pinned_at);
                "#,
            )
            .map_err(|e| format!("Failed to initialize pinned items schema: {}", e))?;

        Ok(())
    }

    /// Load all pinned `(kind, id)` keys from database into memory.
    fn load_from_db(&self) -> Result<(), String> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, id FROM pinned_items")
            .map_err(|e| format!("Failed to prepare pinned items query: {}", e))?;

        let keys: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("Failed to query pinned items: {}", e))?
            .filter_map(|r| r.ok())
            .collect();

        let count = keys.len();
        let mut set = self
            .pinned_keys
            .write()
            .map_err(|_| "Failed to acquire write lock")?;
        *set = keys.into_iter().collect();

        log::info!("[Pinned] Loaded {} pinned items into memory", count);
        Ok(())
    }

    /// Check if an item is pinned - O(1) operation.
    #[inline]
    pub fn is_pinned(&self, kind: &str, id: &str) -> bool {
        // O(1) HashSet lookup.
        self.pinned_keys
            .read()
            .map(|set| set.contains(&(kind.to_string(), id.to_string())))
            .unwrap_or(false)
    }

    /// Pin an item (upsert). The stored `pinned_at` is stamped now — the
    /// value carried by `item` is ignored on write.
    pub fn pin(&self, item: &PinnedItem) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        self.conn
            .execute(
                "INSERT OR REPLACE INTO pinned_items
                 (kind, id, title, subtitle, artwork_url, pinned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    item.kind,
                    item.id,
                    item.title,
                    item.subtitle,
                    item.artwork_url,
                    now
                ],
            )
            .map_err(|e| format!("Failed to pin item: {}", e))?;

        // Update in-memory set.
        if let Ok(mut set) = self.pinned_keys.write() {
            set.insert((item.kind.clone(), item.id.clone()));
        }

        log::info!(
            "[Pinned] Pinned {}: {} (id={})",
            item.kind,
            item.title,
            item.id
        );
        Ok(())
    }

    /// Unpin an item. Absent rows are Ok, not an error.
    pub fn unpin(&self, kind: &str, id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM pinned_items WHERE kind = ?1 AND id = ?2",
                params![kind, id],
            )
            .map_err(|e| format!("Failed to unpin item: {}", e))?;

        // Update in-memory set.
        if let Ok(mut set) = self.pinned_keys.write() {
            set.remove(&(kind.to_string(), id.to_string()));
        }

        log::info!("[Pinned] Unpinned {} id={}", kind, id);
        Ok(())
    }

    /// Get all pinned items, newest first.
    pub fn list(&self) -> Result<Vec<PinnedItem>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT kind, id, title, subtitle, artwork_url, pinned_at
                 FROM pinned_items
                 ORDER BY pinned_at DESC",
            )
            .map_err(|e| format!("Failed to prepare pinned items query: {}", e))?;

        let items = stmt
            .query_map([], |row| {
                Ok(PinnedItem {
                    kind: row.get(0)?,
                    id: row.get(1)?,
                    title: row.get(2)?,
                    subtitle: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    artwork_url: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    pinned_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("Failed to query pinned items: {}", e))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(items)
    }

    /// Get count of pinned items.
    pub fn count(&self) -> usize {
        self.pinned_keys.read().map(|set| set.len()).unwrap_or(0)
    }

    /// Snapshot of the in-memory `(kind, id)` set, for bulk card stamping.
    pub fn keys_snapshot(&self) -> HashSet<(String, String)> {
        self.pinned_keys
            .read()
            .map(|set| set.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, id: &str, title: &str) -> PinnedItem {
        PinnedItem {
            kind: kind.to_string(),
            id: id.to_string(),
            title: title.to_string(),
            subtitle: format!("{title} subtitle"),
            artwork_url: String::new(),
            pinned_at: 0, // ignored on write; the service stamps now
        }
    }

    /// One combined lifecycle test covering the full service surface:
    /// pin+check, kind isolation, upsert-replaces, ordered list roundtrip
    /// with NULL-tolerant fields, count/keys_snapshot, unpin (absent = Ok).
    #[test]
    fn lifecycle() {
        let s = PinnedItemsService::new_in_memory().expect("svc");

        // Fresh store: nothing pinned.
        assert!(!s.is_pinned("album", "abc123"));
        assert_eq!(s.count(), 0);
        assert!(s.keys_snapshot().is_empty());
        assert!(s.list().unwrap().is_empty());

        // Pin + check.
        s.pin(&item("album", "abc123", "First Album")).unwrap();
        assert!(s.is_pinned("album", "abc123"));
        assert!(!s.is_pinned("album", "zzz999"));

        // Kind isolation: pinning album id X does not pin playlist id X.
        assert!(!s.is_pinned("playlist", "abc123"));
        s.pin(&item("playlist", "abc123", "Same-Id Playlist"))
            .unwrap();
        assert!(s.is_pinned("playlist", "abc123"));
        assert_eq!(s.count(), 2);

        // Upsert replaces the display snapshot, keeps one row.
        s.pin(&item("album", "abc123", "Renamed Album")).unwrap();
        assert_eq!(s.count(), 2);
        let all = s.list().unwrap();
        assert_eq!(all.len(), 2);
        let renamed = all
            .iter()
            .find(|i| i.kind == "album" && i.id == "abc123")
            .expect("upserted row present");
        assert_eq!(renamed.title, "Renamed Album");
        assert_eq!(renamed.subtitle, "Renamed Album subtitle");
        assert_eq!(renamed.artwork_url, "");

        // Ordered list roundtrip: pinned_at is stamped and non-increasing
        // (newest first). Same-second pins tie, so assert the DESC property
        // rather than a strict order between them.
        s.pin(&item("artist", "42", "An Artist")).unwrap();
        let all = s.list().unwrap();
        assert_eq!(all.len(), 3);
        assert!(all.windows(2).all(|w| w[0].pinned_at >= w[1].pinned_at));
        assert!(all.iter().all(|i| i.pinned_at > 0));

        // Snapshot mirrors the set.
        let keys = s.keys_snapshot();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&("artist".to_string(), "42".to_string())));

        // Unpin: removed from check + list; absent is Ok, not error.
        s.unpin("album", "abc123").unwrap();
        assert!(!s.is_pinned("album", "abc123"));
        assert!(s.is_pinned("playlist", "abc123")); // other kind untouched
        assert_eq!(s.count(), 2);
        s.unpin("album", "nope").unwrap(); // absent -> Ok
        assert_eq!(s.list().unwrap().len(), 2);
    }
}
