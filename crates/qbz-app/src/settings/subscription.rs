//! Subscription validity tracking for offline download compliance.
//!
//! Tracks when a user was first observed without a valid subscription. If the
//! invalid state persists for more than the grace period, offline downloads are
//! purged by the host-side session lifecycle.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// How long we keep honoring offline access after the server first reports an
/// invalid subscription.
///
/// Qobuz's own mobile app gives a 30-day offline grace window, so QBZ matches
/// that posture for compliance. Shorter windows would punish users on flaky
/// networks; longer windows would be more lenient than the official client.
///
/// The primary protection for offline files is the CMAF-at-rest cache format.
/// This grace period is an additional compliance guard, not the main defense.
pub const GRACE_PERIOD_SECS: i64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionState {
    pub invalid_since: Option<i64>,
    pub last_invalid_at: Option<i64>,
    pub last_valid_at: Option<i64>,
    pub last_checked_at: Option<i64>,
    pub downloads_purged_at: Option<i64>,
}

impl Default for SubscriptionState {
    fn default() -> Self {
        Self {
            invalid_since: None,
            last_invalid_at: None,
            last_valid_at: None,
            last_checked_at: None,
            downloads_purged_at: None,
        }
    }
}

pub struct SubscriptionStateStore {
    conn: Connection,
}

impl SubscriptionStateStore {
    fn open_at(dir: &Path, db_name: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let db_path = dir.join(db_name);
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open subscription state database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=1000;")
            .map_err(|e| {
                format!(
                    "Failed to enable WAL for subscription state database: {}",
                    e
                )
            })?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS subscription_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                invalid_since INTEGER,
                last_invalid_at INTEGER,
                last_valid_at INTEGER,
                last_checked_at INTEGER,
                downloads_purged_at INTEGER
            );
            INSERT OR IGNORE INTO subscription_state (id) VALUES (1);",
        )
        .map_err(|e| format!("Failed to create subscription state table: {}", e))?;

        Ok(Self { conn })
    }

    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or("Could not determine data directory")?
            .join("qbz");
        Self::open_at(&data_dir, "subscription_state.db")
    }

    pub fn new_at(base_dir: &Path) -> Result<Self, String> {
        Self::open_at(base_dir, "subscription_state.db")
    }

    pub fn get_state(&self) -> Result<SubscriptionState, String> {
        self.conn
            .query_row(
                "SELECT invalid_since, last_invalid_at, last_valid_at, last_checked_at, downloads_purged_at
                 FROM subscription_state WHERE id = 1",
                [],
                |row| {
                    Ok(SubscriptionState {
                        invalid_since: row.get(0)?,
                        last_invalid_at: row.get(1)?,
                        last_valid_at: row.get(2)?,
                        last_checked_at: row.get(3)?,
                        downloads_purged_at: row.get(4)?,
                    })
                },
            )
            .map_err(|e| format!("Failed to read subscription state: {}", e))
    }

    pub fn mark_valid(&self, now: i64) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE subscription_state
                 SET invalid_since = NULL,
                     last_valid_at = ?1,
                     last_checked_at = ?1
                 WHERE id = 1",
                params![now],
            )
            .map_err(|e| format!("Failed to update subscription state: {}", e))?;
        Ok(())
    }

    pub fn mark_invalid(&self, now: i64) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE subscription_state
                 SET invalid_since = COALESCE(invalid_since, ?1),
                     last_invalid_at = ?1,
                     last_checked_at = ?1
                 WHERE id = 1",
                params![now],
            )
            .map_err(|e| format!("Failed to update subscription state: {}", e))?;
        Ok(())
    }

    pub fn mark_offline_cache_purged(&self, now: i64) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE subscription_state SET downloads_purged_at = ?1 WHERE id = 1",
                params![now],
            )
            .map_err(|e| format!("Failed to update purge timestamp: {}", e))?;
        Ok(())
    }

    pub fn should_purge_offline_cache(&self, now: i64) -> Result<bool, String> {
        let state = self.get_state()?;
        let Some(invalid_since) = state.invalid_since else {
            return Ok(false);
        };
        if now - invalid_since < GRACE_PERIOD_SECS {
            return Ok(false);
        }
        if let Some(purged_at) = state.downloads_purged_at {
            if purged_at >= invalid_since {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// D4 playback gate: may the offline cache serve FULL tracks right now?
    ///
    /// Binary by design — within the grace window the cache plays complete
    /// tracks; past it, playback is refused outright. There is NO degraded
    /// 30-second-preview path (the owner's explicit requirement: Qobuz's
    /// preview behavior must never appear here).
    ///
    /// `invalid_since == None` (never observed invalid, including the
    /// never-checked default) ⇒ allowed: the grace clock only starts when a
    /// login verdict explicitly reports the account ineligible.
    pub fn offline_playback_allowed(&self, now: i64) -> Result<bool, String> {
        let state = self.get_state()?;
        let Some(invalid_since) = state.invalid_since else {
            return Ok(true);
        };
        Ok(now - invalid_since < GRACE_PERIOD_SECS)
    }
}

pub type SubscriptionStateState = Arc<Mutex<Option<SubscriptionStateStore>>>;

pub fn create_subscription_state() -> Result<SubscriptionStateState, String> {
    let store = SubscriptionStateStore::new()?;
    Ok(Arc::new(Mutex::new(Some(store))))
}

pub fn create_empty_subscription_state() -> SubscriptionStateState {
    Arc::new(Mutex::new(None))
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
    fn subscription_state_defaults_to_valid_access() {
        let dir = unique_test_dir("subscription-default");
        let store = SubscriptionStateStore::new_at(&dir).unwrap();

        let state = store.get_state().unwrap();

        assert!(state.invalid_since.is_none());
        assert!(!store.should_purge_offline_cache(0).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn subscription_invalid_since_preserves_first_invalid_observation() {
        let dir = unique_test_dir("subscription-invalid");
        let store = SubscriptionStateStore::new_at(&dir).unwrap();

        store.mark_invalid(100).unwrap();
        store.mark_invalid(200).unwrap();
        let state = store.get_state().unwrap();

        assert_eq!(state.invalid_since, Some(100));
        assert_eq!(state.last_invalid_at, Some(200));
        assert_eq!(state.last_checked_at, Some(200));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn subscription_purge_waits_for_grace_period_and_only_runs_once() {
        let dir = unique_test_dir("subscription-purge");
        let store = SubscriptionStateStore::new_at(&dir).unwrap();

        store.mark_invalid(100).unwrap();

        assert!(!store
            .should_purge_offline_cache(100 + GRACE_PERIOD_SECS - 1)
            .unwrap());
        assert!(store
            .should_purge_offline_cache(100 + GRACE_PERIOD_SECS)
            .unwrap());

        store
            .mark_offline_cache_purged(100 + GRACE_PERIOD_SECS)
            .unwrap();

        assert!(!store
            .should_purge_offline_cache(100 + GRACE_PERIOD_SECS + 1)
            .unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn offline_playback_allowed_within_grace_refused_after() {
        let dir = unique_test_dir("subscription-playback-gate");
        let store = SubscriptionStateStore::new_at(&dir).unwrap();

        // Never observed invalid (default): allowed.
        assert!(store.offline_playback_allowed(0).unwrap());

        store.mark_invalid(100).unwrap();
        assert!(store
            .offline_playback_allowed(100 + GRACE_PERIOD_SECS - 1)
            .unwrap());
        assert!(!store
            .offline_playback_allowed(100 + GRACE_PERIOD_SECS)
            .unwrap());

        // A valid login verdict re-opens playback.
        store.mark_valid(100 + GRACE_PERIOD_SECS + 10).unwrap();
        assert!(store
            .offline_playback_allowed(100 + GRACE_PERIOD_SECS + 20)
            .unwrap());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn subscription_mark_valid_clears_invalid_since() {
        let dir = unique_test_dir("subscription-valid");
        let store = SubscriptionStateStore::new_at(&dir).unwrap();

        store.mark_invalid(100).unwrap();
        store.mark_valid(200).unwrap();
        let state = store.get_state().unwrap();

        assert_eq!(state.invalid_since, None);
        assert_eq!(state.last_valid_at, Some(200));
        assert_eq!(state.last_checked_at, Some(200));
        assert!(!store.should_purge_offline_cache(1000).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }
}
