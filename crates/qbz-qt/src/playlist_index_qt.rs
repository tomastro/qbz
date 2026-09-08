//! Background Qobuz playlist-membership hydrator (2.1.1 picker redesign).
//!
//! The queue itself is pure and lives in
//! `qbz_library::qobuz_playlist_snapshot::hydration_queue` — missing, stale
//! and count/revision-mismatched memberships of OWNED playlists. This driver
//! only walks it: one `playlist/get?extra=track_ids` fetch at a time, paced,
//! with a bounded per-round retry budget, committing each snapshot
//! transactionally through `replace_tracks`. A network failure never touches
//! the previous good snapshot (nothing is written on the error arm).
//!
//! `poke()` is the only entry point. The authoritative-list producers call it
//! after every recorded generation (login, sidebar reload, playlist-manager
//! load), so reconnects and mutations re-arm it for free; a poke while a walk
//! is running coalesces into one more round instead of a second walker.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use qbz_library::qobuz_playlist_snapshot as repo;

type Runtime = std::sync::Arc<qbz_app::shell::AppRuntime<qbz_core::LoggingAdapter>>;

/// Per-round fetch budget for one playlist before it is parked until the next
/// poke. Transient API errors get a second try; a persistent one must not
/// starve the rest of the queue.
const MAX_ATTEMPTS: u32 = 2;
/// Pace between fetches — the rate-limit courtesy the research doc requires.
const PACE_MS: u64 = 300;

static RUNNING: AtomicBool = AtomicBool::new(false);
static REPOKE: AtomicBool = AtomicBool::new(false);

/// Ask the hydrator to reconcile the membership snapshot with the latest
/// authoritative headers. Cheap and idempotent; safe from any thread.
pub fn poke() {
    if crate::offline_fwd::engine().is_offline() {
        return;
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        REPOKE.store(true, Ordering::SeqCst);
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        loop {
            run_round(&runtime).await;
            if !REPOKE.swap(false, Ordering::SeqCst) {
                break;
            }
        }
        RUNNING.store(false, Ordering::SeqCst);
        // A poke can race the store above: it saw RUNNING=true and only set
        // REPOKE. Pick that flag up rather than dropping the request.
        if REPOKE.swap(false, Ordering::SeqCst) {
            poke();
        }
    });
}

/// Pick the next candidate that still has retry budget and was not already
/// written this round. Pure — the driver's decisions, kept out of the loop so
/// they are testable.
///
/// The `written` set is the FORWARD-PROGRESS GUARD: a playlist successfully
/// synced this round must never be fetched again within it, even if the
/// staleness query still flags it — a queue that cannot be satisfied by a
/// successful write is a data bug to log, not a loop to run. The 2026-08-30
/// smoke hit exactly that: one playlist re-fetched every 500ms for minutes.
fn next_candidate(
    queue: &[repo::HydrationCandidate],
    failures: &HashMap<u64, u32>,
    written: &HashSet<u64>,
) -> Option<u64> {
    queue
        .iter()
        .map(|c| c.qobuz_playlist_id)
        .find(|pid| !written.contains(pid) && failures.get(pid).map_or(true, |&n| n < MAX_ATTEMPTS))
}

async fn run_round(runtime: &Runtime) {
    let mut failures: HashMap<u64, u32> = HashMap::new();
    let mut written: HashSet<u64> = HashSet::new();
    loop {
        if crate::offline_fwd::engine().is_offline() {
            return;
        }
        let queue = tokio::task::spawn_blocking(|| {
            crate::library_db_qt::with_db(false, |db| Ok(db.with_connection(repo::hydration_queue)))
                .and_then(Result::ok)
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        let Some(pid) = next_candidate(&queue, &failures, &written) else {
            if let Some(stuck) = queue
                .iter()
                .find(|c| written.contains(&c.qobuz_playlist_id))
            {
                // A successful sync that does not settle its queue entry is a
                // staleness-rule bug; surface it instead of looping on it.
                log::warn!(
                    "[qbz-qt] playlist index: {} still queued ({:?}) after a successful sync; \
                     leaving it for the next authoritative list",
                    stuck.qobuz_playlist_id,
                    stuck.reason
                );
            }
            crate::playlist_picker_qt::membership_index_progressed();
            return;
        };

        match runtime.core().get_playlist_track_ids(pid).await {
            Ok(playlist) => {
                let wrote = tokio::task::spawn_blocking(move || {
                    crate::library_db_qt::with_db(true, |db| {
                        Ok(db.with_connection(|conn| {
                            let owner =
                                Some(playlist.owner.name.as_str()).filter(|o| !o.is_empty());
                            repo::replace_tracks(
                                conn,
                                pid,
                                &playlist.name,
                                owner,
                                &playlist.track_ids,
                            )
                        }))
                    })
                })
                .await
                .ok()
                .flatten()
                .and_then(Result::ok)
                .unwrap_or(false);
                if wrote {
                    written.insert(pid);
                } else {
                    // Header vanished mid-round (deleted / retired). Spend
                    // the whole budget so the round moves on.
                    log::debug!("[qbz-qt] playlist index: {pid} lost its header; skipped");
                    failures.insert(pid, MAX_ATTEMPTS);
                }
                crate::playlist_picker_qt::membership_index_progressed();
            }
            Err(e) => {
                let n = failures.entry(pid).or_insert(0);
                *n += 1;
                log::warn!(
                    "[qbz-qt] playlist index: membership fetch for {pid} failed (attempt {n}): {e}"
                );
                // Linear backoff on top of the pace below; bounded by
                // MAX_ATTEMPTS so this cannot become a spin.
                tokio::time::sleep(std::time::Duration::from_millis(700 * *n as u64)).await;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(PACE_MS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo::{HydrationCandidate, StaleReason};

    fn candidates(ids: &[u64]) -> Vec<HydrationCandidate> {
        ids.iter()
            .map(|&id| HydrationCandidate {
                qobuz_playlist_id: id,
                reason: StaleReason::NeverSynced,
            })
            .collect()
    }

    #[test]
    fn next_candidate_skips_exhausted_budgets() {
        let queue = candidates(&[1, 2, 3]);
        let mut failures = HashMap::new();
        let written = HashSet::new();
        assert_eq!(next_candidate(&queue, &failures, &written), Some(1));

        failures.insert(1, MAX_ATTEMPTS);
        assert_eq!(next_candidate(&queue, &failures, &written), Some(2));

        failures.insert(2, MAX_ATTEMPTS - 1);
        // Budget not exhausted yet: 2 is still first in line.
        assert_eq!(next_candidate(&queue, &failures, &written), Some(2));

        failures.insert(2, MAX_ATTEMPTS);
        failures.insert(3, MAX_ATTEMPTS);
        // Everything parked: the round ends instead of spinning.
        assert_eq!(next_candidate(&queue, &failures, &written), None);
    }

    #[test]
    fn next_candidate_never_refetches_a_successful_write() {
        // The 2026-08-30 infinite loop: a queue entry that survives its own
        // successful sync must end the round, not restart it.
        let queue = candidates(&[1]);
        let failures = HashMap::new();
        let mut written = HashSet::new();
        assert_eq!(next_candidate(&queue, &failures, &written), Some(1));
        written.insert(1);
        assert_eq!(next_candidate(&queue, &failures, &written), None);
    }
}
