//! Playlist folders — port of `crates/qbz/src/folders.rs`.
//!
//! Local-only organization stored in the per-user `library.db` (the SAME file
//! the reference build writes, so a user's folders survive switching between
//! the two). Folders are FLAT — no nesting; a playlist belongs to at most one
//! folder via `playlist_settings.folder_id`.
//!
//! Every call opens the DB, so they are all blocking; async callers wrap them
//! in `spawn_blocking`. `sidebar_qt.rs` already had its own inline copy of the
//! three reads it needs — that one stays, because it opens the DB once and
//! answers four questions in a single pass, which is the right shape for a
//! per-publish hot path.

use std::collections::HashMap;

use crate::library_db_qt::with_db;

/// Full folder record for the Playlist Manager (icon + color + hidden).
///
/// `Debug` because `playlist_manager_rows::PmData` derives it and holds a
/// `Vec<FolderFull>`.
#[derive(Clone, Debug, Default)]
pub struct FolderFull {
    pub id: String,
    pub name: String,
    pub icon_type: String,
    pub icon_preset: String,
    pub icon_color: String,
    pub custom_image_path: Option<String>,
    pub is_hidden: bool,
}

/// Per-playlist local settings the manager merges onto the remote list.
#[derive(Clone, Default)]
pub struct PlaylistSettingsLite {
    pub hidden: bool,
    pub is_favorite: bool,
    pub position: i32,
    pub folder_id: Option<String>,
}

/// All folders with their full icon/color records, ordered by position.
pub fn load_folders_full() -> Vec<FolderFull> {
    with_db(false, |db| db.get_all_playlist_folders())
        .unwrap_or_default()
        .into_iter()
        .map(|f| FolderFull {
            id: f.id,
            name: f.name,
            icon_type: f.icon_type,
            icon_preset: f.icon_preset,
            icon_color: f.icon_color,
            custom_image_path: f.custom_image_path,
            is_hidden: f.is_hidden,
        })
        .collect()
}

/// playlist id -> its local settings (hidden / favorite / position / folder).
pub fn playlist_settings_map() -> HashMap<u64, PlaylistSettingsLite> {
    with_db(false, |db| db.get_all_playlist_settings())
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            (
                s.qobuz_playlist_id,
                PlaylistSettingsLite {
                    hidden: s.hidden,
                    is_favorite: s.is_favorite,
                    position: s.position,
                    folder_id: s.folder_id,
                },
            )
        })
        .collect()
}

/// playlist id -> play count (the "Play Count" sort + the list badge).
///
/// Read from `playlist_play_history.db` — the store the track-start edge
/// actually writes (`recently_qt::record_queue_track`). The library's
/// `playlist_stats.play_count` has NO writer in the whole workspace
/// (`increment_playlist_play_count` has zero callers), so the badge and the
/// sort were 0 for everyone, always. One play event per track start, so a
/// 40-track playlist played through counts 40 — the same number the
/// "Recently Played Playlists" card shows. Local playlists (`local:<uuid>`)
/// are not numeric ids and are not counted here (their row type has no
/// badge).
pub fn playlist_play_counts() -> HashMap<u64, u32> {
    qbz_app::settings::playlist_play_history::all_recent_playlists()
        .into_iter()
        .filter_map(|row| row.playlist_id.parse::<u64>().ok().map(|id| (id, row.plays)))
        .collect()
}

/// playlist id -> local (non-Qobuz) track count.
pub fn playlist_local_counts() -> HashMap<u64, u32> {
    with_db(false, |db| db.get_all_playlist_local_track_counts()).unwrap_or_default()
}

/// Create a folder with an icon preset + colour (the manager's create path).
///
/// `create: true` — a fresh account has no library.db until the first local
/// flag is written, and "create your first folder" must not be the one action
/// that silently does nothing on a new install.
pub fn create_folder_full(name: &str, icon_preset: &str, icon_color: &str) -> Option<FolderFull> {
    let preset = Some(icon_preset);
    // `""` means "use the theme accent" and it has to REACH the DB as an empty
    // string. Mapping it to `None` made the DB substitute its own default
    // (`#6366f1`, database.rs:3814), so picking the Accent swatch on a NEW
    // folder silently stored indigo — the modal closed, the tile came back
    // purple and nothing reported why. `""` reads back through the Slint
    // build's `parse_color("")` -> None -> has_color false -> accent fallback,
    // so both builds render it identically and nothing needs migrating
    // (contract D24).
    let color = Some(icon_color);
    with_db(true, |db| {
        db.create_playlist_folder(name, Some("preset"), preset, color)
    })
    .map(|f| FolderFull {
        id: f.id,
        name: f.name,
        icon_type: f.icon_type,
        icon_preset: f.icon_preset,
        icon_color: f.icon_color,
        custom_image_path: f.custom_image_path,
        is_hidden: f.is_hidden,
    })
}

/// Update a folder (name, icon type/preset, colour, custom image, hidden).
///
/// `custom_image_path` is `Some(Some(p))` to set, `Some(None)` to clear and
/// `None` to leave unchanged — the DB signature, kept rather than flattened so
/// "clear the image" and "do not touch the image" stay distinguishable.
///
/// Returns whether the write landed, so the folder editor can hold its panel
/// open and toast on a failure instead of closing over a silent no-op (D22 /
/// D23). The reference has no such signal and logs at most a warning.
#[allow(clippy::too_many_arguments)]
pub fn update_folder_full(
    id: &str,
    name: &str,
    icon_type: &str,
    icon_preset: &str,
    icon_color: &str,
    custom_image_path: Option<Option<&str>>,
    is_hidden: bool,
) -> bool {
    // `""` -> `Some("")`, exactly as in `create_folder_full` above and for the
    // same reason: on UPDATE the DB reads `None` as "leave unchanged"
    // (database.rs:3925), so picking the Accent swatch on an EXISTING folder
    // did nothing at all (contract D24).
    let color = Some(icon_color);
    with_db(true, |db| {
        db.update_playlist_folder(
            id,
            Some(name),
            Some(icon_type),
            Some(icon_preset),
            color,
            custom_image_path,
            Some(is_hidden),
        )
    })
    .is_some()
}

/// Set a folder's hidden flag, leaving every other field unchanged.
pub fn set_folder_hidden(id: &str, hidden: bool) {
    with_db(true, |db| {
        db.update_playlist_folder(id, None, None, None, None, None, Some(hidden))
    });
}

/// Delete a folder.
///
/// The shared `playlist_folders` FK is `ON DELETE SET NULL`, but the app's
/// connections keep the `foreign_keys` pragma OFF, so the LOCAL members'
/// `folder_id` has to be nulled explicitly — the Qobuz side is handled inside
/// `delete_playlist_folder`. Dropping that second call leaves local playlists
/// pointing at a folder that no longer exists.
///
/// Returns whether the FOLDER ROW itself was deleted (D23's failure toast).
/// The `clear_folder` sweep is best-effort: a folder row that is gone with a
/// stale `folder_id` left behind is already handled downstream — every reader
/// filters `folder_id` through the live folder id set (§5.18) — whereas a
/// folder row that survives is the failure the user has to be told about.
pub fn delete_folder(id: &str) -> bool {
    let deleted = with_db(true, |db| db.delete_playlist_folder(id)).is_some();
    with_db(true, |db| {
        Ok(db.with_connection(|conn| qbz_library::local_playlists::clear_folder(conn, id)))
    });
    deleted
}

/// Move a playlist into `folder_id`, or to root when `None`.
pub fn move_playlist(playlist_id: u64, folder_id: Option<&str>) {
    with_db(true, |db| {
        db.move_playlist_to_folder(playlist_id, folder_id)
    });
}

/// Set a playlist's favorite flag.
pub fn set_favorite(playlist_id: u64, favorite: bool) {
    with_db(true, |db| db.set_playlist_favorite(playlist_id, favorite));
}

/// Set a playlist's hidden flag.
pub fn set_hidden(playlist_id: u64, hidden: bool) {
    with_db(true, |db| db.set_playlist_hidden(playlist_id, hidden));
}

/// Persist a custom playlist order (the custom-sort positions).
pub fn reorder_playlists(playlist_ids: &[u64]) {
    with_db(true, |db| db.reorder_playlists(playlist_ids));
}
