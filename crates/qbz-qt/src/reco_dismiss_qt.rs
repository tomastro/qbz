//! Per-user "Not interested" dismissal store for Discover > Recommendations —
//! Slint-free port of `crates/qbz/src/reco_dismiss.rs` (1:1; only the log tag
//! changed).
//!
//! Reco-SCOPED dismissal — deliberately NOT the app-wide blacklist: a
//! dismissed artist only leaves the two Recommended-Artist rails ("More like
//! the artists you love" / "Based on what you've been into lately"); it stays
//! visible in search, home, and label pages. The paint choke point in
//! `crate::recommendations_qt` folds [`ids_snapshot`] into its exclusion set,
//! and the Blacklist Manager's "Recommendations" tab lists / undoes entries.
//!
//! This one is NOT in a shared crate: the reference lives in the Slint frontend
//! crate (`crates/qbz/src/reco_dismiss.rs`), so it is ported here rather than
//! consumed. **The on-disk format is deliberately identical** — same per-user
//! directory, same `reco_dismiss.json` file name, same key names, same
//! `to_vec_pretty` — because the two frontends share the user directory and a
//! dismissal made in one must be honoured by the other.
//!
//! Shape follows the light `discovery_dismiss` precedent (one small JSON file,
//! a full read on each op — no SQLite, no change-notify) but bound PER-USER
//! like `fav_cache_qt` / `artist_blacklist`: a process-global path set via
//! [`init_for_user`] / dropped by [`teardown`]. Fail-open everywhere: with no
//! session bound (or a corrupt file) reads are empty and mutations no-op.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// JSON file name inside the per-user data dir.
const FILE_NAME: &str = "reco_dismiss.json";

/// One dismissed artist. `image_url` is persisted but never rendered by the
/// manager tab (the row draws a generic avatar); it is stored so a future
/// surface can show a thumbnail — and because a blank first write can never be
/// repaired, [`dismiss`] only backfills an EMPTY field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DismissedArtist {
    #[serde(default)]
    pub artist_id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub image_url: String,
}

#[derive(Default, Serialize, Deserialize)]
struct DismissStore {
    #[serde(default)]
    artists: Vec<DismissedArtist>,
}

/// The bound per-user file. `None` outside an active session (pure fail-open
/// window), matching the `fav_cache_qt` lifecycle.
static STORE_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Bind the per-user store path from `<dir>/reco_dismiss.json`. Called on
/// every session activation — login, restore, AND offline entry — next to
/// `fav_cache_qt::init_for_user`.
pub fn init_for_user(base_dir: &Path) {
    if let Ok(mut guard) = STORE_PATH.lock() {
        *guard = Some(base_dir.join(FILE_NAME));
    }
}

/// Drop the binding on logout. Mirrors `fav_cache_qt::teardown`.
pub fn teardown() {
    if let Ok(mut guard) = STORE_PATH.lock() {
        *guard = None;
    }
}

fn store_path() -> Option<PathBuf> {
    STORE_PATH.lock().ok().and_then(|g| g.clone())
}

/// Fail-open read: no binding, unreadable file, or unknown/corrupt format all
/// yield an empty store (a corrupted file never blocks recommendations — the
/// user simply re-dismisses).
fn load_store() -> DismissStore {
    let Some(path) = store_path() else {
        return DismissStore::default();
    };
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => DismissStore::default(),
    }
}

fn write_store(store: &DismissStore) {
    let Some(path) = store_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("[qbz-qt] reco-dismiss dir failed: {e}");
            return;
        }
    }
    match serde_json::to_vec_pretty(store) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log::warn!("[qbz-qt] reco-dismiss write failed: {e}");
            }
        }
        Err(e) => log::warn!("[qbz-qt] reco-dismiss serialize failed: {e}"),
    }
}

/// Snapshot of the dismissed id set — the reco paint-filter input. Empty when
/// no session is bound. Id 0 (a corrupt row) never matches.
// Consumed by the reco exclusion set (spec 03 §9.2 item F21), which lands with
// the recommendations filter slice.
pub fn ids_snapshot() -> HashSet<u64> {
    load_store()
        .artists
        .into_iter()
        .map(|a| a.artist_id)
        .filter(|id| *id != 0)
        .collect()
}

/// All dismissed artists in insertion order (NEVER sorted — the manager's
/// Recommendations tab deliberately has no `COLLATE NOCASE` ordering, unlike
/// the two blacklist axes). Empty on no session or a corrupt file.
pub fn list() -> Vec<DismissedArtist> {
    load_store()
        .artists
        .into_iter()
        .filter(|a| a.artist_id != 0)
        .collect()
}

/// Record a dismissal (idempotent). A re-dismiss with a richer snapshot
/// backfills a previously empty name/image (e.g. first dismissed offline,
/// where the name could not be resolved); a populated field is never
/// overwritten.
pub fn dismiss(artist_id: u64, name: &str, image_url: &str) {
    if artist_id == 0 {
        return;
    }
    let mut store = load_store();
    match store.artists.iter().position(|a| a.artist_id == artist_id) {
        Some(idx) => {
            let existing = &mut store.artists[idx];
            if existing.name.is_empty() && !name.is_empty() {
                existing.name = name.to_string();
            }
            if existing.image_url.is_empty() && !image_url.is_empty() {
                existing.image_url = image_url.to_string();
            }
        }
        None => store.artists.push(DismissedArtist {
            artist_id,
            name: name.to_string(),
            image_url: image_url.to_string(),
        }),
    }
    write_store(&store);
}

/// Remove a dismissal (the manager tab's undo). No-op when absent / unbound.
pub fn remove(artist_id: u64) {
    let mut store = load_store();
    let before = store.artists.len();
    store.artists.retain(|a| a.artist_id != artist_id);
    if store.artists.len() != before {
        write_store(&store);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp dir under the system temp root (no `tempfile` dev-dep on
    /// qbz-qt). Created here, removed at the end of the test.
    fn unique_temp_dir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("qbz-qt-reco-dismiss-test-{nanos}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// One combined test: the path singleton is process-global, so splitting
    /// into parallel tests would let them clobber each other. Covers the full
    /// lifecycle: bind, dismiss (idempotent + snapshot backfill), persistence
    /// across a re-bind, remove, unknown-format tolerance, and the fail-open
    /// unbound state.
    #[test]
    fn lifecycle_roundtrip() {
        let dir = unique_temp_dir();

        init_for_user(&dir);
        assert!(ids_snapshot().is_empty(), "fresh store has no ids");
        assert!(list().is_empty(), "fresh store has no rows");

        dismiss(42, "Artist X", "https://img/x.jpg");
        dismiss(7, "Artist Y", "");
        assert!(ids_snapshot().contains(&42));
        assert!(ids_snapshot().contains(&7));
        assert_eq!(list().len(), 2);

        // Idempotent: a re-dismiss does not duplicate, but backfills an empty
        // snapshot field.
        dismiss(42, "Artist X", "https://img/x.jpg");
        dismiss(7, "Artist Y", "https://img/y.jpg");
        assert_eq!(list().len(), 2, "no duplicate rows");
        assert_eq!(list()[1].image_url, "https://img/y.jpg", "image backfilled");

        // Id 0 is rejected and never matches.
        dismiss(0, "Nobody", "");
        assert!(!ids_snapshot().contains(&0));

        // Persistence: re-binding the same dir loads the file back.
        teardown();
        assert!(ids_snapshot().is_empty(), "fail-open after teardown");
        dismiss(9, "Lost", ""); // no-op while unbound; must not panic
        init_for_user(&dir);
        assert!(ids_snapshot().contains(&42), "rows survive a re-bind");
        assert!(
            !ids_snapshot().contains(&9),
            "unbound mutation did not persist"
        );

        // Undo.
        remove(42);
        assert!(!ids_snapshot().contains(&42));
        assert_eq!(list().len(), 1);
        remove(42); // absent: no-op, no write needed

        // Unknown-format tolerance: junk in the file reads as an empty store.
        std::fs::write(dir.join(FILE_NAME), b"{ not json !!").expect("write junk");
        assert!(ids_snapshot().is_empty(), "corrupt file fails open");
        assert!(list().is_empty());
        // A partially-unknown row shape degrades to defaults, not a crash.
        std::fs::write(
            dir.join(FILE_NAME),
            br#"{"artists":[{"artist_id":5},{"artist_id":0,"name":"zero"}]}"#,
        )
        .expect("write partial");
        assert!(ids_snapshot().contains(&5), "partial row still loads");
        assert!(!ids_snapshot().contains(&0), "id 0 row is dropped");

        teardown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
