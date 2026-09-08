//! Offline-cache actions for the Qt frontend — port of the Slint
//! `crates/qbz/src/offline_cache.rs` action set (cache_track /
//! cache_tracks / cache_album / redownload_album / remove_cached /
//! refresh_cached) with the bridge-signal row sink instead of the Slint
//! event-loop hop.
//!
//! All download machinery is the shared `qbz_offline_cache` crate (CMAF-first
//! downloader, 3-permit semaphore, retry backoff, vault); this module is the
//! frontend glue: pre-flight the limit, insert the queued row, push row
//! status, spawn, toast. Offline copies are always fetched at the top
//! quality tier (reference rule, `offline_cache.rs:138-156`).
//!
//! Row status vocabulary (Slint TrackRow.slint:639-702):
//!   0 = none · 1 = queued · 2 = downloading · 3 = ready · 4 = failed
//! Status 3 rows surface `trackCacheStatusChanged` so any listening view can
//! flip its glyph live; views also seed from `offline_qt::is_cached` at
//! document build time.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use qbz_offline_cache::{CacheEvent, CacheEventSink, TrackCacheInfo};
use qbz_offline_cache::{OfflineCacheState, OfflineCacheStatus};

use crate::offline_qt;
use crate::shell_bridge;
use cxx_qt_lib::QString;

/// Push a row's cache status onto every listening view (the Qt fan-out of
/// Slint's `set_row_cache_status`). QML Connections handlers patch the row
/// inside their own document copy.
fn push_status(track_id: u64, status: i32, progress: f64) {
    let id = track_id.to_string();
    shell_bridge::ui(move |mut b| {
        b.as_mut()
            .track_cache_status_changed(QString::from(id.as_str()), status, progress);
    });
}

/// Build a sink that reflects cache + unlock events onto visible rows and
/// surfaces terminal toasts (Slint `row_sink`). Shared by the cache triggers
/// AND the play path (UnlockStart/End → the row padlock in Slint; the Qt
/// v1 does not draw the padlock — the events are still sunk here so the
/// status model stays truthful).
pub fn row_sink() -> CacheEventSink {
    Arc::new(move |ev: CacheEvent| match ev {
        CacheEvent::Started { track_id } => {
            push_status(track_id, 2, 0.0);
        }
        CacheEvent::Progress {
            track_id,
            progress_percent,
            ..
        } => {
            let p = (progress_percent as f64 / 100.0).clamp(0.0, 1.0);
            push_status(track_id, 2, p);
        }
        CacheEvent::Completed { track_id, .. } => {
            offline_qt::mark_cached(track_id, true);
            push_status(track_id, 3, 1.0);
            crate::toast_qt::success(qbz_i18n::t("Cached for offline"));
        }
        CacheEvent::Processed { .. } => {
            // Post-processing done; status already 'ready' from Completed.
        }
        CacheEvent::Failed { track_id, error } => {
            log::warn!("[qbz-qt] offline cache failed for {track_id}: {error}");
            push_status(track_id, 4, 0.0);
            crate::toast_qt::error(qbz_i18n::t("Offline caching failed"));
        }
        CacheEvent::UnlockStart { .. } | CacheEvent::UnlockEnd { .. } => {
            // The padlock row state is not drawn in the Qt port yet.
        }
    })
}

/// Build the DB row metadata from a catalog track (Slint `track_cache_info`).
///
/// `album_fallback` is `(id, title)` of the album this batch came FROM, used
/// only when the track carries no nested album of its own.
///
/// THAT IS NOT AN EDGE CASE — it is the album path's normal state. A `Track`
/// inside an `/album/get` payload has no nested `album` object (the envelope
/// would be repeating itself), so `cache_album` produced rows with
/// `album = NULL, album_id = NULL` for every track it ever queued. Measured on
/// a real index: 34 of 46 rows, three complete albums, all NULL — everything
/// downloaded through the album button since the CMAF path landed.
///
/// The damage is not cosmetic. `album_id` is the key `remove_album`,
/// `redownload_album` and the downloaded-album registry all look rows up by,
/// so an album cached this way could not be removed, refreshed or recognised
/// as downloaded, and the Offline Manager could not group it. The reference
/// has the identical bug (`offline_cache.rs:149-150` + `:311`).
fn track_cache_info(
    track: &qbz_models::Track,
    album_fallback: Option<&(String, String)>,
) -> TrackCacheInfo {
    let (album, album_id) = match track.album.as_ref() {
        Some(a) => (Some(a.title.clone()), Some(a.id.clone())),
        None => match album_fallback {
            Some((id, title)) => (Some(title.clone()), Some(id.clone())),
            None => (None, None),
        },
    };
    TrackCacheInfo {
        track_id: track.id,
        title: track.title.clone(),
        artist: track
            .performer
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_default(),
        album,
        album_id,
        duration_secs: track.duration as u64,
        quality: "UltraHiRes".to_string(),
        bit_depth: track.maximum_bit_depth,
        sample_rate: track.maximum_sampling_rate,
    }
}

fn spawn_download(off: &Arc<OfflineCacheState>, track_id: u64) {
    let file_path = off.track_file_path(track_id, "flac");
    qbz_offline_cache::spawn_track_cache_download(
        track_id,
        file_path,
        crate::app().core().client(),
        off.fetcher.clone(),
        off.db.clone(),
        off.get_cache_path(),
        off.library_db.clone(),
        row_sink(),
        off.cache_semaphore.clone(),
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollectionCacheMode {
    All,
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollectionKind {
    Album,
    Playlist,
}

impl CollectionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Playlist => "playlist",
        }
    }
}

#[derive(Clone)]
struct CacheCollection {
    kind: CollectionKind,
    key: String,
    title: String,
    tracks: Vec<qbz_models::Track>,
    album_fallback: Option<(String, String)>,
}

struct PendingCollection {
    generation: u64,
    collection: CacheCollection,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionChoiceDoc<'a> {
    kind: &'a str,
    title: &'a str,
    total_tracks: usize,
    cached_tracks: usize,
    missing_tracks: usize,
}

static COLLECTION_REQUEST_GENERATION: AtomicU64 = AtomicU64::new(0);
static PENDING_COLLECTION: OnceLock<tokio::sync::Mutex<Option<PendingCollection>>> =
    OnceLock::new();

fn pending_collection() -> &'static tokio::sync::Mutex<Option<PendingCollection>> {
    PENDING_COLLECTION.get_or_init(|| tokio::sync::Mutex::new(None))
}

fn publish_collection_ui(loading: bool, key: String, open: bool, json: String) {
    crate::offline_manager_bridge::ui(move |mut bridge| {
        bridge.as_mut().set_collection_preflight_loading(loading);
        bridge
            .as_mut()
            .set_collection_preflight_key(QString::from(key.as_str()));
        bridge.as_mut().set_collection_choice_open(open);
        bridge
            .as_mut()
            .set_collection_choice_json(QString::from(json.as_str()));
    });
}

fn begin_collection_request(key: &str) -> u64 {
    let generation = COLLECTION_REQUEST_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    publish_collection_ui(true, key.to_string(), false, "{}".to_string());
    generation
}

fn finish_collection_request(generation: u64) {
    if COLLECTION_REQUEST_GENERATION.load(Ordering::Acquire) == generation {
        publish_collection_ui(false, String::new(), false, "{}".to_string());
    }
}

fn fail_collection_request(generation: u64, message: &'static str) {
    if COLLECTION_REQUEST_GENERATION.load(Ordering::Acquire) == generation {
        finish_collection_request(generation);
        crate::toast_qt::error(qbz_i18n::t(message));
    }
}

fn unique_tracks(tracks: Vec<qbz_models::Track>) -> Vec<qbz_models::Track> {
    let mut seen = HashSet::with_capacity(tracks.len());
    tracks
        .into_iter()
        .filter(|track| track.id != 0 && seen.insert(track.id))
        .collect()
}

fn should_queue_collection_track(
    status: Option<OfflineCacheStatus>,
    mode: CollectionCacheMode,
) -> bool {
    match (mode, status) {
        // Never duplicate an active job. It already satisfies either choice.
        (_, Some(OfflineCacheStatus::Queued | OfflineCacheStatus::Downloading)) => false,
        (CollectionCacheMode::All, _) => true,
        (CollectionCacheMode::Missing, None | Some(OfflineCacheStatus::Failed)) => true,
        (CollectionCacheMode::Missing, Some(OfflineCacheStatus::Ready)) => false,
    }
}

async fn cached_statuses(
    off: &Arc<OfflineCacheState>,
) -> Result<HashMap<u64, OfflineCacheStatus>, String> {
    let guard = off.db.lock().await;
    let db = guard
        .as_ref()
        .ok_or_else(|| "offline cache database is not open".to_string())?;
    Ok(db
        .get_all_tracks()?
        .into_iter()
        .map(|track| (track.track_id, track.status))
        .collect())
}

async fn execute_collection_cache(collection: CacheCollection, mode: CollectionCacheMode) {
    let Some(off) = offline_qt::get().await else {
        crate::toast_qt::error(qbz_i18n::t("Log in to cache tracks offline"));
        return;
    };

    let statuses = match cached_statuses(&off).await {
        Ok(statuses) => statuses,
        Err(error) => {
            log::warn!("[qbz-qt] collection cache status read failed: {error}");
            crate::toast_qt::error(qbz_i18n::t("Offline caching failed"));
            return;
        }
    };
    let targets: Vec<&qbz_models::Track> = collection
        .tracks
        .iter()
        .filter(|track| should_queue_collection_track(statuses.get(&track.id).copied(), mode))
        .collect();
    if targets.is_empty() {
        crate::toast_qt::success(qbz_i18n::t("Everything is already available offline"));
        return;
    }

    let prepared: Vec<(TrackCacheInfo, String)> = targets
        .iter()
        .map(|track| {
            let id = track.id;
            (
                track_cache_info(track, collection.album_fallback.as_ref()),
                off.track_file_path(id, "flac")
                    .to_string_lossy()
                    .to_string(),
            )
        })
        .collect();

    {
        let limit = *off.limit_bytes.lock().await;
        let guard = off.db.lock().await;
        let Some(db) = guard.as_ref() else {
            return;
        };
        let root = std::path::PathBuf::from(off.get_cache_path());
        if let Err(error) = qbz_offline_cache::maintenance::check_cache_limit(db, &root, limit) {
            log::warn!("[qbz-qt] collection cache limit reached: {error}");
            crate::toast_qt::error(qbz_i18n::t(
                "Offline cache is full — free space or raise the limit",
            ));
            return;
        }
        let rows: Vec<(&TrackCacheInfo, String)> = prepared
            .iter()
            .map(|(info, path)| (info, path.clone()))
            .collect();
        if let Err(error) = db.insert_tracks_batch(&rows) {
            log::error!("[qbz-qt] collection cache insert failed: {error}");
            crate::toast_qt::error(qbz_i18n::t("Offline caching failed"));
            return;
        }
    }

    for (info, _) in &prepared {
        offline_qt::mark_cached(info.track_id, false);
        push_status(info.track_id, 1, 0.0);
        spawn_download(&off, info.track_id);
    }
    let count = prepared.len();
    crate::toast_qt::success(qbz_i18n::tf(
        "Caching {} track offline…",
        "Caching {} tracks offline…",
        count as i64,
        &[&count.to_string()],
    ));
    crate::offline_manager_qt::refresh_if_open().await;
}

async fn preflight_collection(generation: u64, collection: CacheCollection) {
    if COLLECTION_REQUEST_GENERATION.load(Ordering::Acquire) != generation {
        return;
    }
    let Some(off) = offline_qt::get().await else {
        fail_collection_request(generation, "Log in to cache tracks offline");
        return;
    };
    let statuses = match cached_statuses(&off).await {
        Ok(statuses) => statuses,
        Err(error) => {
            log::warn!("[qbz-qt] collection preflight failed: {error}");
            fail_collection_request(generation, "Offline caching failed");
            return;
        }
    };
    let cached_tracks = collection
        .tracks
        .iter()
        .filter(|track| statuses.get(&track.id) == Some(&OfflineCacheStatus::Ready))
        .count();

    // No ready copy exists: the requested fast path queues the collection
    // immediately. Failed rows are repaired and in-flight rows are left alone.
    if cached_tracks == 0 {
        finish_collection_request(generation);
        execute_collection_cache(collection, CollectionCacheMode::All).await;
        return;
    }

    let doc = CollectionChoiceDoc {
        kind: collection.kind.as_str(),
        title: &collection.title,
        total_tracks: collection.tracks.len(),
        cached_tracks,
        missing_tracks: collection.tracks.len().saturating_sub(cached_tracks),
    };
    let json = serde_json::to_string(&doc).unwrap_or_else(|_| "{}".to_string());
    {
        let mut pending = pending_collection().lock().await;
        if COLLECTION_REQUEST_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        *pending = Some(PendingCollection {
            generation,
            collection,
        });
    }
    publish_collection_ui(false, String::new(), true, json);
}

/// Cache a single track for offline playback (Slint `cache_track`).
pub fn cache_track(id: u64) {
    crate::spawn(async move {
        let Some(off) = offline_qt::get().await else {
            crate::toast_qt::error(qbz_i18n::t("Log in to cache tracks offline"));
            return;
        };
        let runtime = crate::app();
        let track = match runtime.core().get_track(id).await {
            Ok(t) => t,
            Err(e) => {
                log::error!("[qbz-qt] cache: get_track {id} failed: {e}");
                crate::toast_qt::error(qbz_i18n::t("Couldn't load that track"));
                return;
            }
        };
        // A `/track/get` payload DOES carry its album, so no fallback here.
        let info = track_cache_info(&track, None);
        let file_path = off.track_file_path(id, "flac");
        let file_path_str = file_path.to_string_lossy().to_string();

        // Pre-flight the cache limit, then insert the queued row.
        {
            let limit = *off.limit_bytes.lock().await;
            let guard = off.db.lock().await;
            let Some(db) = guard.as_ref() else {
                return;
            };
            let root = std::path::PathBuf::from(off.get_cache_path());
            if let Err(e) = qbz_offline_cache::maintenance::check_cache_limit(db, &root, limit) {
                log::warn!("[qbz-qt] cache limit reached: {e}");
                crate::toast_qt::error(qbz_i18n::t(
                    "Offline cache is full — free space or raise the limit",
                ));
                return;
            }
            if let Err(e) = db.insert_track(&info, &file_path_str) {
                log::error!("[qbz-qt] cache insert {id} failed: {e}");
                return;
            }
        }

        push_status(id, 1, 0.0);
        spawn_download(&off, id);
    });
}

/// Cache a batch of already-fetched catalog tracks (album flow, multi-select
/// bulk "Make available offline"; Slint `cache_tracks`).
pub fn cache_tracks(tracks: Vec<qbz_models::Track>) {
    cache_tracks_with_album(tracks, None)
}

/// [`cache_tracks`] for a batch that came out of ONE album document, whose
/// `(album_id, album_title)` stamps every row that has no nested album of its
/// own — see `track_cache_info`.
pub fn cache_tracks_with_album(
    tracks: Vec<qbz_models::Track>,
    album_fallback: Option<(String, String)>,
) {
    if tracks.is_empty() {
        return;
    }
    crate::spawn(async move {
        let Some(off) = offline_qt::get().await else {
            crate::toast_qt::error(qbz_i18n::t("Log in to cache tracks offline"));
            return;
        };
        // Pre-flight once for the whole batch (mirrors the reference).
        {
            let limit = *off.limit_bytes.lock().await;
            let guard = off.db.lock().await;
            let Some(db) = guard.as_ref() else {
                return;
            };
            let root = std::path::PathBuf::from(off.get_cache_path());
            if let Err(e) = qbz_offline_cache::maintenance::check_cache_limit(db, &root, limit) {
                log::warn!("[qbz-qt] batch cache limit reached: {e}");
                crate::toast_qt::error(qbz_i18n::t(
                    "Offline cache is full — free space or raise the limit",
                ));
                return;
            }
        }
        let count = tracks.len();
        for track in &tracks {
            let id = track.id;
            let info = track_cache_info(track, album_fallback.as_ref());
            let file_path = off.track_file_path(id, "flac");
            let file_path_str = file_path.to_string_lossy().to_string();
            {
                let guard = off.db.lock().await;
                let Some(db) = guard.as_ref() else {
                    return;
                };
                if db.insert_track(&info, &file_path_str).is_err() {
                    continue;
                }
            }
            push_status(id, 1, 0.0);
            spawn_download(&off, id);
        }
        crate::toast_qt::success(qbz_i18n::tf(
            "Caching {} track offline…",
            "Caching {} tracks offline…",
            count as i64,
            &[&count.to_string()],
        ));
    });
}

/// Cache a whole album for offline playback. Every whole-collection entry
/// point first checks the ready cache rows. A wholly new album takes the fast
/// path; a partial/existing one opens the shared all-vs-missing chooser.
pub fn cache_album(album_id: String) {
    let key = format!("album:{album_id}");
    let generation = begin_collection_request(&key);
    crate::spawn(async move {
        {
            let mut pending = pending_collection().lock().await;
            if COLLECTION_REQUEST_GENERATION.load(Ordering::Acquire) != generation {
                return;
            }
            *pending = None;
        }
        let runtime = crate::app();
        let album = match runtime.core().get_album(&album_id).await {
            Ok(a) => a,
            Err(e) => {
                log::error!("[qbz-qt] cache: get_album {album_id} failed: {e}");
                fail_collection_request(generation, "Couldn't load that album");
                return;
            }
        };
        let tracks = unique_tracks(album.tracks.map(|c| c.items).unwrap_or_default());
        if tracks.is_empty() {
            fail_collection_request(generation, "This album has no playable tracks");
            return;
        }
        let album_fallback = Some((album.id.clone(), album.title.clone()));
        preflight_collection(
            generation,
            CacheCollection {
                kind: CollectionKind::Album,
                key,
                title: album.title,
                tracks,
                album_fallback,
            },
        )
        .await;
    });
}

/// Cache every Qobuz member of a playlist. The Qobuz client resolves all
/// pagination before returning, so the retained snapshot is the complete
/// playlist rather than only the visible page. Repeated track ids are queued
/// once because the offline index is keyed by track id.
pub fn cache_playlist(playlist_id: String) {
    let Ok(id) = playlist_id.parse::<u64>() else {
        crate::toast_qt::error(qbz_i18n::t("Couldn't load that playlist"));
        return;
    };
    let key = format!("playlist:{id}");
    let generation = begin_collection_request(&key);
    crate::spawn(async move {
        {
            let mut pending = pending_collection().lock().await;
            if COLLECTION_REQUEST_GENERATION.load(Ordering::Acquire) != generation {
                return;
            }
            *pending = None;
        }
        let playlist = match crate::app().core().get_playlist(id).await {
            Ok(playlist) => playlist,
            Err(error) => {
                log::error!("[qbz-qt] cache: get_playlist {id} failed: {error}");
                fail_collection_request(generation, "Couldn't load that playlist");
                return;
            }
        };
        let tracks = unique_tracks(playlist.tracks.map(|c| c.items).unwrap_or_default());
        if tracks.is_empty() {
            fail_collection_request(generation, "This playlist has no playable tracks");
            return;
        }
        preflight_collection(
            generation,
            CacheCollection {
                kind: CollectionKind::Playlist,
                key,
                title: playlist.name,
                tracks,
                album_fallback: None,
            },
        )
        .await;
    });
}

pub fn confirm_collection_cache(mode: String) {
    let mode = match mode.as_str() {
        "all" => CollectionCacheMode::All,
        "missing" => CollectionCacheMode::Missing,
        _ => return,
    };
    let generation = COLLECTION_REQUEST_GENERATION.load(Ordering::Acquire);
    COLLECTION_REQUEST_GENERATION.fetch_add(1, Ordering::AcqRel);
    publish_collection_ui(false, String::new(), false, "{}".to_string());
    crate::spawn(async move {
        let collection = {
            let mut pending = pending_collection().lock().await;
            match pending.take() {
                Some(pending) if pending.generation == generation => Some(pending.collection),
                _ => None,
            }
        };
        if let Some(collection) = collection {
            log::info!(
                "[qbz-qt] offline collection choice: {} mode={mode:?}",
                collection.key
            );
            execute_collection_cache(collection, mode).await;
        }
    });
}

pub fn cancel_collection_cache() {
    let generation = COLLECTION_REQUEST_GENERATION.load(Ordering::Acquire);
    COLLECTION_REQUEST_GENERATION.fetch_add(1, Ordering::AcqRel);
    publish_collection_ui(false, String::new(), false, "{}".to_string());
    crate::spawn(async move {
        let mut pending = pending_collection().lock().await;
        if pending.as_ref().map(|p| p.generation) == Some(generation) {
            *pending = None;
        }
    });
}

/// Re-download an album's offline copies (Slint `redownload_album`,
/// `failed_only = false` from the album menu's "Refresh offline copy").
pub fn redownload_album(album_id: String, failed_only: bool) {
    crate::spawn(async move {
        let Some(off) = offline_qt::get().await else {
            return;
        };
        let targets: Vec<u64> = {
            let guard = off.db.lock().await;
            let Some(db) = guard.as_ref() else {
                return;
            };
            match db.get_album_tracks(&album_id) {
                Ok(tracks) => {
                    let picked = qbz_offline_cache::maintenance::select_redownload_targets(
                        &tracks,
                        failed_only,
                    );
                    let ids: Vec<u64> = picked.iter().map(|t| t.track_id).collect();
                    for id in &ids {
                        let _ = db.reset_track_for_redownload(*id);
                    }
                    ids
                }
                Err(_) => Vec::new(),
            }
        };
        for id in targets {
            push_status(id, 1, 0.0);
            spawn_download(&off, id);
        }
        crate::offline_manager_qt::refresh_if_open().await;
    });
}

/// Re-download ONE track (Slint `redownload_track`): reset its row and spawn
/// the download, skipping a copy that is already in flight.
///
/// Distinct from `refresh_cached` below, which DELETES the copy first: this
/// keeps the row (and its place in the index) and re-fetches into it, which is
/// what the manager's per-row refresh and its bulk arm want — a failed row has
/// nothing on disk to delete, and deleting a ready one would drop the user's
/// only copy if the network is gone by the time the fetch runs.
pub fn redownload_track(id: u64) {
    crate::spawn(async move {
        let Some(off) = offline_qt::get().await else {
            return;
        };
        {
            let guard = off.db.lock().await;
            let Some(db) = guard.as_ref() else {
                return;
            };
            if let Ok(Some(t)) = db.get_track(id) {
                if matches!(t.status, qbz_offline_cache::OfflineCacheStatus::Downloading) {
                    return;
                }
            }
            let _ = db.reset_track_for_redownload(id);
        }
        push_status(id, 1, 0.0);
        spawn_download(&off, id);
        crate::offline_manager_qt::refresh_if_open().await;
    });
}

/// Remove EVERY offline copy of an album (Slint `remove_album`): the shared
/// maintenance sweep does the DB rows + the on-disk bundles, then the library
/// rows and the session set follow it.
pub fn remove_album(album_id: String) {
    crate::spawn(async move {
        let Some(off) = offline_qt::get().await else {
            return;
        };
        let report = {
            let guard = off.db.lock().await;
            let Some(db) = guard.as_ref() else {
                return;
            };
            let root = std::path::PathBuf::from(off.get_cache_path());
            qbz_offline_cache::maintenance::remove_album_cached_tracks(db, &root, &album_id)
        };
        let report = match report {
            Ok(r) => r,
            Err(e) => {
                log::error!("[qbz-qt] remove album {album_id} failed: {e}");
                return;
            }
        };
        {
            let guard = off.library_db.lock().await;
            if let Some(db) = guard.as_ref() {
                for id in &report.removed_track_ids {
                    let _ = db.remove_qobuz_cached_track(*id);
                }
            }
        }
        for id in &report.removed_track_ids {
            offline_qt::mark_cached(*id, false);
            push_status(*id, 0, 0.0);
        }
        crate::toast_qt::success(qbz_i18n::t("Removed album from offline"));
        crate::offline_manager_qt::refresh_if_open().await;
    });
}

/// Open the cache directory in the desktop file manager (Slint `open_folder`).
///
/// Not the `rfd` used elsewhere in the port: rfd opens a PICKER, and this row
/// is "show me the folder". Routed through the `open` crate like the other
/// eleven sites in this crate — the hand-rolled `xdg-open` fallback ran on
/// every non-macOS host, Windows included, where it does not exist. `open`
/// uses ShellExecuteW there and xdg-open/open on Linux/macOS, so the two
/// supported platforms behave exactly as before.
pub fn open_folder() {
    crate::spawn(async move {
        let Some(off) = offline_qt::get().await else {
            return;
        };
        let path = off.get_cache_path();
        if let Err(e) = open::that(&path) {
            log::warn!("[qbz-qt] open offline folder failed: {e}");
        }
    });
}

/// Clear the WHOLE offline cache — DB rows, on-disk bundles and the library
/// rows (Slint `clear_all`).
///
/// The Settings copy is precise about the blast radius and so is this: it
/// removes the cached AUDIO. Purchased downloads live in the user's own music
/// folder and are not touched by `purge_all_cached_files`.
pub fn clear_all() {
    crate::spawn(async move {
        let Some(off) = offline_qt::get().await else {
            return;
        };
        if let Err(e) = qbz_offline_cache::purge_all_cached_files(&off, &off.library_db).await {
            log::error!("[qbz-qt] clear offline cache failed: {e}");
            crate::toast_qt::error(qbz_i18n::t("Couldn't clear the cache"));
            return;
        }
        offline_qt::clear_cached_ids();
        crate::toast_qt::success(qbz_i18n::t("Cache cleared"));
        crate::offline_manager_qt::refresh_if_open().await;
    });
}

/// Remove a track's offline copy (Slint `remove_cached`).
pub fn remove_cached(id: u64) {
    crate::spawn(async move {
        remove_cached_inner(id, true).await;
        crate::offline_manager_qt::refresh_if_open().await;
    });
}

/// Refresh a track's offline copy (Slint `refresh_cached`): delete + re-queue
/// sequenced in ONE task — `insert_track` is not an upsert, so the delete
/// must land first.
pub fn refresh_cached(id: u64) {
    crate::spawn(async move {
        remove_cached_inner(id, false).await;
        cache_track(id);
    });
}

/// The removal body shared by `remove_cached` and `refresh_cached` (Slint
/// `remove_cached_inner`): DB row + on-disk bundle/file + library row.
async fn remove_cached_inner(id: u64, toast: bool) {
    let Some(off) = offline_qt::get().await else {
        return;
    };
    let removed_path = {
        let guard = off.db.lock().await;
        match guard.as_ref() {
            Some(db) => db.delete_track(id).ok().flatten(),
            None => return,
        }
    };
    if let Some(p) = removed_path {
        let path = std::path::Path::new(&p);
        // v2 bundles live in `tracks-cmaf/<id>/` — remove the whole dir.
        let looks_v2 = path
            .parent()
            .and_then(|pp| pp.parent())
            .and_then(|r| r.file_name())
            .and_then(|n| n.to_str())
            == Some("tracks-cmaf");
        if looks_v2 {
            if let Some(dir) = path.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    {
        let guard = off.library_db.lock().await;
        if let Some(db) = guard.as_ref() {
            let _ = db.remove_qobuz_cached_track(id);
        }
    }
    offline_qt::mark_cached(id, false);
    push_status(id, 0, 0.0);
    if toast {
        crate::toast_qt::success(qbz_i18n::t("Removed from offline"));
    }
}

#[cfg(test)]
mod collection_tests {
    use super::{should_queue_collection_track, unique_tracks, CollectionCacheMode};
    use qbz_offline_cache::OfflineCacheStatus;

    #[test]
    fn playlist_snapshot_deduplicates_cache_keys_without_reordering() {
        let tracks = [4, 9, 4, 12]
            .into_iter()
            .map(|id| qbz_models::Track {
                id,
                ..Default::default()
            })
            .collect();
        let ids: Vec<u64> = unique_tracks(tracks)
            .into_iter()
            .map(|track| track.id)
            .collect();
        assert_eq!(ids, vec![4, 9, 12]);
    }

    #[test]
    fn missing_mode_preserves_ready_and_in_flight_rows() {
        assert!(!should_queue_collection_track(
            Some(OfflineCacheStatus::Ready),
            CollectionCacheMode::Missing
        ));
        assert!(!should_queue_collection_track(
            Some(OfflineCacheStatus::Downloading),
            CollectionCacheMode::Missing
        ));
        assert!(should_queue_collection_track(
            Some(OfflineCacheStatus::Failed),
            CollectionCacheMode::Missing
        ));
        assert!(should_queue_collection_track(
            None,
            CollectionCacheMode::Missing
        ));
    }

    #[test]
    fn all_mode_requeues_ready_but_never_duplicates_an_active_job() {
        assert!(should_queue_collection_track(
            Some(OfflineCacheStatus::Ready),
            CollectionCacheMode::All
        ));
        assert!(!should_queue_collection_track(
            Some(OfflineCacheStatus::Queued),
            CollectionCacheMode::All
        ));
    }
}
