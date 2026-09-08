//! Local Library BULK actions — the three multi-select bars.
//!
//! Three surfaces, ONE action vocabulary (LocalLibraryView.slint: the tree
//! rail's compact bar at :1779, the albums grid bar at :1251, the tracks
//! table bar at :1419):
//!
//! | surface       | selection lives in | entry point            |
//! |---------------|--------------------|------------------------|
//! | tree rail     | Rust (`local_tree`)| `folders_bulk_action`  |
//! | albums grid   | QML                | `bulk_action("album")` |
//! | tracks table  | QML                | `bulk_action("track")` |
//!
//! The split is not arbitrary: a tree selection is a set of FILE PATHS that
//! only Rust can expand (a folder check means "every track under me,
//! recursively" — a query), while the grid/table select rows the QML already
//! holds, so it keeps the ids and hands them over per action. That is exactly
//! how the Slint does it, and why `select-all` / `clear` never reach Rust for
//! the grid and table but DO for the tree.
//!
//! Once resolved, every surface funnels into the same `apply` — the same
//! queue seam a single-row context menu takes (`local_playback::
//! local_queue_track` + the core queue helpers), just with N rows.
//!
//! This file is the Qt-facing half of the tree selection; the state and the
//! blocking mutators live in `local_tree.rs`, next to the tree they annotate.

use std::collections::HashMap;

use cxx_qt_lib::QString;
use qbz_library::LocalTrack;
use qbz_models::QueueTrack;

use crate::local_albums::fetch_album_tracks_blocking;
use crate::local_bridge::ui;
use crate::local_library_qt as lib;
use crate::local_playback::local_queue_track;
use crate::local_state::{state, with_db};
use crate::local_tree as tree;

// ---------------------------------------------------------------------------
// Publish
// ---------------------------------------------------------------------------

/// Republish the tree document + the bulk bar's counter after a selection
/// mutation.
///
/// Deliberately NOT `local_bridge_ops::publish_tree`: that one is the LOAD
/// publish and also clears `localTreeLoading`, which a selection change must
/// leave alone.
fn publish_selection() {
    let json = lib::to_json(&tree::tree_visible());
    let count = tree::tree_selected_count();
    ui(move |mut b| {
        b.as_mut().set_local_tree_json(QString::from(json.as_str()));
        b.as_mut().set_local_tree_selected_count(count);
    });
}

// ---------------------------------------------------------------------------
// Tree rail: select mode + the two checkboxes
// ---------------------------------------------------------------------------

/// Rail header toggle. Leaving select mode drops the selection, so the
/// republish is what hollows every checkbox again.
pub fn set_select_mode(on: bool) {
    tree::set_tree_select_mode(on);
    publish_selection();
}

/// Folder checkbox: recursive, so it is a DB read on a blocking thread.
pub fn toggle_folder_select(path: String) {
    crate::spawn(async move {
        let _ =
            tokio::task::spawn_blocking(move || tree::toggle_folder_select_blocking(&path)).await;
        publish_selection();
    });
}

/// Track checkbox: a deselect is pure state, a select resolves the record
/// from the parent folder listing — blocking either way.
pub fn toggle_track_select(path: String) {
    crate::spawn(async move {
        let _ =
            tokio::task::spawn_blocking(move || tree::toggle_track_select_blocking(&path)).await;
        publish_selection();
    });
}

// ---------------------------------------------------------------------------
// The bulk bars
// ---------------------------------------------------------------------------

/// Tree-rail bulk bar. `select-all` and `clear` mutate the Rust-side
/// selection; everything else acts on a snapshot of it.
///
/// The bar is only VISIBLE with a non-empty selection (LocalTreeRail.qml:75),
/// so an empty selection can only be reached by a stale call — it no-ops
/// rather than enqueuing nothing.
pub fn folders_bulk_action(action: String) {
    match action.as_str() {
        "select-all" => {
            // Two-way: "check all" un-checks when everything already is.
            crate::spawn(async move {
                let _ = tokio::task::spawn_blocking(tree::tree_select_all_blocking).await;
                publish_selection();
            });
        }
        "clear" => {
            tree::tree_clear_selection();
            publish_selection();
        }
        _ => {
            let rows = tree::tree_selected_snapshot();
            crate::spawn(async move {
                if apply(rows, &action).await {
                    tree::tree_clear_selection();
                    publish_selection();
                }
            });
        }
    }
}

/// Albums-grid / Tracks-table bulk bar. `scope` = "album" | "track",
/// `ids_json` = the JSON string array the QML built from its own selection
/// map. `select-all` / `clear` never arrive here — the QML owns that
/// selection and short-circuits them (LocalLibraryView.qml:405/425).
pub fn bulk_action(scope: String, ids_json: String, action: String) {
    let ids: Vec<String> = serde_json::from_str(&ids_json).unwrap_or_default();
    if ids.is_empty() {
        log::debug!("[qbz-qt] local bulk {action}: empty {scope} selection, ignored");
        return;
    }
    if matches!(action.as_str(), "track-info" | "album-info") {
        crate::local_media_info_qt::begin();
    }
    crate::spawn(async move {
        let rows = tokio::task::spawn_blocking(move || resolve_blocking(&scope, &ids))
            .await
            .unwrap_or_default();
        apply(rows, &action).await;
    });
}

// ---------------------------------------------------------------------------
// Resolution + the shared action body
// ---------------------------------------------------------------------------

/// Selected ids -> the raw rows an action operates on.
///
/// - `album`: group keys, expanded through the SAME query album detail and
///   album playback use, so a `plex:<hash>` key resolves identically.
/// - `track`: Tracks-table row ids. The LOADED page (`tracks_raw`, plus the
///   open detail pane) is the model of truth — a merged PLEX row is not in
///   `local_tracks` and could never be re-queried by id; the DB lookup is a
///   last resort for a local id that scrolled out of the cached page.
fn resolve_blocking(scope: &str, ids: &[String]) -> Vec<LocalTrack> {
    if scope == "album" {
        return ids
            .iter()
            .flat_map(|key| fetch_album_tracks_blocking(key))
            .collect();
    }
    let cached: HashMap<i64, LocalTrack> = state(|s| {
        s.tracks_raw
            .iter()
            .chain(s.detail_raw.iter())
            // The Library Explorer's expanded albums live in their own row
            // cache. Without these, an Explorer track menu resolved through
            // this function silently dropped every merged Plex/remote row —
            // "Add to playlist" opened nothing (smoke 2026-08-30).
            .chain(s.genre_detail_raw.values().flatten())
            .chain(
                s.genre_detail_all_tracks
                    .values()
                    .flat_map(|arc| arc.iter()),
            )
            .map(|t| (t.id, t.clone()))
            .collect()
    });
    ids.iter()
        .filter_map(|id| id.parse::<i64>().ok())
        .filter_map(|row| {
            if let Some(t) = cached.get(&row) {
                return Some(t.clone());
            }
            if crate::local_plex::is_plex_track_id(row) {
                return None;
            }
            with_db(|db| db.get_track(row)).flatten()
        })
        .collect()
}

/// Track-scope resolution for callers outside the bulk bars (the shared
/// drag's local payload): same caches, same last-resort DB lookup.
pub(crate) fn resolve_track_rows_blocking(ids: &[String]) -> Vec<LocalTrack> {
    resolve_blocking("track", ids)
}

pub(crate) fn resolve_album_ids_blocking(ids: &[String]) -> Vec<LocalTrack> {
    ids.iter()
        .flat_map(|key| fetch_album_tracks_blocking(key))
        .collect()
}

/// Run one action over already-resolved rows. Returns whether the caller
/// should DROP its selection afterwards — the Slint clears after an enqueue
/// and keeps it while a picker is still open.
pub(crate) async fn apply(rows: Vec<LocalTrack>, action: &str) -> bool {
    match action {
        "track-info" => {
            if let Some(track) = rows.into_iter().next() {
                crate::local_media_info_qt::open_track(track);
            } else {
                crate::local_media_info_qt::open_empty("track");
            }
            false
        }
        "album-info" => {
            crate::local_media_info_qt::open_album(rows);
            false
        }
        "edit-metadata" => {
            if let Some(track) = rows.into_iter().next() {
                crate::tag_editor_qt::open_track(track);
            }
            false
        }
        "queue" | "play-next" | "play-later" => {
            if rows.is_empty() {
                return false;
            }
            enqueue_rows(rows, action).await;
            true
        }
        // The MyQBZ picker — 1:1 with the Slint's `myqbz_add` route. Returns
        // FALSE (keep the selection): the picker is still open and a failed
        // write is retried from the same modal, which is exactly what this
        // function's doc-comment describes.
        "add-to-mixtape" => {
            if rows.is_empty() {
                return false;
            }
            crate::myqbz_add_qt::open_items(crate::myqbz_add_qt::track_items_from_local(&rows));
            false
        }
        // The app-wide picker in LOCAL MODE — the Slint's
        // `playlist_picker::open_multi(&ids, local = true)`. The refs are
        // SOURCE-AWARE (`local_picker_ref_for_track`: Plex rows as
        // "plex:<rating key>", everything else as its library row id, resolved
        // at insert time), and they are carried as refs the whole way: the
        // picker's `Payload::LocalRefs` is the type that keeps a library row
        // id from ever reaching the Qobuz endpoint, where it would mean a
        // different track.
        //
        // Returns FALSE (keep the selection) for the same reason
        // `add-to-mixtape` does: the picker is still open and a failed write
        // is retried from the same modal.
        "add-to-playlist" => {
            if rows.is_empty() {
                return false;
            }
            // Shared tail: source-aware refs, then the picker.
            crate::local_album_actions::open_picker_for_rows(&rows);
            false
        }
        // NO CALLER as of 2026-07-31: no bulk bar offers this action any more.
        // The Local Library Tracks tab used to (LocalTracksTab.qml), and the
        // owner removed it as a context error — that tab is the whole local
        // library, not a favourites surface. The arm is kept because the
        // vocabulary is shared with the surfaces that WILL want it once local
        // hearts land; it must stay a no-op until then.
        //
        // GAP: local hearts are not wired in this port at all — `map_track`
        // publishes `isFavorite: false` unconditionally, and the local
        // favorites store is keyed by FILE PATH behind a private handle in
        // `library_qt.rs` (its public `toggle_favorite` would route a numeric
        // local id to the Qobuz API, which is worse than doing nothing).
        "remove-favorites" => {
            log::warn!(
                "[qbz-qt] local bulk remove-favorites: local favorites not wired ({} row(s) ignored)",
                rows.len()
            );
            false
        }
        other => {
            log::warn!("[qbz-qt] local bulk: unknown action {other}");
            false
        }
    }
}

/// The bulk enqueue. Identical semantics to the single-row context menu
/// (`local_playback::enqueue`): "play-next" inserts at the cursor REVERSED so
/// a multi-row insert keeps its order, "play-later" appends to the manual
/// block's tail, anything else appends.
///
/// The shared enqueue seam runs before any core mutation. While QConnect is
/// enabled it drops this local-only batch, raises the single counted notice,
/// and leaves the existing queue untouched.
async fn enqueue_rows(rows: Vec<LocalTrack>, mode: &str) {
    let runtime = crate::app();
    // Same folder-cover backfill the single-row path runs
    // (`local_playback::enqueue`): a bulk-queued row must reach the queue with
    // the cover its folder has, not a blank thumbnail. Blocking fs, so it goes
    // through spawn_blocking like every other caller.
    let rows = tokio::task::spawn_blocking(move || {
        let mut rows = rows;
        crate::local_playback::fill_missing_covers(&mut rows);
        rows
    })
    .await
    .unwrap_or_default();
    let queue: Vec<QueueTrack> = rows.iter().map(local_queue_track).collect();
    let queue = crate::playback_qt::stamped(queue, None);
    if queue.is_empty() {
        return;
    }
    log::info!("[qbz-qt] local bulk {mode}: {} track(s)", queue.len());
    let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
        return;
    };
    match mode {
        "play-next" => {
            for t in queue.into_iter().rev() {
                runtime.core().add_track_next(t).await;
            }
        }
        "play-later" => {
            for t in queue {
                runtime.core().add_track_later(t).await;
            }
        }
        _ => runtime.core().add_tracks(queue).await,
    }
    crate::playback_qt::publish_queue(&runtime).await;
}
