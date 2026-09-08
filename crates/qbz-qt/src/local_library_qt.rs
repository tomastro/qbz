//! Local Library data layer — FACADE.
//!
//! The Qt/QML port of the shipping Slint controller (`crates/qbz/src/
//! local_library.rs`). ADR-006: NOTHING here re-implements scanning, album
//! identity or the queries — every read goes through the frontend-agnostic
//! `qbz-library` crate (`LibraryDatabase`), and every Plex read goes through
//! `qbz-plex` + the shared `qbz_app::settings::plex` store.
//!
//! This file used to be the whole layer (1,237 lines). It is now a thin
//! re-export surface over five sibling modules, one per concern — the split
//! is by CONCERN, not by line count:
//!
//! | module              | owns                                              |
//! |---------------------|---------------------------------------------------|
//! | `local_rows`        | the transport rows + `qbz_library` -> row mappers |
//! | `local_state`       | `library.db` access, prefs, the document cache    |
//! | `local_plex`        | Plex settings/gates/cache reads + the manual sync |
//! | `local_albums`      | albums, artists, Tracks pages, badges, detail     |
//! | `local_tree`        | the lazy folder tree + folder detail              |
//! | `local_artwork`     | windowed, id-keyed artwork (local + Plex thumbs)  |
//! | `local_playback`    | queue mapping + the source-aware audible steps    |
//!
//! Everything the bridge (`local_bridge.rs`) and the shared playback
//! controller call is re-exported here, so call sites keep reading
//! `crate::local_library_qt::…`.
//!
//! PLEX (wired 2026-07-28, was the port's headline gap):
//! - Albums/badges: `get_albums_metadata_page(..., plex_cache_path, ...)`
//!   ATTACHes `<data_dir>/qbz/plex_cache.db` when the master toggle is ON, so
//!   the grid is the local+Plex UNION. Toggle OFF -> `None` -> local-only,
//!   byte-for-byte the pre-Plex behaviour.
//! - Tracks: local, Plex, Jellyfin and Subsonic each keep an independent
//!   offset; bounded candidate pages are merged into one stable global page.
//! - Artists: the aggregated Plex artists fold into the rail by name.
//! - Artwork: `/library/...` thumbs resolve through the shared image cache
//!   with a tokenized transcode URL; no token ever reaches QML.
//! - Playback: Plex rows carry their `rating_key` in `source_item_id_hint`
//!   and resolve their direct-play part at play time.
//!
//! Network-folder visibility follows live connectivity through
//! `offline_fwd::exclude_network_folders_now`; source playback is delegated to
//! the shared registry/audible seams.

// The stable facade over the local_* modules. Keep this list equal to the
// symbols consumed through the `lib::` aliases in the bridge/ops/bulk modules
// plus the direct playback and diagnostics callers.

// --- rows ------------------------------------------------------------------
pub use crate::local_rows::to_json;

// --- state / prefs ---------------------------------------------------------
pub use crate::local_state::{
    album_mode, begin_tracks_load, counts, explorer_columns, has_library, set_album_mode,
    set_explorer_columns, set_tracks_filter, set_tracks_group, set_tracks_query, set_tracks_sort,
    state, tracks_filter, tracks_generation, tracks_group, tracks_has_more, tracks_sort,
};

// --- queries ---------------------------------------------------------------
pub use crate::local_albums::{
    load_album_detail_filtered_blocking, load_albums_blocking, load_artists_blocking,
    load_counts_blocking, load_folders_blocking, load_tracks_page_blocking,
};

// --- folder tree -----------------------------------------------------------
pub use crate::local_tree::{
    load_folder_detail_blocking, load_tree_roots_blocking, set_tree_search, tree_collapse,
    tree_collapse_all, tree_expand_blocking, tree_visible,
};

// --- artwork ---------------------------------------------------------------
pub use crate::local_artwork::{fetch_plex_misses, resolve_window_blocking};

// --- playback --------------------------------------------------------------
pub use crate::local_playback::{
    enqueue, enqueue_album_filtered, play_album, play_album_filtered, play_current_if_local,
    play_folder, play_folder_track, play_tracks_visible,
};

/// Drop every cached document AND unbind the Plex store (logout / user
/// switch — the next read re-binds to the new user's `plex_settings.db`).
pub fn reset() {
    crate::local_tree::tree_clear_selection();
    crate::local_state::reset();
    crate::local_plex::reset();
}
