//! Background maintenance hints for the incremental local scanner (phase G).
//!
//! Native filesystem events debounce into a root-scoped scan for local disks.
//! They are never the source of truth: every root also receives a periodic
//! reconciliation, and network roots use only that bounded schedule.

use std::collections::BTreeMap;
use std::sync::Once;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use qbz_library::{LibraryDatabase, LibraryFolder, LocalRootWatcher, RootWatchEvent};

const WATCH_POLL: Duration = Duration::from_secs(1);
const WATCH_DEBOUNCE: Duration = Duration::from_secs(2);
const ROOT_REFRESH: Duration = Duration::from_secs(60);
const LOCAL_RECONCILE: Duration = Duration::from_secs(24 * 60 * 60);
const NETWORK_RECONCILE: Duration = Duration::from_secs(6 * 60 * 60);
const BUSY_RETRY: Duration = Duration::from_secs(15);

pub(crate) fn start() {
    static START: Once = Once::new();
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("qbz-local-scan-maintenance".to_string())
            .spawn(run);
    });
}

fn run() {
    let Ok(db) = LibraryDatabase::open(&qbz_library::get_db_path()) else {
        log::warn!("[local-scan] maintenance disabled: library database unavailable");
        return;
    };
    let mut folders = db.get_folders_with_metadata().unwrap_or_default();
    let mut watcher = LocalRootWatcher::new(&folders).ok();
    let mut pending = BTreeMap::<i64, Instant>::new();
    queue_periodic_due(&folders, unix_now(), Instant::now(), &mut pending);
    let mut refreshed = Instant::now();

    loop {
        let event = watcher
            .as_ref()
            .map(|watcher| watcher.recv_timeout(WATCH_POLL))
            .unwrap_or_else(|| {
                std::thread::sleep(WATCH_POLL);
                RootWatchEvent::Timeout
            });
        let now = Instant::now();
        match event {
            RootWatchEvent::Changed(root_ids) => {
                for root_id in root_ids {
                    pending.insert(root_id, now + WATCH_DEBOUNCE);
                }
            }
            RootWatchEvent::Error(root_ids) => {
                // A watcher error is only loss of an accelerator. Queue the
                // affected roots and let the generation scan decide safety.
                for root_id in root_ids {
                    pending.insert(root_id, now + WATCH_DEBOUNCE);
                }
            }
            RootWatchEvent::Disconnected => {
                watcher = None;
            }
            RootWatchEvent::Timeout => {}
        }

        if now.duration_since(refreshed) >= ROOT_REFRESH {
            folders = db.get_folders_with_metadata().unwrap_or_default();
            match watcher.as_mut() {
                Some(active_watcher) => {
                    if active_watcher.rebuild(&folders).is_err() {
                        watcher = None;
                    }
                }
                None => watcher = LocalRootWatcher::new(&folders).ok(),
            }
            queue_periodic_due(&folders, unix_now(), now, &mut pending);
            refreshed = now;
        }

        let ready = pending
            .iter()
            .find(|(_, deadline)| **deadline <= now)
            .map(|(root_id, _)| *root_id);
        let Some(root_id) = ready else {
            continue;
        };
        if crate::settings_qt::library::scan(Some(root_id)) {
            pending.remove(&root_id);
        } else {
            pending.insert(root_id, now + BUSY_RETRY);
        }
    }
}

fn queue_periodic_due(
    folders: &[LibraryFolder],
    now_secs: i64,
    now: Instant,
    pending: &mut BTreeMap<i64, Instant>,
) {
    for folder in folders.iter().filter(|folder| folder.enabled) {
        if reconciliation_due(folder, now_secs) {
            pending.entry(folder.id).or_insert(now);
        }
    }
}

fn reconciliation_due(folder: &LibraryFolder, now_secs: i64) -> bool {
    let interval = if folder.is_network {
        NETWORK_RECONCILE
    } else {
        LOCAL_RECONCILE
    };
    let last = folder.last_scan.unwrap_or(0);
    last <= 0 || now_secs.saturating_sub(last) >= interval.as_secs().min(i64::MAX as u64) as i64
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(network: bool, last_scan: Option<i64>) -> LibraryFolder {
        LibraryFolder {
            id: 1,
            path: "/fixture/music".to_string(),
            alias: None,
            enabled: true,
            is_network: network,
            network_fs_type: network.then(|| "nfs".to_string()),
            user_override_network: network,
            last_scan,
        }
    }

    #[test]
    fn local_watch_roots_still_reconcile_daily() {
        let now = 2_000_000;
        assert!(reconciliation_due(&folder(false, None), now));
        assert!(!reconciliation_due(
            &folder(false, Some(now - LOCAL_RECONCILE.as_secs() as i64 + 1)),
            now
        ));
        assert!(reconciliation_due(
            &folder(false, Some(now - LOCAL_RECONCILE.as_secs() as i64)),
            now
        ));
    }

    #[test]
    fn network_roots_use_the_shorter_periodic_schedule() {
        let now = 2_000_000;
        assert!(reconciliation_due(
            &folder(true, Some(now - NETWORK_RECONCILE.as_secs() as i64)),
            now
        ));
        assert!(!reconciliation_due(
            &folder(true, Some(now - NETWORK_RECONCILE.as_secs() as i64 + 1)),
            now
        ));
    }
}
