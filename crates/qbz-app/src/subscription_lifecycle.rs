//! Subscription lifecycle: the D4 grace clock and the offline-cache purge.
//!
//! Runs from the SHARED session activation (`AppRuntime::activate_at`) and
//! from a daily ticker the runtime owns, so every host — the Qt shell, the
//! daemon — gets the same verdict and the same enforcement without code of
//! its own. Before this module the verdict was produced by the Qt shell
//! alone, only when the login was REJECTED, and the purge had no caller at
//! all: past the 30-day grace the offline cache was merely blocked from
//! playing (`offline_playback_allowed`), never removed. The official Qobuz
//! app removes it; so does this.
//!
//! Semantics (member mode, 2026-09-02):
//!
//! - A login whose entitlements carry `offline_streaming` is a VALID
//!   observation: the clock is cleared.
//! - A login without it — a Qobuz member with no subscription — is an
//!   INVALID observation: the clock starts, or keeps running from its first
//!   observation. The member still gets in; only offline is on the clock.
//! - Once the clock passes [`GRACE_PERIOD_SECS`], the cached audio is
//!   purged — when a cache is registered. A host without one (qbzd) keeps
//!   the verdict and the read gate, and a cache registered later is purged
//!   on registration.
//! - Nothing here decides playback. `offline_playback_allowed` stays the
//!   read gate the views consult.

use std::path::Path;
use std::sync::Arc;

use qbz_models::UserSession;
use qbz_offline_cache::OfflineCacheState;

use crate::settings::subscription::SubscriptionStateStore;
pub use crate::settings::subscription::GRACE_PERIOD_SECS;

/// What the lifecycle concluded for the active session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionVerdict {
    /// The session carries the `offline_streaming` entitlement.
    pub offline_entitled: bool,
    /// No streaming entitlement at all: a Qobuz member without subscription.
    pub member_only: bool,
    /// First time the account was seen without the offline entitlement,
    /// unix seconds; `None` while the clock is clear.
    pub invalid_since: Option<i64>,
    /// When the grace window ends (`invalid_since + GRACE_PERIOD_SECS`).
    pub grace_deadline: Option<i64>,
    /// The cache was purged by THIS run.
    pub purged: bool,
}

/// D4 producer: record the login verdict for this session.
pub fn observe(
    store: &SubscriptionStateStore,
    session: &UserSession,
    now: i64,
) -> Result<(), String> {
    if session.entitlements.offline_streaming {
        store.mark_valid(now)
    } else {
        store.mark_invalid(now)
    }
}

/// D4 enforcer: purge the offline cache once the grace window has passed.
///
/// Returns `Ok(true)` when a purge happened. With no cache registered the
/// decision is logged and left standing (not marked purged), so the next
/// registration or tick purges. Takes the directory rather than an open
/// store because the store's SQLite connection cannot be held across the
/// purge's await point on a multi-threaded runtime.
pub async fn enforce(
    data_dir: &Path,
    now: i64,
    offline: Option<&OfflineCacheState>,
) -> Result<bool, String> {
    let due = SubscriptionStateStore::new_at(data_dir)?.should_purge_offline_cache(now)?;
    if !due {
        return Ok(false);
    }
    let Some(cache) = offline else {
        log::warn!(
            "[subscription] offline cache purge is due (no offline entitlement for {} days) but no cache is registered in this host",
            GRACE_PERIOD_SECS / 86_400
        );
        return Ok(false);
    };
    log::warn!(
        "[subscription] no offline entitlement for {} days: purging the offline cache",
        GRACE_PERIOD_SECS / 86_400
    );
    qbz_offline_cache::purge_all_cached_files(cache, &cache.library_db).await?;
    SubscriptionStateStore::new_at(data_dir)?.mark_offline_cache_purged(now)?;
    Ok(true)
}

/// The verdict as the host may want to show it (diagnostics, `/api/status`).
pub fn verdict(
    store: &SubscriptionStateStore,
    session: Option<&UserSession>,
    purged: bool,
) -> Result<SubscriptionVerdict, String> {
    let state = store.get_state()?;
    let entitlements = session.map(|s| &s.entitlements);
    Ok(SubscriptionVerdict {
        offline_entitled: entitlements.map(|e| e.offline_streaming).unwrap_or(false),
        member_only: entitlements.map(|e| e.is_member_only()).unwrap_or(false),
        invalid_since: state.invalid_since,
        grace_deadline: state.invalid_since.map(|t| t + GRACE_PERIOD_SECS),
        purged,
    })
}

/// One full pass for the per-user directory `data_dir`: observe the session
/// (when there is one), enforce, report. Best-effort by contract — the
/// caller logs the error and activation proceeds; a broken clock must never
/// keep a user out.
pub async fn run(
    data_dir: &Path,
    session: Option<&UserSession>,
    offline: Option<Arc<OfflineCacheState>>,
    now: i64,
) -> Result<SubscriptionVerdict, String> {
    if let Some(session) = session {
        observe(&SubscriptionStateStore::new_at(data_dir)?, session, now)?;
    }
    let purged = enforce(data_dir, now, offline.as_deref()).await?;
    verdict(&SubscriptionStateStore::new_at(data_dir)?, session, purged)
}

pub fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qbz_models::Entitlements;

    fn session(offline: bool) -> UserSession {
        UserSession {
            user_id: 7,
            entitlements: Entitlements {
                lossless_streaming: offline,
                offline_streaming: offline,
                ..Entitlements::default()
            },
            ..UserSession::default()
        }
    }

    #[test]
    fn subscriber_login_clears_the_clock() {
        let dir = tempfile::tempdir().unwrap();
        let store = SubscriptionStateStore::new_at(dir.path()).unwrap();
        store.mark_invalid(100).unwrap();
        observe(&store, &session(true), 200).unwrap();
        let v = verdict(&store, Some(&session(true)), false).unwrap();
        assert_eq!(v.invalid_since, None);
        assert_eq!(v.grace_deadline, None);
        assert!(v.offline_entitled);
        assert!(!v.member_only);
    }

    #[test]
    fn member_login_starts_the_clock_once_and_keeps_its_origin() {
        let dir = tempfile::tempdir().unwrap();
        let store = SubscriptionStateStore::new_at(dir.path()).unwrap();
        observe(&store, &session(false), 100).unwrap();
        observe(&store, &session(false), 500).unwrap();
        let v = verdict(&store, Some(&session(false)), false).unwrap();
        assert_eq!(v.invalid_since, Some(100));
        assert_eq!(v.grace_deadline, Some(100 + GRACE_PERIOD_SECS));
        assert!(v.member_only);
        assert!(!v.offline_entitled);
    }

    #[tokio::test]
    async fn purge_waits_for_the_grace_window_and_needs_a_cache() {
        let dir = tempfile::tempdir().unwrap();
        let store = SubscriptionStateStore::new_at(dir.path()).unwrap();
        observe(&store, &session(false), 100).unwrap();
        // Inside the window: nothing.
        assert!(!enforce(dir.path(), 100 + GRACE_PERIOD_SECS - 1, None)
            .await
            .unwrap());
        // Past the window but no cache registered: still nothing, and the
        // decision is NOT marked as purged, so a later host can act.
        assert!(!enforce(dir.path(), 100 + GRACE_PERIOD_SECS, None)
            .await
            .unwrap());
        assert!(store
            .should_purge_offline_cache(100 + GRACE_PERIOD_SECS)
            .unwrap());
    }

    #[tokio::test]
    async fn run_without_a_session_only_enforces() {
        let dir = tempfile::tempdir().unwrap();
        let store = SubscriptionStateStore::new_at(dir.path()).unwrap();
        store.mark_invalid(100).unwrap();
        drop(store);
        // An offline entry (no Qobuz session) must not clear or restart the
        // clock; it only reports it.
        let v = run(dir.path(), None, None, 200).await.unwrap();
        assert_eq!(v.invalid_since, Some(100));
        assert!(!v.purged);
        assert!(!v.offline_entitled);
    }
}
