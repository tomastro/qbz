//! Resolved-recommendation -> Qobuz-id cache (per-user SQLite, WAL per ADR-002).
//!
//! Mirrors the `MusicBrainzCache` shape. Caches BOTH positive hits (a resolved
//! Qobuz id, TTL 30d) AND negative hits (a recommendation that does not exist on
//! Qobuz, TTL 7d) so an unfindable rec does not re-hammer the Qobuz search API
//! on every render. The connection is `!Sync`; wrap it in a `Mutex` for
//! concurrent validation (locks are brief and never held across `.await`).

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// TTL for positive (found) entries — 30 days.
const FOUND_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// TTL for negative (not-on-Qobuz) entries — 7 days.
const MISS_TTL_SECS: i64 = 7 * 24 * 60 * 60;
/// Default TTL for the cached BUILT result rows — 48 hours (fast opens +
/// rotation: the tab paints instantly from cache within the window, and rebuilds
/// every 48h so the content is never "eternally the same"). The effective TTL is
/// caller-configurable (Recommendations cache-window setting); this is the
/// fallback when no preference is supplied.
pub const DEFAULT_RESULTS_TTL_SECS: i64 = 48 * 60 * 60;
/// TTL for a RESOLVED weekly playlist, keyed by its ListenBrainz playlist mbid —
/// 9 days (a week + slack). ListenBrainz regenerates Weekly Exploration / Weekly
/// Jams every Monday with a NEW mbid, so a new week is a natural cache miss while
/// the current week is served instantly. This is deliberately SEPARATE from the
/// 48h results blob: the weeklies have their own ListenBrainz cadence (a date per
/// playlist) and must not be clobbered by the unrelated 48h rotation.
const WEEKLY_TTL_SECS: i64 = 9 * 24 * 60 * 60;
/// How long a successfully-resolved weekly stays usable as a STALE FALLBACK (any
/// week, newest first) when the current build comes back empty — 21 days. Better
/// to show last week's row than an empty one on a transient ListenBrainz/Qobuz
/// hiccup.
const WEEKLY_STALE_FALLBACK_SECS: i64 = 21 * 24 * 60 * 60;

/// A cache lookup outcome.
pub enum CacheLookup {
    /// Resolved Qobuz id (track id as decimal string, or album id verbatim).
    Found(String),
    /// Previously resolved to "does not exist on Qobuz".
    Negative,
    /// Not cached (or expired) — caller must resolve live.
    Miss,
}

pub struct RecoCache {
    conn: Connection,
}

impl RecoCache {
    /// Open (or create) the cache at `<base_dir>/external_reco_cache.db`.
    pub fn open_at(base_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(base_dir)
            .map_err(|e| format!("Failed to create reco cache dir: {}", e))?;
        let conn = Connection::open(base_dir.join("external_reco_cache.db"))
            .map_err(|e| format!("Failed to open external reco cache: {}", e))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to enable WAL: {}", e))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reco_qobuz_cache (
                key TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                qobuz_id TEXT,
                found INTEGER NOT NULL,
                fetched_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_reco_qobuz_cache_fetched
                ON reco_qobuz_cache(fetched_at);
            CREATE TABLE IF NOT EXISTS reco_results (
                key TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                built_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS reco_weekly (
                key TEXT PRIMARY KEY,
                source_patch TEXT NOT NULL,
                data TEXT NOT NULL,
                built_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_reco_weekly_patch
                ON reco_weekly(source_patch, built_at);",
        )
        .map_err(|e| format!("Failed to init reco cache schema: {}", e))?;
        Ok(Self { conn })
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Look up a resolution by key, honoring the per-regime TTL.
    pub fn get(&self, key: &str) -> CacheLookup {
        let row: Option<(i64, Option<String>, i64)> = self
            .conn
            .query_row(
                "SELECT found, qobuz_id, fetched_at FROM reco_qobuz_cache WHERE key = ?",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .unwrap_or(None);

        match row {
            Some((found, qobuz_id, fetched_at)) => {
                let ttl = if found != 0 { FOUND_TTL_SECS } else { MISS_TTL_SECS };
                if Self::now() - fetched_at > ttl {
                    return CacheLookup::Miss; // expired
                }
                if found != 0 {
                    match qobuz_id {
                        Some(id) if !id.is_empty() => CacheLookup::Found(id),
                        _ => CacheLookup::Miss,
                    }
                } else {
                    CacheLookup::Negative
                }
            }
            None => CacheLookup::Miss,
        }
    }

    /// Store a resolution. `qobuz_id = None` records a negative (not-on-Qobuz).
    pub fn put(&self, key: &str, kind: &str, qobuz_id: Option<&str>) {
        let found = if qobuz_id.is_some() { 1 } else { 0 };
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO reco_qobuz_cache (key, kind, qobuz_id, found, fetched_at)
             VALUES (?, ?, ?, ?, ?)",
            params![key, kind, qobuz_id, found, Self::now()],
        );
    }

    /// Get the cached BUILT result rows (JSON of `ExternalCarousels`) for `key`,
    /// IF still within `ttl_secs`. `None` -> the caller must rebuild.
    pub fn get_results(&self, key: &str, ttl_secs: i64) -> Option<String> {
        let row: Option<(String, i64)> = self
            .conn
            .query_row(
                "SELECT data, built_at FROM reco_results WHERE key = ?",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap_or(None);
        match row {
            Some((data, built_at)) if Self::now() - built_at <= ttl_secs => Some(data),
            _ => None,
        }
    }

    /// Store the built result rows (JSON) for `key`, stamped now.
    pub fn put_results(&self, key: &str, data: &str) {
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO reco_results (key, data, built_at) VALUES (?, ?, ?)",
            params![key, data, Self::now()],
        );
    }

    /// Drop the cached BUILT result rows for `key` (force-refresh). The next
    /// build re-populates via `put_results`.
    pub fn clear_results(&self, key: &str) {
        let _ = self
            .conn
            .execute("DELETE FROM reco_results WHERE key = ?", params![key]);
    }

    /// Get a RESOLVED weekly playlist (JSON `Vec<TrackReco>`) by its exact
    /// `"{source_patch}:{playlist_mbid}"` key, IF still within the 9d TTL. The
    /// mbid changes weekly, so a fresh week misses here and triggers a rebuild;
    /// within the week the resolved set is served without re-paying Qobuz /
    /// MusicBrainz validation. `None` -> rebuild.
    pub fn get_weekly(&self, key: &str) -> Option<String> {
        let row: Option<(String, i64)> = self
            .conn
            .query_row(
                "SELECT data, built_at FROM reco_weekly WHERE key = ?",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap_or(None);
        match row {
            Some((data, built_at)) if Self::now() - built_at <= WEEKLY_TTL_SECS => Some(data),
            _ => None,
        }
    }

    /// Store a resolved weekly playlist under its `"{source_patch}:{mbid}"` key.
    /// Callers MUST only store NON-empty results so a transient empty build can
    /// never poison the row.
    pub fn put_weekly(&self, key: &str, source_patch: &str, data: &str) {
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO reco_weekly (key, source_patch, data, built_at)
             VALUES (?, ?, ?, ?)",
            params![key, source_patch, data, Self::now()],
        );
    }

    /// The most recent successfully-cached weekly for a `source_patch` (any
    /// week), within the stale-fallback window — used so a transient empty
    /// build still shows last week's row instead of nothing.
    pub fn get_latest_weekly_for_patch(&self, source_patch: &str) -> Option<String> {
        let row: Option<(String, i64)> = self
            .conn
            .query_row(
                "SELECT data, built_at FROM reco_weekly WHERE source_patch = ?
                 ORDER BY built_at DESC LIMIT 1",
                params![source_patch],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap_or(None);
        match row {
            Some((data, built_at)) if Self::now() - built_at <= WEEKLY_STALE_FALLBACK_SECS => {
                Some(data)
            }
            _ => None,
        }
    }

    /// Drop expired rows (both regimes). Safe to call opportunistically.
    pub fn cleanup_expired(&self) -> usize {
        let now = Self::now();
        let found = self
            .conn
            .execute(
                "DELETE FROM reco_qobuz_cache WHERE found = 1 AND fetched_at <= ?",
                params![now - FOUND_TTL_SECS],
            )
            .unwrap_or(0);
        let miss = self
            .conn
            .execute(
                "DELETE FROM reco_qobuz_cache WHERE found = 0 AND fetched_at <= ?",
                params![now - MISS_TTL_SECS],
            )
            .unwrap_or(0);
        let weekly = self
            .conn
            .execute(
                "DELETE FROM reco_weekly WHERE built_at <= ?",
                params![now - WEEKLY_STALE_FALLBACK_SECS],
            )
            .unwrap_or(0);
        found + miss + weekly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("qbz-extreco-{tag}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn positive_negative_and_miss() {
        let dir = tmp_dir("cache");
        let cache = RecoCache::open_at(&dir).expect("open");
        assert!(matches!(cache.get("k1"), CacheLookup::Miss));

        cache.put("k1", "track", Some("12345"));
        match cache.get("k1") {
            CacheLookup::Found(id) => assert_eq!(id, "12345"),
            _ => panic!("expected Found"),
        }

        cache.put("k2", "track", None);
        assert!(matches!(cache.get("k2"), CacheLookup::Negative));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn weekly_cache_per_week_and_stale_fallback() {
        let dir = tmp_dir("weekly");
        let cache = RecoCache::open_at(&dir).expect("open");

        // Empty until something is stored.
        assert!(cache.get_weekly("weekly-jams:mbid-A").is_none());
        assert!(cache.get_latest_weekly_for_patch("weekly-jams").is_none());

        // Store week A; exact-key hit + patch-latest both return it.
        cache.put_weekly("weekly-jams:mbid-A", "weekly-jams", "[\"A\"]");
        assert_eq!(cache.get_weekly("weekly-jams:mbid-A").as_deref(), Some("[\"A\"]"));
        assert_eq!(
            cache.get_latest_weekly_for_patch("weekly-jams").as_deref(),
            Some("[\"A\"]")
        );

        // A different week (new mbid) is a natural miss -> triggers a rebuild,
        // but the latest-for-patch fallback still serves week A.
        assert!(cache.get_weekly("weekly-jams:mbid-B").is_none());
        assert_eq!(
            cache.get_latest_weekly_for_patch("weekly-jams").as_deref(),
            Some("[\"A\"]")
        );

        // Patches are isolated from each other.
        assert!(cache.get_latest_weekly_for_patch("weekly-exploration").is_none());

        let _ = std::fs::remove_dir_all(dir);
    }
}
