//! Offline cache purge helper.
//!
//! Used by `session_lifecycle` to clear all cached audio when the active
//! session is torn down. Not a Tauri command — pure helper.

use crate::OfflineCacheState;

/// Directory names under the offline cache root that purge may remove entirely.
const PURGEABLE_CACHE_DIRS: &[&str] = &["tracks", "tracks-cmaf", "artwork"];

/// True when `name` is a known QBZ offline-cache layout directory.
pub(crate) fn is_purgeable_cache_dir_name(name: &str) -> bool {
    PURGEABLE_CACHE_DIRS.contains(&name)
}

/// Clear entire offline cache (internal helper).
///
/// `library_db` is the (locked-on-demand) main library connection — the
/// caller passes its `Arc<Mutex<Option<LibraryDatabase>>>` so this helper
/// doesn't depend on any frontend's state-wrapper type.
pub async fn purge_all_cached_files(
    cache_state: &OfflineCacheState,
    library_db: &tokio::sync::Mutex<Option<qbz_library::LibraryDatabase>>,
) -> Result<(), String> {
    let paths = {
        let guard__ = cache_state.db.lock().await;
        let db = guard__
            .as_ref()
            .ok_or("No active session - please log in")?;
        db.clear_all()?
    };

    // Delete all files. For v1 entries `file_path` is the plain FLAC;
    // for v2 entries it's `<dir>/segments.bin` — we remove the enclosing
    // track directory so init.mp4 + manifest.json + segments.bin all go
    // in one step. Plain files still work because remove_dir_all on a
    // file path fails silently and we fall through to remove_file.
    for path in paths {
        let p = std::path::Path::new(&path);
        if !p.exists() {
            continue;
        }
        // Heuristic: the v2 layout puts everything inside `tracks-cmaf/<id>/`.
        // If the parent directory matches that shape, remove the directory.
        let looks_like_v2 = p
            .parent()
            .and_then(|parent| parent.parent())
            .and_then(|root| root.file_name())
            .and_then(|n| n.to_str())
            == Some("tracks-cmaf");
        if looks_like_v2 {
            if let Some(track_dir) = p.parent() {
                let _ = std::fs::remove_dir_all(track_dir);
                continue;
            }
        }
        let _ = std::fs::remove_file(p);
    }

    // Clear only known layout directories under the cache root (allowlist).
    // Never recursive-delete arbitrary siblings — offline root may be a shared
    // mount and non-QBZ folders must survive purge.
    let cache_dir = cache_state.cache_dir.read().unwrap().clone();
    if let Ok(entries) = std::fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if !is_purgeable_cache_dir_name(name) {
                continue;
            }
            if path.is_dir() {
                if let Err(e) = std::fs::remove_dir_all(&path) {
                    log::warn!("[OfflineCache/Purge] Failed to remove {path:?}: {e}");
                } else {
                    log::info!("[OfflineCache/Purge] Removed allowlisted dir {name}");
                }
            }
        }
    }

    // Remove all Qobuz cached tracks from library
    let guard__ = library_db.lock().await;
    let library_db = guard__
        .as_ref()
        .ok_or("No active session - please log in")?;
    let removed_count = library_db
        .remove_all_qobuz_cached_tracks()
        .map_err(|e| format!("Failed to remove cached tracks from library: {}", e))?;
    log::info!("Removed {} Qobuz cached tracks from library", removed_count);

    Ok(())
}

#[cfg(test)]
mod purge_allowlist_tests {
    use super::is_purgeable_cache_dir_name;

    #[test]
    fn only_known_layout_dirs_are_purgeable() {
        assert!(is_purgeable_cache_dir_name("tracks"));
        assert!(is_purgeable_cache_dir_name("tracks-cmaf"));
        assert!(is_purgeable_cache_dir_name("artwork"));
        assert!(!is_purgeable_cache_dir_name("Music"));
        assert!(!is_purgeable_cache_dir_name("Artist Name"));
        assert!(!is_purgeable_cache_dir_name(".."));
    }
}
