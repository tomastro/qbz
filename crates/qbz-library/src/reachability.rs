//! Reachability probes that cannot hang the caller.
//!
//! # Why this exists
//!
//! A network share that is MOUNTED BUT UNREACHABLE — the user is on a
//! different network today, the NAS is off, the VPN dropped — does not make
//! filesystem calls fail. It makes them **block**, for however long the mount
//! was configured to wait (an NFS hard mount waits forever). `Path::exists()`,
//! `read_dir()` and even `canonicalize()` are all syscalls, and all of them sit
//! there.
//!
//! That matters because the codebase already tries to defend against exactly
//! this case and cannot, since every guard probes with a bare blocking call:
//!
//!   * `scan.rs` decides a folder is unreachable with `read_dir(..).is_err()` —
//!     on a hung mount that never returns an error, it never returns at all;
//!   * `mount_info::is_network_path()` canonicalizes the path it is classifying,
//!     so the network detector hangs on the very thing it exists to detect;
//!   * the local playback path awaits an `exists()` with no deadline, so
//!     playing one dead file never resolves.
//!
//! # The two rules this module encodes
//!
//! **1. Deadline the PROBE, never the transfer.** Reading a hi-res FLAC over a
//! working-but-slow share legitimately takes many seconds; a blanket timeout
//! around the whole operation turns "your network is slow" into "this track
//! does not exist", which is worse than the bug. Only the question *does this
//! answer at all?* gets a deadline.
//!
//! **2. Probe a MOUNT once, not a file each time.** A blocking syscall cannot
//! be cancelled: when the deadline expires the helper thread is still stuck in
//! the kernel, and it stays stuck until the mount gives up. One leaked thread
//! per dead mount per cool-down is the price of asking at all. One per TRACK
//! would be a thread bomb on a 2000-row queue, which is how a defence turns
//! into the outage it was meant to prevent.

use std::collections::HashMap;
use std::fs::Metadata;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a mount stays marked unreachable before anyone probes it again.
/// Long enough that a dead NAS is asked about rarely; short enough that
/// reconnecting to the right network recovers without a restart.
const COOLDOWN: Duration = Duration::from_secs(30);

/// The default probe deadline. The owner's spec: a couple of seconds, then
/// stop waiting and treat it as gone.
pub const DEFAULT_PROBE: Duration = Duration::from_secs(3);

/// What a probe learned about a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// The filesystem answered and the file is there.
    Present,
    /// The filesystem answered and the file is not there. This is a REAL
    /// answer — the caller may delete/clean on it.
    Missing,
    /// The filesystem did not answer inside the deadline, or answered with an
    /// I/O error. NOT the same as missing: the file may be perfectly fine on a
    /// share we cannot see from this network, so the caller should skip it,
    /// never clean it.
    Unreachable,
}

/// A bounded metadata probe for callers that need to validate a regular file
/// without doing a second, potentially blocking filesystem syscall.
#[derive(Debug)]
pub enum FileProbe {
    Present(Metadata),
    Missing,
    Unreachable,
}

impl Reach {
    /// Can playback proceed?
    pub fn is_playable(self) -> bool {
        self == Reach::Present
    }
}

fn unreachable_until() -> &'static Mutex<HashMap<PathBuf, Instant>> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, Instant>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The key a path is remembered under: its first two components, which is a
/// good enough stand-in for "the mount" without calling into the filesystem to
/// find the real mount point (that lookup is itself a syscall on the path we
/// are trying not to touch).
fn mount_key(path: &Path) -> PathBuf {
    path.components().take(3).collect()
}

/// True when this path's mount answered too slowly recently and its cool-down
/// has not elapsed. Costs no syscall — that is the whole point.
pub fn is_cooling_down(path: &Path) -> bool {
    let key = mount_key(path);
    let mut map = match unreachable_until().lock() {
        Ok(m) => m,
        Err(_) => return false,
    };
    match map.get(&key) {
        Some(until) if Instant::now() < *until => true,
        Some(_) => {
            map.remove(&key);
            false
        }
        None => false,
    }
}

fn mark_unreachable(path: &Path) {
    if let Ok(mut map) = unreachable_until().lock() {
        map.insert(mount_key(path), Instant::now() + COOLDOWN);
    }
}

/// Forget every cool-down. Call when connectivity changes — the user joined
/// the right network and should not wait out a timer for it.
pub fn reset_cooldowns() {
    if let Ok(mut map) = unreachable_until().lock() {
        map.clear();
    }
}

/// Does `path` exist, answered within `deadline`?
///
/// Blocking, but bounded: safe to call from `spawn_blocking` or a plain
/// thread. The probe itself runs on its own thread precisely because it may
/// never return; this function stops WAITING for it, which is all anyone can
/// do about a syscall stuck in the kernel.
pub fn probe(path: &Path, deadline: Duration) -> Reach {
    if is_cooling_down(path) {
        return Reach::Unreachable;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let probe_path = path.to_path_buf();
    // Detached on purpose: if it is wedged, joining it would re-introduce the
    // hang this function exists to remove.
    std::thread::Builder::new()
        .name("qbz-reach-probe".to_string())
        .spawn(move || {
            // `try_exists` rather than `exists`: it distinguishes "answered,
            // not there" from "could not answer", which is exactly the
            // distinction the caller needs to decide clean-vs-skip.
            let _ = tx.send(probe_path.try_exists());
        })
        .ok();

    match rx.recv_timeout(deadline) {
        Ok(Ok(true)) => Reach::Present,
        Ok(Ok(false)) => Reach::Missing,
        Ok(Err(error)) => {
            log::warn!(
                "[reach] mount probe returned an I/O error — treating it as unreachable: {}",
                error
            );
            mark_unreachable(path);
            Reach::Unreachable
        }
        Err(_) => {
            log::warn!(
                "[reach] mount probe did not answer in {:?} — treating it as unreachable for {:?}",
                deadline,
                COOLDOWN
            );
            mark_unreachable(path);
            Reach::Unreachable
        }
    }
}

/// [`probe`] with the default deadline.
pub fn probe_default(path: &Path) -> Reach {
    probe(path, DEFAULT_PROBE)
}

/// Read metadata for `path`, bounded by `deadline`.
///
/// Only `NotFound` is classified as [`FileProbe::Missing`]. Permission errors,
/// I/O failures, symlink loops, and timeouts preserve durable registry state by
/// returning [`FileProbe::Unreachable`].
pub fn probe_file(path: &Path, deadline: Duration) -> FileProbe {
    if is_cooling_down(path) {
        return FileProbe::Unreachable;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let probe_path = path.to_path_buf();
    std::thread::Builder::new()
        .name("qbz-file-probe".to_string())
        .spawn(move || {
            let _ = tx.send(std::fs::metadata(probe_path));
        })
        .ok();

    match rx.recv_timeout(deadline) {
        Ok(Ok(metadata)) => FileProbe::Present(metadata),
        Ok(Err(error)) if error.kind() == ErrorKind::NotFound => FileProbe::Missing,
        Ok(Err(error)) => {
            log::warn!(
                "[reach] file metadata returned an I/O error — treating it as unreachable: {}",
                error
            );
            mark_unreachable(path);
            FileProbe::Unreachable
        }
        Err(_) => {
            log::warn!(
                "[reach] file metadata did not answer in {:?} — treating it as unreachable for {:?}",
                deadline,
                COOLDOWN
            );
            mark_unreachable(path);
            FileProbe::Unreachable
        }
    }
}

pub fn probe_file_default(path: &Path) -> FileProbe {
    probe_file(path, DEFAULT_PROBE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_file_is_present() {
        let f = std::env::temp_dir().join("qbz-reach-present");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(probe_default(&f), Reach::Present);
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn an_absent_file_is_missing_not_unreachable() {
        // The distinction the caller acts on: Missing may be cleaned,
        // Unreachable may only be skipped.
        let f = std::env::temp_dir().join("qbz-reach-no-such-file-9e1f");
        assert_eq!(probe_default(&f), Reach::Missing);
    }

    #[test]
    fn a_cooling_mount_answers_without_touching_the_filesystem() {
        let p = PathBuf::from("/mnt/qbz-test-dead/album/track.flac");
        mark_unreachable(&p);
        assert!(is_cooling_down(&p));
        // A sibling under the SAME mount is covered by the one probe.
        assert!(is_cooling_down(Path::new(
            "/mnt/qbz-test-dead/other/x.flac"
        )));
        assert_eq!(probe_default(&p), Reach::Unreachable);
        reset_cooldowns();
        assert!(!is_cooling_down(&p));
    }

    #[test]
    fn mount_key_groups_siblings_and_separates_mounts() {
        assert_eq!(
            mount_key(Path::new("/mnt/nas/a/b.flac")),
            mount_key(Path::new("/mnt/nas/c/d.flac"))
        );
        assert_ne!(
            mount_key(Path::new("/mnt/nas/a.flac")),
            mount_key(Path::new("/mnt/other/a.flac"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_io_error_is_unreachable_not_missing() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let loop_path = root.path().join("loop");
        symlink(&loop_path, &loop_path).unwrap();

        assert_eq!(probe_default(&loop_path), Reach::Unreachable);
        reset_cooldowns();
    }

    #[test]
    fn file_probe_returns_metadata_and_only_not_found_is_missing() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("track.flac");
        std::fs::write(&file, b"audio").unwrap();
        match probe_file_default(&file) {
            FileProbe::Present(metadata) => {
                assert!(metadata.is_file());
                assert_eq!(metadata.len(), 5);
            }
            other => panic!("unexpected probe: {other:?}"),
        }
        assert!(matches!(
            probe_file_default(&root.path().join("absent")),
            FileProbe::Missing
        ));
    }
}
