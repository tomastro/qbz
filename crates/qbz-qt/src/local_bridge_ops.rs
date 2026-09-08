//! QbzLocal bridge OPS — the publish + load orchestration behind the
//! `local_bridge.rs` invokables.
//!
//! Split out of `local_bridge.rs` so the bridge file stays what it is meant
//! to be: ONE `#[cxx_qt::bridge]` declaration plus the thin invokable
//! forwards (cxx-qt requires the bridge module to live alone in a FLAT
//! `src/` file — QTBUG-93443). Everything here is a plain free function
//! that hops to the Qt thread through `local_bridge::ui`, so it carries no
//! cxx-qt constraints.
//!
//! Every loader publishes its own tab's document + the shared badges; the
//! Plex helpers publish the gates and drive the manual sync.

use std::sync::atomic::{AtomicU64, Ordering};

use cxx_qt_lib::QString;

use crate::local_bridge::ui;
use crate::local_library_qt as lib;
use crate::local_plex as plex;

/// Republish the tab badges after any load.
pub(crate) fn publish_counts() {
    let counts = lib::to_json(&lib::counts());
    ui(move |mut b| {
        b.as_mut()
            .set_local_counts_json(QString::from(counts.as_str()));
    });
}

/// Republish whether any browse source currently makes Local Library usable.
///
/// `localAvailable` gates the entire Local Library surface, but it used to be
/// seeded only once during bridge boot. Adding the first folder (or removing
/// the last one) then refreshed every document behind the gate without ever
/// changing the gate itself. Keep this asynchronous because `has_library`
/// opens the per-user SQLite database.
///
/// A generation prevents a slower, older database read from overwriting the
/// answer from a newer source mutation.
static AVAILABILITY_GENERATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn publish_availability() {
    let generation = AVAILABILITY_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    crate::spawn(async move {
        let available = tokio::task::spawn_blocking(lib::has_library)
            .await
            .unwrap_or(false);
        if AVAILABILITY_GENERATION.load(Ordering::Acquire) != generation {
            log::debug!("[qbz-qt] discarded stale local availability generation {generation}");
            return;
        }
        ui(move |mut b| b.as_mut().set_local_available(available));
    });
}

pub(crate) fn publish_tree(nodes_json: String) {
    ui(move |mut b| {
        b.as_mut()
            .set_local_tree_json(QString::from(nodes_json.as_str()));
        b.as_mut().set_local_tree_loading(false);
    });
}

// ---------------------------------------------------------------------------
// Plex helpers
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct PlexSectionRow {
    key: String,
    title: String,
    selected: bool,
}

/// Read the Plex gates + cached sections off the Qt thread and publish them.
pub(crate) fn publish_plex_state() {
    crate::spawn(async move {
        let snapshot = tokio::task::spawn_blocking(|| {
            let (sections, selected) = plex::cached_sections();
            let rows: Vec<PlexSectionRow> = sections
                .into_iter()
                .map(|s| PlexSectionRow {
                    selected: selected.iter().any(|k| *k == s.key),
                    key: s.key,
                    title: s.title,
                })
                .collect();
            (
                plex::is_enabled(),
                plex::is_configured(),
                serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
            )
        })
        .await;
        let Ok((enabled, available, sections_json)) = snapshot else {
            return;
        };
        ui(move |mut b| {
            b.as_mut().set_plex_enabled(enabled);
            b.as_mut().set_plex_available(available);
            b.as_mut()
                .set_plex_sections_json(QString::from(sections_json.as_str()));
        });
    });
}

/// Reload every browse document in place (after a sync / connect / toggle).
/// Push the "is this server configured" gates the filter chips read.
///
/// Called from every path that can change the answer — a connect, a
/// disconnect, the master toggle and the user bind — because a chip that
/// outlives its server filters a bucket that no longer exists.
pub(crate) fn publish_media_gates() {
    let plex = crate::local_plex::is_configured();
    let words = crate::media_servers_qt::configured_words();
    let jf = words.contains(&"jellyfin");
    let sub = words.contains(&"subsonic");
    // The saved funnel is seeded HERE, with the gates, so the two can never
    // disagree: a filter restored from disk must not tick a source whose chip
    // the popup is about to hide. That combination is invisible — the grid
    // comes back empty and the funnel badge counts a filter the user cannot
    // see, let alone clear.
    let filter = pruned_albums_filter(plex, jf, sub);
    ui(move |mut b| {
        b.as_mut().set_media_has_jellyfin(jf);
        b.as_mut().set_media_has_subsonic(sub);
        b.as_mut()
            .set_albums_filter(cxx_qt_lib::QString::from(filter.as_str()));
    });
}

/// Key the Albums funnel is stored under in the shared `ui_prefs.json`.
const ALBUMS_FILTER_KEY: &str = "local_albums_filter";

/// Persist the funnel. Called from the bridge on every toggle.
pub(crate) fn save_albums_filter(json: &str) {
    let value = if json.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(json)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
    };
    crate::settings_qt::save_pref(ALBUMS_FILTER_KEY, value);
}

/// The stored funnel with any source key its server cannot serve removed.
///
/// A source key is pruned whenever its integration is not both enabled and
/// configured. Otherwise the hidden chip can leave the grid permanently
/// empty with no control available to clear it.
fn pruned_albums_filter(has_plex: bool, has_jellyfin: bool, has_subsonic: bool) -> String {
    let Some(serde_json::Value::Object(mut map)) = crate::settings_qt::read_pref(ALBUMS_FILTER_KEY)
    else {
        return String::new();
    };
    if !has_plex {
        map.remove("plex");
    }
    if !has_jellyfin {
        map.remove("jellyfin");
    }
    if !has_subsonic {
        map.remove("subsonic");
    }
    // Only the TRUE keys travel: the view stores a key by deleting it when it
    // goes false, but a hand-edited prefs file need not honour that.
    map.retain(|_, v| v.as_bool() == Some(true));
    if map.is_empty() {
        return String::new();
    }
    serde_json::to_string(&map).unwrap_or_default()
}

pub(crate) fn reload_browse() {
    publish_media_gates();
    // The derived catalog is source-scoped too. Coalescing makes this cheap
    // for callers that already requested a catch-up, while toggle/disconnect
    // paths need it to prune a newly unauthorized source immediately.
    crate::local_catalog_qt::request_catch_up();
    // This is the shared mutation chokepoint for folders and remote sources.
    // Republish the gate as well as the documents it controls (#723).
    publish_availability();
    // The cortinilla's instant-paint cache embeds LOCAL rows and their artwork
    // paths, so anything that reaches here has just made those rows possibly
    // wrong. This is the single chokepoint for local-library mutations, which
    // is why the invalidation lives here rather than at each call site.
    crate::search_cache_qt::invalidate();
    // Expanded Genres albums cache their physical versions so switching the
    // picker never re-queries four sources. A source reconciliation changes
    // exactly that membership (and may prune provider ids), so keeping these
    // documents would republish a removed copy after the cards themselves had
    // refreshed. The visible QML document remains stable until its next open;
    // every subsequent expand/select is rebuilt from authoritative caches.
    crate::local_state::state(|state| {
        state.genre_detail_raw.clear();
        state.genre_detail_all_tracks.clear();
        state.genre_detail_versions.clear();
        state.genre_detail_docs.clear();
        state.genre_detail_filters.clear();
        state.genre_detail_requests.clear();
        state.genre_detail_version_generations.clear();
    });
    load_albums();
    load_artists();
    load_tracks(true);
}

/// The sync body shared by the header button, `plex_connect` and
/// `set_plex_sections`.
pub(crate) fn run_sync() {
    ui(|mut b| {
        b.as_mut().set_plex_syncing(true);
        b.as_mut().set_plex_error(QString::from(""));
    });
    crate::spawn(async move {
        let result = plex::sync_now().await;
        match result {
            Ok(total) => {
                log::info!("[qbz-qt] plex sync finished: {total} tracks");
                crate::local_catalog_qt::request_catch_up();
                ui(move |mut b| {
                    b.as_mut().set_plex_last_sync_tracks(total as i32);
                    b.as_mut().set_plex_syncing(false);
                });
            }
            Err(e) => {
                log::warn!("[qbz-qt] plex sync failed: {e}");
                ui(move |mut b| {
                    b.as_mut().set_plex_error(QString::from(e.as_str()));
                    b.as_mut().set_plex_syncing(false);
                });
            }
        }
        publish_plex_state();
        reload_browse();
    });
}

/// One resolved cover -> the id-keyed signal. A plain `fn` (not a closure) so
/// `local_artwork::stream_cold` can take it as a fn pointer and call it from
/// the blocking pool the moment each thumbnail lands.
pub(crate) fn emit_artwork_one(key: String, path: String) {
    ui(move |mut b| {
        b.as_mut()
            .local_artwork_ready(QString::from(key.as_str()), QString::from(path.as_str()));
    });
}

pub(crate) fn emit_artwork(pairs: Vec<(String, String)>) {
    for (key, path) in pairs {
        emit_artwork_one(key, path);
    }
}

// ---------------------------------------------------------------------------
// Loaders (each publishes its own tab's document + the shared badges)
// ---------------------------------------------------------------------------

pub(crate) fn load_tab_impl(tab: String) {
    match tab.as_str() {
        "albums" => load_albums(),
        // Artists remains on the bounded legacy reader until its own F2
        // commit, and it still derives album credits from that document.
        "albums-legacy" => load_albums_legacy(),
        "artists" => load_artists(),
        // The browser needs the bounded logical-album document even when the
        // Albums surface itself is using the optional native model.
        "genres" => load_albums_legacy(),
        "folders" => load_folders(),
        "tracks" => load_tracks(true),
        // The ephemeral session lives in memory and is published the moment it
        // is opened — there is nothing to fetch when its tab is selected. It
        // still needs an ARM: without one it lands in the `_` below and every
        // switch to the tab logs "unknown tab", which is how a real routing
        // bug would look.
        "ephemeral" => {}
        _ => log::warn!("[qbz-qt] local: unknown tab {tab}"),
    }
}

fn log_surface_route(surface: &str, path: &str, reason: &str) {
    log::info!("[local-catalog] phase=surface-route surface={surface} path={path} reason={reason}");
}

pub(crate) fn load_albums() {
    ui(|mut b| {
        b.as_mut().set_local_albums_error(QString::from(""));
    });
    if crate::local_albums_model_qt::requested()
        && crate::local_library_qt::album_mode() == "folder"
    {
        log_surface_route("albums", "catalog", "feature-enabled");
        // Once QML has supplied its width-dependent descriptor this is also
        // the retry path used by tab remounts and album-mode changes. On the
        // first mount LocalAlbumCollection supplies it immediately.
        if !crate::local_albums_model_qt::retry_last(false) {
            ui(|mut b| b.as_mut().set_local_albums_loading(true));
        }
        return;
    }
    let reason = if crate::local_albums_model_qt::requested() {
        "metadata-mode"
    } else {
        "feature-disabled-or-session-failed"
    };
    log_surface_route("albums", "legacy", reason);
    load_albums_legacy();
}

pub(crate) fn load_albums_legacy() {
    ui(|mut b| {
        b.as_mut().set_local_albums_loading(true);
        b.as_mut().set_local_albums_error(QString::from(""));
    });
    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(lib::load_albums_blocking)
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        let _ = tokio::task::spawn_blocking(lib::load_counts_blocking).await;
        match result {
            Ok(rows) => {
                // TIMED alongside the SQL/map segments in `load_albums_blocking`.
                // This document is republished on every mount of the view, and
                // the three numbers together are the only way to tell whether a
                // slow grid is the query, the mapping, the serialisation, or —
                // as it turned out — the QML parse that follows this line and
                // that Rust cannot see at all.
                let t = std::time::Instant::now();
                let json = lib::to_json(&rows);
                let ser = t.elapsed();
                log::info!(
                    "[qbz-qt][perf] local albums published: {} rows, {} bytes, serialize {ser:?}",
                    rows.len(),
                    json.len()
                );
                ui(move |mut b| {
                    b.as_mut()
                        .set_local_albums_json(QString::from(json.as_str()));
                    b.as_mut().set_local_albums_loading(false);
                });
            }
            Err(e) => {
                log::warn!("[qbz-qt] local albums load failed: {e}");
                ui(move |mut b| {
                    b.as_mut().set_local_albums_error(QString::from(e.as_str()));
                    b.as_mut().set_local_albums_loading(false);
                });
            }
        }
        publish_counts();
    });
}

/// Album identity changed: the Artists tab's album cache is keyed by the group
/// key, so it is stale the moment the mode flips (PARITY-DEBT #9 — a
/// folder-mode compilation cross-lists under every artist). Drop the cached
/// document, then re-run the load so the tab is correct even when the user is
/// standing on it. Port of `local_library.rs:727-738 invalidate_artists` (the
/// reference can stop at the drop: its `ensure_artists_loaded` guard re-fetches
/// on the next visit, a guard this port does not have).
pub(crate) fn invalidate_artists() {
    lib::state(|s| {
        s.artists.clear();
    });
    ui(|mut b| b.as_mut().set_local_artists_json(QString::from("[]")));
    load_artists();
}

pub(crate) fn load_artists() {
    ui(|mut b| b.as_mut().set_local_artists_loading(true));
    if crate::local_artists_model_qt::requested()
        && crate::local_library_qt::album_mode() == "folder"
    {
        log_surface_route("artists", "catalog", "feature-enabled");
        crate::local_artists_model_qt::retry_last();
        return;
    }
    let reason = if crate::local_artists_model_qt::requested() {
        "metadata-mode"
    } else {
        "feature-disabled-or-session-failed"
    };
    log_surface_route("artists", "legacy", reason);
    load_artists_legacy();
    if lib::state(|state| state.albums.is_empty()) {
        load_albums_legacy();
    }
}

pub(crate) fn load_artists_legacy() {
    ui(|mut b| b.as_mut().set_local_artists_loading(true));
    crate::spawn(async move {
        let rows = tokio::task::spawn_blocking(lib::load_artists_blocking)
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
            .unwrap_or_default();
        let json = lib::to_json(&rows);
        ui(move |mut b| {
            b.as_mut()
                .set_local_artists_json(QString::from(json.as_str()));
            b.as_mut().set_local_artists_loading(false);
        });
        publish_counts();
    });
}

pub(crate) fn load_folders() {
    ui(|mut b| {
        b.as_mut().set_local_folders_loading(true);
        b.as_mut().set_local_tree_loading(true);
    });
    crate::spawn(async move {
        let rows = tokio::task::spawn_blocking(lib::load_folders_blocking)
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
            .unwrap_or_default();
        let json = lib::to_json(&rows);
        ui(move |mut b| {
            b.as_mut()
                .set_local_folders_json(QString::from(json.as_str()));
            b.as_mut().set_local_folders_loading(false);
        });
        // The tree rail seeds from the registered library folders; the
        // levels below it are fetched on expand.
        let _ = tokio::task::spawn_blocking(lib::load_tree_roots_blocking).await;
        publish_tree(lib::to_json(&lib::tree_visible()));
        publish_counts();
    });
}

pub(crate) fn load_tracks(reset: bool) {
    ui(move |mut b| {
        if reset {
            b.as_mut().set_local_tracks_loading(true);
        } else {
            b.as_mut().set_local_tracks_loading_more(true);
        }
    });
    if reset {
        if crate::local_tracks_model_qt::reset() {
            log_surface_route("tracks", "catalog", "feature-enabled");
            return;
        }
        log_surface_route("tracks", "legacy", "feature-disabled-or-session-failed");
    } else {
        log_surface_route("tracks", "legacy", "legacy-pagination");
    }
    load_tracks_legacy_body(reset);
}

/// Per-surface rollback target for phase E. The native controller calls this
/// after disabling itself for the session; the old reader remains intact.
pub(crate) fn load_tracks_legacy(reset: bool) {
    ui(move |mut b| {
        if reset {
            b.as_mut().set_local_tracks_loading(true);
        } else {
            b.as_mut().set_local_tracks_loading_more(true);
        }
    });
    load_tracks_legacy_body(reset);
}

fn load_tracks_legacy_body(reset: bool) {
    // Snapshot the query/sort and mint the generation before the worker is
    // scheduled. A superseded worker may finish, but cannot mutate state or
    // publish over the newer request.
    let request = lib::begin_tracks_load(reset);
    let generation = request.generation;
    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || lib::load_tracks_page_blocking(request))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        let _ = tokio::task::spawn_blocking(lib::load_counts_blocking).await;
        match result {
            Ok(Some(load)) => {
                let serialize_started = std::time::Instant::now();
                let json = lib::to_json(&load.rows);
                let serialize_time = serialize_started.elapsed();
                log::info!(
                    "[qbz-qt][perf] tracks generation={} page_rows={} accumulated_rows={} json_bytes={} query={:?} merge={:?} map={:?} serialize={:?} candidates=local:{} plex:{} jellyfin:{} subsonic:{} selected=local:{} plex:{} jellyfin:{} subsonic:{} has_more={}",
                    load.generation,
                    load.page_rows,
                    load.rows.len(),
                    json.len(),
                    load.query_time,
                    load.merge_time,
                    load.map_time,
                    serialize_time,
                    load.candidates.local,
                    load.candidates.plex,
                    load.candidates.jellyfin,
                    load.candidates.subsonic,
                    load.published.local,
                    load.published.plex,
                    load.published.jellyfin,
                    load.published.subsonic,
                    load.has_more,
                );
                let publish_generation = load.generation;
                let has_more = load.has_more;
                ui(move |mut b| {
                    if lib::tracks_generation() != publish_generation {
                        return;
                    }
                    b.as_mut()
                        .set_local_tracks_json(QString::from(json.as_str()));
                    b.as_mut().set_local_tracks_has_more(has_more);
                    b.as_mut().set_local_tracks_loading(false);
                    b.as_mut().set_local_tracks_loading_more(false);
                });
            }
            Ok(None) => {
                log::debug!("[qbz-qt] discarded stale tracks generation {generation}");
            }
            Err(e) => {
                log::warn!("[qbz-qt] local tracks load failed: {e}");
                ui(move |mut b| {
                    if lib::tracks_generation() != generation {
                        return;
                    }
                    b.as_mut().set_local_tracks_loading(false);
                    b.as_mut().set_local_tracks_loading_more(false);
                });
            }
        }
        publish_counts();
    });
}
