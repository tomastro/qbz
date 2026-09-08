//! Best-effort local-root watcher.
//!
//! Watch notifications are only acceleration hints. The scheduler always
//! performs periodic generation scans as the source of truth, and network
//! roots are deliberately excluded because recursive watches on NAS mounts are
//! not reliable enough to authorize deletion.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{LibraryError, LibraryFolder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootWatchEvent {
    Changed(Vec<i64>),
    Error(Vec<i64>),
    Disconnected,
    Timeout,
}

pub struct LocalRootWatcher {
    watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    roots: BTreeMap<i64, PathBuf>,
}

impl LocalRootWatcher {
    pub fn new(folders: &[LibraryFolder]) -> Result<Self, LibraryError> {
        let (sender, receiver) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })
        .map_err(|error| LibraryError::Other(format!("local root watcher: {error}")))?;
        let mut result = Self {
            watcher,
            receiver,
            roots: BTreeMap::new(),
        };
        result.rebuild(folders)?;
        Ok(result)
    }

    pub fn rebuild(&mut self, folders: &[LibraryFolder]) -> Result<(), LibraryError> {
        let desired = folders
            .iter()
            .filter(|folder| folder.enabled && !folder.is_network)
            .map(|folder| (folder.id, PathBuf::from(&folder.path)))
            .collect::<BTreeMap<_, _>>();

        for (root_id, path) in self.roots.clone() {
            if desired.get(&root_id) == Some(&path) {
                continue;
            }
            let _ = self.watcher.unwatch(&path);
            self.roots.remove(&root_id);
        }
        let mut watch_failures = 0_usize;
        for (root_id, path) in &desired {
            if self.roots.get(root_id) == Some(path) {
                continue;
            }
            // A missing/newly-unmounted root remains covered by periodic
            // reconciliation. Failing to install its hint is not fatal.
            if self.watcher.watch(path, RecursiveMode::Recursive).is_ok() {
                self.roots.insert(*root_id, path.clone());
            } else {
                watch_failures += 1;
            }
        }
        if watch_failures > 0 {
            log::warn!(
                "[local-scan] watcher unavailable for {watch_failures} local root(s); periodic reconciliation remains active"
            );
        }
        Ok(())
    }

    pub fn recv_timeout(&self, timeout: Duration) -> RootWatchEvent {
        match self.receiver.recv_timeout(timeout) {
            Ok(Ok(event)) => RootWatchEvent::Changed(self.roots_for_paths(&event.paths)),
            Ok(Err(error)) => RootWatchEvent::Error(self.roots_for_paths(&error.paths)),
            Err(RecvTimeoutError::Timeout) => RootWatchEvent::Timeout,
            Err(RecvTimeoutError::Disconnected) => RootWatchEvent::Disconnected,
        }
    }

    fn roots_for_paths(&self, paths: &[PathBuf]) -> Vec<i64> {
        let mut roots = BTreeSet::new();
        for path in paths {
            for (root_id, root) in &self.roots {
                if path.starts_with(root) {
                    roots.insert(*root_id);
                }
            }
        }
        roots.into_iter().collect()
    }

    #[cfg(test)]
    fn watched_roots(&self) -> usize {
        self.roots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: i64, path: &std::path::Path, network: bool) -> LibraryFolder {
        LibraryFolder {
            id,
            path: path.to_string_lossy().into_owned(),
            alias: None,
            enabled: true,
            is_network: network,
            network_fs_type: network.then(|| "nfs".to_string()),
            user_override_network: network,
            last_scan: None,
        }
    }

    #[test]
    fn only_local_roots_are_watched_and_nested_path_queues_every_owner() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("local");
        let nested = local.join("nested");
        let network = temp.path().join("network");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&network).unwrap();
        let watcher = LocalRootWatcher::new(&[
            folder(1, &local, false),
            folder(2, &nested, false),
            folder(3, &network, true),
        ])
        .unwrap();
        assert_eq!(watcher.watched_roots(), 2);
        assert_eq!(
            watcher.roots_for_paths(&[nested.join("track.flac")]),
            vec![1, 2]
        );
    }
}
