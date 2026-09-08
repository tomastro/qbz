//! Offline-cache → audio-bytes bridge for playback.
//!
//! When a track is played back from the offline cache (either via the
//! main playback path or via the Local Library path when the track is a
//! Qobuz-cached offline entry), this module converts the stored row
//! into a `Vec<u8>` ready for `player.play_data`.
//!
//! For `cache_format = 2` (v2 CMAF bundle) this means:
//! 1. Read init.mp4 + segments.bin + manifest.json from disk
//! 2. Unwrap the content_key via the secret vault
//! 3. Decrypt the encrypted frames and prepend the FLAC header
//!
//! For `cache_format = 1` (legacy plain FLAC) the caller should just
//! `std::fs::read(file_path)` directly — this module doesn't handle v1
//! since v1 needs no extra work.
//!
//! This is the PURE resolution (no Tauri, no events). The UI-events
//! wrapper that emits unlock-start/end lives in `download` behind the
//! `CacheEventSink`.

use std::path::Path;

use crate::cmaf_store::{self, BundleLayout};
use crate::db::CmafBundleRow;
use crate::event::{CacheEvent, CacheEventSink};
use crate::secret_vault;

/// Run `load_cmaf_bundle` on the blocking pool and emit `UnlockStart` /
/// `UnlockEnd` through the sink so the frontend can show an "unlocking"
/// animation on the track row.
///
/// `display_track_id` is what the frontend knows this track as — for the
/// Qobuz flow it's the Qobuz track id, for Local Library it's the library
/// row id. The events carry THIS id so the UI can key off it.
///
/// `cmaf_track_id` is the key `load_cmaf_bundle` logs against (always the
/// Qobuz track id — that's what the bundle is identified by on disk).
pub async fn load_cmaf_bundle_with_ui_events(
    sink: &CacheEventSink,
    display_track_id: u64,
    cmaf_track_id: u64,
    row: CmafBundleRow,
    cache_path: String,
) -> Option<Vec<u8>> {
    sink(CacheEvent::UnlockStart {
        track_id: display_track_id,
    });
    let result = tokio::task::spawn_blocking(move || {
        load_cmaf_bundle(cmaf_track_id, &row, Path::new(&cache_path))
    })
    .await
    .ok()
    .flatten();
    sink(CacheEvent::UnlockEnd {
        track_id: display_track_id,
        success: result.is_some(),
    });
    result
}

/// Decrypt a v2 CMAF bundle row into plain FLAC bytes ready for
/// `player.play_data`. Returns `None` on any failure (missing init,
/// wrong-size unwrapped key, corrupt manifest, decrypt error). The
/// caller should treat `None` as a cache miss — continue to the next
/// tier or the network.
///
/// `offline_root_path` is what the bundle itself is located under, and what
/// locates the secret vault's install UUID file. It is the current root rather
/// than the one the row recorded, which is the whole point of this change: iOS
/// reassigns an app's data container on reinstall, so a recorded absolute path
/// stops existing while the bytes are still on disk.
/// Passing `OfflineCacheState::get_cache_path()` is correct.
pub fn load_cmaf_bundle(
    track_id: u64,
    row: &CmafBundleRow,
    offline_root_path: &Path,
) -> Option<Vec<u8>> {
    match load_bundle(track_id, row, offline_root_path) {
        Ok(flac_bytes) => {
            log::info!(
                "[OfflineCache/Play] Track {} unwrapped + decrypted ({:.2} MB FLAC)",
                track_id,
                flac_bytes.len() as f64 / (1024.0 * 1024.0)
            );
            Some(flac_bytes)
        }
        // A v1 row is not a failure, it is simply not this function's business.
        Err(BundleLoadError::NotCmaf) => None,
        Err(e) => {
            log::warn!("[OfflineCache/Play] Track {} {}", track_id, e);
            None
        }
    }
}

/// Why a bundle could not be turned back into FLAC.
///
/// The public entry point collapses all of these to `None`, because to a caller
/// they mean the same thing: fall through to the next tier. They are kept apart
/// **so a test can tell which stage failed** — without that, an implementation
/// that looked for the bundle in the wrong place would be indistinguishable from
/// one that simply could not decrypt it (see
/// `resolution_looks_under_the_current_root_not_the_recorded_path`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BundleLoadError {
    /// A v1 (plain FLAC) row. The caller reads `file_path` itself.
    NotCmaf,
    /// The row was written before the bundle finished landing.
    IncompleteRow(&'static str),
    Read(String),
    Vault(String),
    Unwrap(String),
    KeySize(usize),
    Decrypt(String),
}

impl std::fmt::Display for BundleLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCmaf => write!(f, "not a CMAF bundle"),
            Self::IncompleteRow(col) => write!(f, "cache_format=2 but {col} is null"),
            Self::Read(e) => write!(f, "failed to read CMAF bundle: {e}"),
            Self::Vault(e) => write!(f, "SecretBox init failed: {e}"),
            Self::Unwrap(e) => write!(f, "content_key unwrap failed: {e}"),
            Self::KeySize(n) => write!(f, "unwrapped content_key wrong size ({n} bytes)"),
            Self::Decrypt(e) => write!(f, "CMAF decrypt failed: {e}"),
        }
    }
}

fn load_bundle(
    track_id: u64,
    row: &CmafBundleRow,
    offline_root_path: &Path,
) -> Result<Vec<u8>, BundleLoadError> {
    if row.cache_format != 2 {
        return Err(BundleLoadError::NotCmaf);
    }
    // The column is not read for its value — the layout below is derived — but a
    // null here means the row was written before the bundle was complete, and
    // that is the one thing it still distinguishes.
    if row.init_path.is_none() {
        return Err(BundleLoadError::IncompleteRow("init_path"));
    }
    let content_key_wrapped = row
        .content_key_wrapped
        .as_ref()
        .ok_or(BundleLoadError::IncompleteRow("content_key_wrapped"))?;

    // Derived from the root the cache is open at, not from the absolute paths
    // the row recorded. The two agree until the application directory moves,
    // and then only the recorded ones are wrong: iOS reassigns an app's data
    // container on reinstall (and can on restore or migration), which leaves
    // every stored path naming a directory that no longer exists while the
    // bundle itself sits untouched under the new one.
    //
    // Deriving is safe because the index lives *inside* the root
    // (`<root>/index.db`), so a row can only ever describe a bundle under that
    // same root, and `persist_bundle` writes nowhere else. Removal and eviction
    // already resolve this way — see `maintenance::remove_album_cached_tracks`.
    let layout = BundleLayout::new(offline_root_path, track_id);
    if layout.segments_path != std::path::Path::new(&row.segments_path) {
        log::info!(
            "[OfflineCache/Play] Track {} bundle resolved under the current root; \
             the recorded path predates a move",
            track_id
        );
    }

    let loaded = cmaf_store::read_bundle(&layout).map_err(BundleLoadError::Read)?;

    let vault = secret_vault::get_or_init(offline_root_path)
        .map_err(|e| BundleLoadError::Vault(e.to_string()))?;
    let unwrapped = vault
        .unwrap(content_key_wrapped)
        .map_err(|e| BundleLoadError::Unwrap(e.to_string()))?;
    if unwrapped.len() != 16 {
        return Err(BundleLoadError::KeySize(unwrapped.len()));
    }
    let mut content_key = [0u8; 16];
    content_key.copy_from_slice(&unwrapped);

    loaded
        .decrypt_to_flac(&content_key)
        .map_err(BundleLoadError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row_recording(segments_path: &str) -> CmafBundleRow {
        CmafBundleRow {
            cache_format: 2,
            segments_path: segments_path.to_string(),
            init_path: Some("/gone/init.mp4".to_string()),
            // Deliberately not a real wrapped key: the point of these tests is
            // which stage is *reached*, and the key stage is past the read.
            content_key_wrapped: Some(vec![0u8; 8]),
            infos_wrapped: None,
            format_id: Some(6),
            n_segments: Some(1),
        }
    }

    /// The regression guard for the container-move bug.
    ///
    /// The row records a path that no longer exists — which is exactly what iOS
    /// leaves behind when it reassigns an app's data container. With nothing on
    /// disk either way this fails at the read, and `read_bundle` names the path
    /// it tried, so the message is the assertion: it must be the root the cache
    /// is open at, never the one the row recorded.
    ///
    /// Deliberately stops at the read rather than persisting a bundle and
    /// checking it got further. Getting further means reaching the secret vault,
    /// and that opens the OS keyring under the *production* service name — a
    /// unit test has no business touching a developer's login keychain.
    #[test]
    fn resolution_looks_under_the_current_root_not_the_recorded_path() {
        let root = tempfile::tempdir().unwrap();
        let row = row_recording("/recorded-elsewhere/tracks-cmaf/42/segments.bin");

        let err = load_bundle(42, &row, root.path()).unwrap_err();

        let BundleLoadError::Read(message) = &err else {
            panic!("expected a read failure with nothing on disk, got {err:?}");
        };
        // The *whole* derived path, not just the root prefix. Matching the
        // prefix alone leaves the guard blind to the two ways this can still be
        // wrong while looking under the right root: the wrong subdirectory, and
        // the wrong track id.
        let expected = BundleLayout::new(root.path(), 42);
        assert!(
            message.contains(expected.init_path.to_str().unwrap()),
            "the read should have been attempted at {:?}, got: {message}",
            expected.init_path
        );
        assert!(
            !message.contains("/recorded-elsewhere/"),
            "the recorded path was used, which is the bug this guards: {message}"
        );
    }

    #[test]
    fn a_v1_row_is_not_this_functions_business() {
        let root = tempfile::tempdir().unwrap();
        let mut row = row_recording("/gone/x.flac");
        row.cache_format = 1;

        assert_eq!(
            load_bundle(1, &row, root.path()).unwrap_err(),
            BundleLoadError::NotCmaf
        );
    }

    #[test]
    fn a_row_written_before_the_bundle_landed_is_reported_as_incomplete() {
        let root = tempfile::tempdir().unwrap();

        let mut row = row_recording("/gone/tracks-cmaf/42/segments.bin");
        row.init_path = None;
        assert_eq!(
            load_bundle(42, &row, root.path()).unwrap_err(),
            BundleLoadError::IncompleteRow("init_path")
        );

        let mut row = row_recording("/gone/tracks-cmaf/42/segments.bin");
        row.content_key_wrapped = None;
        assert_eq!(
            load_bundle(42, &row, root.path()).unwrap_err(),
            BundleLoadError::IncompleteRow("content_key_wrapped")
        );
    }
}
