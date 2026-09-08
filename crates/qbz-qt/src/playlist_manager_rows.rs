//! Playlist Manager, controller part 2 — the data model and every PURE
//! function over it: filters, comparators, the Qobuz+local merge, the row
//! serializers and the tree flattener.
//!
//! Port of the model half of `crates/qbz/src/playlist_manager.rs`
//! (`passes` / `sort_playlists` / `sort_entries` / `local_entries` /
//! `folder_item` / `playlist_item` / `local_playlist_item` / `build_tree` /
//! `format_duration` / `parse_color`).
//!
//! Nothing here touches the DB, the network, a global or the Qt thread — it is
//! all `(&PmData, toolbar) -> rows`, which is what makes it the only part of
//! this controller that can be unit-tested without a build. `playlist_manager_qt`
//! owns the cache, the toolbar statics and the publish hop;
//! `playlist_manager_ops` owns the mutations.
//!
//! # Two deliberate divergences from the reference, both from contract D6
//!
//! 1. **Locals live in folders.** The Qt sidebar already files local playlists
//!    inside folders (`sidebar_qt.rs:352-379`), so the reference's "locals are
//!    always root" would ship a contradiction the user can see, with no way to
//!    undo it. So: the flat grid/list EXCLUDES a filed local in folder mode
//!    exactly as it excludes a filed Qobuz playlist, a folder's tree members
//!    are *(Qobuz members) ∪ (local members)*, and both count fields fold
//!    locals in. A folder holding three locals and zero Qobuz rows must not
//!    read "0 playlists".
//! 2. **`sort_playlists` (the Qobuz-only comparator) is not ported.** Its one
//!    caller in the reference is `build_tree`'s folder-member sort, and here
//!    that member list is a merged Qobuz+local set, so it goes through
//!    [`sort_entries`] like every other list. Keeping a second comparator that
//!    nothing calls would be two definitions of one order — and running it as a
//!    pre-pass is not harmless: for `"recent"` it contributes only the
//!    `reverse()`, which `sort_entries` would then apply a second time.
//!
//! # `parse_color` is a GATE, not a converter (§5.20)
//!
//! Slint needed a real `Color`; QML's `Qt.color()` parses `#rgb` and `#rrggbb`
//! natively. What still has to happen in Rust is the VALIDITY test, so a stored
//! gradient (`linear-gradient(...)`) or CSS var (`var(--accent-primary)`) never
//! reaches QML as a colour. The published pair is `iconColor` (the string, or
//! `""`) plus `hasColor`; `""` means "use the theme accent".

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::folders_qt::FolderFull;

// ---------------------------------------------------------------------------
// The Send data model (the loader fills it, the cache holds it)
// ---------------------------------------------------------------------------

/// One Qobuz playlist merged with its local settings + stats.
#[derive(Clone, Debug, Default)]
pub(crate) struct PmPlaylist {
    pub id: u64,
    pub name: String,
    /// The REAL description, carried so a rename cannot wipe it (§5.2). The
    /// fetch already returned it, so it is free. Read by block 3's playlist
    /// editor through `playlist_manager_qt::cached_playlist_seed`, which seeds
    /// from this cache before falling back to a `get_playlist` request.
    pub description: Option<String>,
    /// Remote (Qobuz) track count.
    pub tracks_count: u32,
    /// Total playlist duration in seconds.
    pub duration: u32,
    /// Local (non-Qobuz) sidecar track count.
    pub local_count: u32,
    pub play_count: u32,
    pub is_favorite: bool,
    pub is_hidden: bool,
    /// Already filtered through the LIVE folder id set by the loader — a
    /// playlist pointing at a deleted folder arrives here as `None`.
    pub folder_id: Option<String>,
    pub position: i32,
    /// Up to four de-duplicated cover urls (same scheme as the sidebar).
    pub cover_urls: Vec<String>,
}

impl PmPlaylist {
    pub fn total_count(&self) -> u32 {
        self.tracks_count + self.local_count
    }
}

/// One LOCAL playlist (library.db entity, id `local:<uuid>`).
#[derive(Clone, Debug, Default)]
pub(crate) struct PmLocalPlaylist {
    pub id: String,
    pub name: String,
    pub offline_only: bool,
    pub track_count: u32,
    pub is_favorite: bool,
    pub is_hidden: bool,
    /// D6: locals DO carry folder membership here, via
    /// `local_playlists.folder_id`, pointing at the same folder table the
    /// Qobuz playlists use. Loader-normalised against the live folder ids.
    pub folder_id: Option<String>,
    /// §5.6: filled, unlike the reference's hardcoded `cover_count: 0`.
    pub cover_urls: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PmData {
    pub playlists: Vec<PmPlaylist>,
    pub folders: Vec<FolderFull>,
    pub locals: Vec<PmLocalPlaylist>,
}

// ---------------------------------------------------------------------------
// The wire documents (§4)
// ---------------------------------------------------------------------------

/// One entry of `QbzPlaylistManager.foldersJson` (§4.1).
///
/// `iconType` is deliberately absent: `hasCustomImage` answers the only
/// question the view asks, and it answers it correctly (D25).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderRow {
    pub id: String,
    pub name: String,
    /// Meaning depends on the surface, ON PURPOSE (§5.7): in `foldersJson` it
    /// is TOTAL membership (Qobuz + local, ignoring search and the visibility
    /// filter); in `managerJson.tree[].folder` it is the POST-FILTER member
    /// count. Do not unify them.
    pub count: i32,
    pub icon_preset: String,
    /// `""` means "use the theme accent".
    pub icon_color: String,
    pub has_color: bool,
    /// Published, NEVER filtered on here — the manager shows hidden folders at
    /// 0.6 opacity and the sidebar derives its own `visibleFolders` (D11).
    pub is_hidden: bool,
    pub has_custom_image: bool,
    /// `file://…` percent-encoded, `""` whenever `hasCustomImage` is false.
    pub custom_image_path: String,
}

/// One playlist row — what `PmGridCard` / `PmListRow` / `PmTreePlaylistRow`
/// read (§4.3). Every count line is pluralised in RUST; QML never pluralises.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaylistRow {
    /// Qobuz id AS A STRING, or `local:<uuid>`. Nothing in QML may parse one.
    pub id: String,
    pub name: String,
    pub tracks_line: String,
    /// `""` when the duration is 0; always `""` for locals.
    pub duration_line: String,
    /// `""` when `localCount == 0`; always `""` for locals.
    pub local_line: String,
    pub local_count: i32,
    pub total_count: i32,
    pub play_count: i32,
    /// `"no" | "some_local" | "all_local"`; `""` for locals.
    pub local_status: String,
    pub is_favorite: bool,
    pub is_hidden: bool,
    pub is_local: bool,
    pub offline_only: bool,
    /// `""` = root. Carries the LOCAL's real folder id too (D6).
    pub folder_id: String,
    /// 0..4 urls, de-duplicated, in order. `PlaylistCollage` resolves paths.
    pub covers: Vec<String>,
}

/// One flattened tree row (§4.2). The absent half is omitted from the JSON, so
/// QML reads `modelData.folder` / `.playlist` guarded by `kind`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TreeRow {
    /// Exactly `"folder"` or `"playlist"`.
    pub kind: String,
    pub expanded: bool,
    pub indent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<FolderRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlist: Option<PlaylistRow>,
}

// ---------------------------------------------------------------------------
// Colour + duration
// ---------------------------------------------------------------------------

/// Validity gate for a stored folder colour: `Some(hex)` only for solid `#rgb`
/// / `#rrggbb`. Gradients, CSS vars, `#rrggbbaa` and empty all return `None`
/// and the tile falls back to the accent.
pub(crate) fn parse_color(s: &str) -> Option<String> {
    let hex = s.strip_prefix('#')?;
    if !matches!(hex.len(), 3 | 6) {
        return None;
    }
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(s.to_string())
}

/// Total-playtime label, e.g. `"1 h 43 min"` or `"12 min"`. Empty at zero.
pub(crate) fn format_duration(seconds: u32) -> String {
    if seconds == 0 {
        return String::new();
    }
    let hours = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    if hours > 0 {
        qbz_i18n::t_args("{} h {} min", &[&hours.to_string(), &mins.to_string()])
    } else {
        qbz_i18n::t_args("{} min", &[&mins.to_string()])
    }
}

// ---------------------------------------------------------------------------
// Row builders
// ---------------------------------------------------------------------------

pub(crate) fn folder_item(f: &FolderFull, count: usize) -> FolderRow {
    let color = parse_color(&f.icon_color);
    // BOTH halves, like the reference: an `icon_type` of "custom" with no
    // stored path is a half-written row, and rendering it as "has an image"
    // paints a bare coloured square.
    let has_custom_image = f.icon_type == "custom" && f.custom_image_path.is_some();
    FolderRow {
        id: f.id.clone(),
        name: f.name.clone(),
        count: count as i32,
        icon_preset: f.icon_preset.clone(),
        icon_color: color.clone().unwrap_or_default(),
        has_color: color.is_some(),
        is_hidden: f.is_hidden,
        has_custom_image,
        custom_image_path: if has_custom_image {
            f.custom_image_path
                .as_deref()
                .map(crate::artwork_qt::file_url)
                .unwrap_or_default()
        } else {
            String::new()
        },
    }
}

pub(crate) fn playlist_item(p: &PmPlaylist) -> PlaylistRow {
    let local_status = if p.local_count == 0 {
        "no"
    } else if p.tracks_count == 0 {
        "all_local"
    } else {
        "some_local"
    };
    let total = p.total_count();
    PlaylistRow {
        id: p.id.to_string(),
        name: p.name.clone(),
        tracks_line: qbz_i18n::tf("{} track", "{} tracks", total as i64, &[&total.to_string()]),
        duration_line: format_duration(p.duration),
        local_line: if p.local_count > 0 {
            qbz_i18n::t_args("({} local)", &[&p.local_count.to_string()])
        } else {
            String::new()
        },
        local_count: p.local_count as i32,
        total_count: total as i32,
        play_count: p.play_count as i32,
        local_status: local_status.to_string(),
        is_favorite: p.is_favorite,
        is_hidden: p.is_hidden,
        is_local: false,
        offline_only: false,
        folder_id: p.folder_id.clone().unwrap_or_default(),
        covers: p.cover_urls.clone(),
    }
}

pub(crate) fn local_playlist_item(p: &PmLocalPlaylist) -> PlaylistRow {
    PlaylistRow {
        id: p.id.clone(),
        name: p.name.clone(),
        tracks_line: qbz_i18n::tf(
            "{} track",
            "{} tracks",
            p.track_count as i64,
            &[&p.track_count.to_string()],
        ),
        // A local playlist has no Qobuz duration and no sidecar split, so both
        // secondary lines are empty by construction — not "not implemented".
        duration_line: String::new(),
        local_line: String::new(),
        local_count: 0,
        total_count: p.track_count as i32,
        play_count: 0,
        local_status: String::new(),
        is_favorite: p.is_favorite,
        is_hidden: p.is_hidden,
        is_local: true,
        offline_only: p.offline_only,
        // D6: the local's REAL folder id, not "".
        folder_id: p.folder_id.clone().unwrap_or_default(),
        covers: p.cover_urls.clone(),
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Whether a Qobuz playlist passes search + folder + visibility.
///
/// `query` must already be trimmed and lowercased.
pub(crate) fn passes(
    p: &PmPlaylist,
    query: &str,
    filter: &str,
    folder_mode: bool,
    view_mode: &str,
) -> bool {
    if !query.is_empty() && !p.name.to_lowercase().contains(query) {
        return false;
    }
    // In folder mode (non-tree) the grid/list shows ONLY root playlists —
    // folders own their members, and entering one is the tree's job.
    if folder_mode && view_mode != "tree" && p.folder_id.is_some() {
        return false;
    }
    match filter {
        "visible" => !p.is_hidden,
        "hidden" => p.is_hidden,
        _ => true,
    }
}

/// The LOCAL playlists that pass the same three filters, name-sorted.
///
/// D6.1: the folder clause is the PARALLEL of `passes`'s, applied to the local
/// vector — never one filter over an already-merged set, which would let the
/// offline filter (Qobuz-only) reach the locals.
pub(crate) fn local_entries<'a>(
    data: &'a PmData,
    query: &str,
    filter: &str,
    folder_mode: bool,
    view_mode: &str,
) -> Vec<&'a PmLocalPlaylist> {
    let mut locals: Vec<&PmLocalPlaylist> = data
        .locals
        .iter()
        .filter(|p| query.is_empty() || p.name.to_lowercase().contains(query))
        .filter(|p| !(folder_mode && view_mode != "tree" && p.folder_id.is_some()))
        .filter(|p| match filter {
            "visible" => !p.is_hidden,
            "hidden" => p.is_hidden,
            _ => true,
        })
        .collect();
    locals.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    locals
}

// ---------------------------------------------------------------------------
// The merged display set
// ---------------------------------------------------------------------------

/// Display-stage union of a Qobuz playlist and a LOCAL one, so locals
/// INTERLEAVE into the active sort instead of being appended after the Qobuz
/// block.
///
/// Sort keys a local does not have, and what it sorts as (all reference
/// behaviour, §5.17):
/// * `playcount` — 0, i.e. LAST under the descending play-count sort;
/// * `custom` — `i64::MAX`, i.e. after the whole positioned set;
/// * `recent` — no comparator runs at all, so it keeps its position after the
///   API-ordered Qobuz block.
///
/// Ties keep pre-sort order (the sorts are stable): API order among Qobuz rows,
/// name order among locals ([`local_entries`] name-sorts before the union).
pub(crate) enum PmEntry<'a> {
    Qobuz(&'a PmPlaylist),
    Local(&'a PmLocalPlaylist),
}

impl PmEntry<'_> {
    fn name_lower(&self) -> String {
        match self {
            Self::Qobuz(p) => p.name.to_lowercase(),
            Self::Local(p) => p.name.to_lowercase(),
        }
    }

    fn total_count(&self) -> u32 {
        match self {
            Self::Qobuz(p) => p.total_count(),
            Self::Local(p) => p.track_count,
        }
    }

    fn play_count(&self) -> u32 {
        match self {
            Self::Qobuz(p) => p.play_count,
            Self::Local(_) => 0,
        }
    }

    fn position(&self) -> i64 {
        match self {
            Self::Qobuz(p) => p.position as i64,
            Self::Local(_) => i64::MAX,
        }
    }

    pub(crate) fn item(&self) -> PlaylistRow {
        match self {
            Self::Qobuz(p) => playlist_item(p),
            Self::Local(p) => local_playlist_item(p),
        }
    }
}

/// Order the merged display set.
///
/// The direction is NOT folded into the comparator — it is a full `reverse()`
/// of the sorted vector, which is what makes descending `"recent"` mean
/// oldest-first (that option runs no comparator at all: the API order IS
/// newest-first). Looks like a bug, is not (§5.17).
pub(crate) fn sort_entries(list: &mut [PmEntry], sort: &str, asc: bool) {
    match sort {
        "name" => list.sort_by_key(|e| e.name_lower()),
        "playcount" => list.sort_by(|a, b| b.play_count().cmp(&a.play_count())),
        "tracks" => list.sort_by(|a, b| b.total_count().cmp(&a.total_count())),
        "custom" => list.sort_by_key(|e| e.position()),
        // "recent" — Qobuz keeps API order, locals stay after it.
        _ => {}
    }
    if !asc {
        list.reverse();
    }
}

// ---------------------------------------------------------------------------
// The two published collections
// ---------------------------------------------------------------------------

/// TOTAL membership per folder — Qobuz members plus LOCAL members — ignoring
/// search and the visibility filter. This is what `foldersJson[].count` shows
/// on the cards and chips (§5.7 / D6.3).
pub(crate) fn folder_counts(data: &PmData) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in &data.playlists {
        if let Some(fid) = &p.folder_id {
            *counts.entry(fid.clone()).or_insert(0) += 1;
        }
    }
    for p in &data.locals {
        if let Some(fid) = &p.folder_id {
            *counts.entry(fid.clone()).or_insert(0) += 1;
        }
    }
    counts
}

/// `foldersJson` from a WARM cache: every folder, hidden ones included, with
/// total-membership counts.
pub(crate) fn folder_rows(data: &PmData) -> Vec<FolderRow> {
    let counts = folder_counts(data);
    data.folders
        .iter()
        .map(|f| folder_item(f, counts.get(&f.id).copied().unwrap_or(0)))
        .collect()
}

/// The filtered + sorted flat set that feeds the grid and list bodies — and,
/// because it IS the visible order, the vector `reorder_step` renumbers.
///
/// Published unconditionally, tree mode included: in tree mode `passes` skips
/// its folder clause, so this holds every matching row (folder members
/// included) and a view-mode switch needs no rebuild.
///
/// `offline` reduces the Qobuz side to the MIXED playlists (>= 1 local sidecar
/// row). The offline SNAPSHOT subsystem is absent from this port (D30), so
/// `offline_available` is not a term here. Locals are never touched by it —
/// they are local.
#[allow(clippy::too_many_arguments)]
pub(crate) fn visible_playlist_rows(
    data: &PmData,
    query: &str,
    filter: &str,
    sort: &str,
    asc: bool,
    folder_mode: bool,
    view_mode: &str,
    offline: bool,
) -> Vec<PlaylistRow> {
    let filtered: Vec<&PmPlaylist> = data
        .playlists
        .iter()
        .filter(|p| !offline || p.local_count > 0)
        .filter(|p| passes(p, query, filter, folder_mode, view_mode))
        .collect();
    let mut entries: Vec<PmEntry> = filtered.into_iter().map(PmEntry::Qobuz).collect();
    entries.extend(
        local_entries(data, query, filter, folder_mode, view_mode)
            .into_iter()
            .map(PmEntry::Local),
    );
    sort_entries(&mut entries, sort, asc);
    entries.iter().map(|e| e.item()).collect()
}

/// The filtered + sorted members of one opened folder in grid/list mode.
///
/// Build the same merged Qobuz/local set as the flat surface first, then
/// constrain it by the already-normalised folder id carried on every wire
/// row. This keeps search, visibility, offline and ordering semantics exactly
/// aligned with the root view while allowing folder cards to be genuine
/// navigation targets instead of aliases for Edit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn visible_folder_playlist_rows(
    data: &PmData,
    folder_id: &str,
    query: &str,
    filter: &str,
    sort: &str,
    asc: bool,
    offline: bool,
) -> Vec<PlaylistRow> {
    visible_playlist_rows(data, query, filter, sort, asc, false, "grid", offline)
        .into_iter()
        .filter(|row| row.folder_id == folder_id)
        .collect()
}

/// Flatten folders + their (expanded) members + the root run into the tree.
///
/// `expanded` is passed IN rather than read from a global so this stays pure:
/// the first-open auto-expand latch lives in `playlist_manager_qt`, which
/// primes the set before calling here.
///
/// While searching — and offline, where the mixed-only filter can empty a
/// folder — a folder with nothing matching in EITHER set is dropped entirely,
/// and every surviving folder is force-expanded so the matches inside are
/// visible (§5.8). That is the opposite of the grid/list, where the folder
/// cards never react to the query.
pub(crate) fn build_tree(
    data: &PmData,
    query: &str,
    filter: &str,
    sort: &str,
    asc: bool,
    offline: bool,
    expanded: &HashSet<String>,
) -> Vec<TreeRow> {
    let searching = !query.is_empty();

    let matches = |p: &PmPlaylist| -> bool {
        if offline && p.local_count == 0 {
            return false;
        }
        if searching && !p.name.to_lowercase().contains(query) {
            return false;
        }
        match filter {
            "visible" => !p.is_hidden,
            "hidden" => p.is_hidden,
            _ => true,
        }
    };
    let local_matches = |p: &PmLocalPlaylist| -> bool {
        if searching && !p.name.to_lowercase().contains(query) {
            return false;
        }
        match filter {
            "visible" => !p.is_hidden,
            "hidden" => p.is_hidden,
            _ => true,
        }
    };

    let mut rows: Vec<TreeRow> = Vec::new();
    for f in &data.folders {
        // D6.2: a folder's members are (Qobuz members) ∪ (local members).
        let mut members: Vec<PmEntry> = data
            .playlists
            .iter()
            .filter(|p| p.folder_id.as_deref() == Some(f.id.as_str()))
            .filter(|p| matches(p))
            .map(PmEntry::Qobuz)
            .collect();
        let mut local_members: Vec<&PmLocalPlaylist> = data
            .locals
            .iter()
            .filter(|p| p.folder_id.as_deref() == Some(f.id.as_str()))
            .filter(|p| local_matches(p))
            .collect();
        local_members.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        members.extend(local_members.into_iter().map(PmEntry::Local));

        if (searching || offline) && members.is_empty() {
            continue;
        }
        sort_entries(&mut members, sort, asc);
        let is_exp = searching || expanded.contains(&f.id);
        rows.push(TreeRow {
            kind: "folder".into(),
            expanded: is_exp,
            indent: false,
            // POST-FILTER member count here, unlike `foldersJson` (§5.7).
            folder: Some(folder_item(f, members.len())),
            playlist: None,
        });
        if is_exp {
            for e in &members {
                rows.push(TreeRow {
                    kind: "playlist".into(),
                    expanded: false,
                    indent: true,
                    folder: None,
                    playlist: Some(e.item()),
                });
            }
        }
    }

    // The root run: unfiled Qobuz playlists and unfiled LOCALS, interleaved
    // into the same sort. A filed local is NOT here — it is under its folder.
    let mut entries: Vec<PmEntry> = data
        .playlists
        .iter()
        .filter(|p| p.folder_id.is_none())
        .filter(|p| matches(p))
        .map(PmEntry::Qobuz)
        .collect();
    let mut root_locals: Vec<&PmLocalPlaylist> = data
        .locals
        .iter()
        .filter(|p| p.folder_id.is_none())
        .filter(|p| local_matches(p))
        .collect();
    root_locals.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries.extend(root_locals.into_iter().map(PmEntry::Local));
    sort_entries(&mut entries, sort, asc);
    for e in &entries {
        rows.push(TreeRow {
            kind: "playlist".into(),
            expanded: false,
            indent: false,
            folder: None,
            playlist: Some(e.item()),
        });
    }
    rows
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, name: &str) -> FolderFull {
        FolderFull {
            id: id.into(),
            name: name.into(),
            icon_type: "preset".into(),
            icon_preset: "folder".into(),
            icon_color: String::new(),
            custom_image_path: None,
            is_hidden: false,
        }
    }

    fn pl(id: u64, name: &str) -> PmPlaylist {
        PmPlaylist {
            id,
            name: name.into(),
            tracks_count: 10,
            ..Default::default()
        }
    }

    fn local(id: &str, name: &str) -> PmLocalPlaylist {
        PmLocalPlaylist {
            id: format!("local:{id}"),
            name: name.into(),
            track_count: 5,
            ..Default::default()
        }
    }

    fn ids(rows: &[PlaylistRow]) -> Vec<String> {
        rows.iter().map(|r| r.id.clone()).collect()
    }

    // --- parse_color: the validity gate ---------------------------------

    #[test]
    fn parse_color_accepts_only_solid_hex() {
        assert_eq!(parse_color("#3b82f6").as_deref(), Some("#3b82f6"));
        assert_eq!(parse_color("#abc").as_deref(), Some("#abc"));
        // Everything the reference rejects, so the tile falls back to accent.
        assert!(parse_color("").is_none());
        assert!(parse_color("#3b82f6ff").is_none());
        assert!(parse_color("var(--accent-primary)").is_none());
        assert!(parse_color("linear-gradient(90deg, #fff, #000)").is_none());
        assert!(parse_color("#zzz").is_none());
        assert!(parse_color("3b82f6").is_none());
    }

    #[test]
    fn folder_item_publishes_the_colour_pair_and_suppresses_a_half_written_image() {
        let mut f = folder("f1", "Jazz");
        f.icon_color = "linear-gradient(#fff,#000)".into();
        let row = folder_item(&f, 3);
        assert_eq!(row.icon_color, "");
        assert!(!row.has_color);
        assert_eq!(row.count, 3);

        // icon_type says custom but no path was stored — NOT an image.
        f.icon_type = "custom".into();
        f.custom_image_path = None;
        assert!(!folder_item(&f, 0).has_custom_image);
        assert_eq!(folder_item(&f, 0).custom_image_path, "");

        f.custom_image_path = Some("/tmp/a b#1.png".into());
        let row = folder_item(&f, 0);
        assert!(row.has_custom_image);
        assert_eq!(row.custom_image_path, "file:///tmp/a b%231.png");
    }

    // --- duration --------------------------------------------------------

    #[test]
    fn format_duration_is_empty_at_zero_and_drops_hours_under_one() {
        assert_eq!(format_duration(0), "");
        assert_eq!(format_duration(12 * 60), "12 min");
        assert_eq!(format_duration(3600 + 43 * 60), "1 h 43 min");
    }

    // --- comparators -----------------------------------------------------

    #[test]
    fn sort_entries_interleaves_locals_by_name() {
        let data = PmData {
            playlists: vec![pl(1, "Beta"), pl(2, "Delta")],
            locals: vec![local("a", "Alpha"), local("c", "Charlie")],
            folders: vec![],
        };
        let rows = visible_playlist_rows(&data, "", "all", "name", true, false, "grid", false);
        assert_eq!(
            ids(&rows),
            vec!["local:a", "1", "local:c", "2"],
            "locals must interleave, not be appended"
        );
    }

    #[test]
    fn descending_is_a_reverse_not_a_flipped_comparator() {
        let data = PmData {
            playlists: vec![pl(1, "Beta"), pl(2, "Alpha")],
            locals: vec![],
            folders: vec![],
        };
        let asc = visible_playlist_rows(&data, "", "all", "name", true, false, "grid", false);
        let desc = visible_playlist_rows(&data, "", "all", "name", false, false, "grid", false);
        assert_eq!(ids(&asc), vec!["2", "1"]);
        assert_eq!(ids(&desc), vec!["1", "2"]);
    }

    #[test]
    fn recent_runs_no_comparator_so_descending_means_oldest_first() {
        // API order IS newest-first; "recent" must preserve it verbatim.
        let data = PmData {
            playlists: vec![pl(3, "Zulu"), pl(1, "Alpha"), pl(2, "Mike")],
            locals: vec![],
            folders: vec![],
        };
        let asc = visible_playlist_rows(&data, "", "all", "recent", true, false, "grid", false);
        assert_eq!(ids(&asc), vec!["3", "1", "2"]);
        let desc = visible_playlist_rows(&data, "", "all", "recent", false, false, "grid", false);
        assert_eq!(ids(&desc), vec!["2", "1", "3"]);
    }

    #[test]
    fn locals_sort_last_under_playcount_and_custom() {
        let mut a = pl(1, "Aaa");
        a.play_count = 7;
        a.position = 3;
        let mut b = pl(2, "Bbb");
        b.play_count = 0;
        b.position = 1;
        let data = PmData {
            playlists: vec![a, b],
            locals: vec![local("l", "Zzz")],
            folders: vec![],
        };
        let by_play =
            visible_playlist_rows(&data, "", "all", "playcount", true, false, "grid", false);
        assert_eq!(by_play[0].id, "1");
        assert_eq!(
            by_play.last().unwrap().id,
            "local:l",
            "a local has no play stat, so it sorts as 0 = last"
        );
        let by_custom =
            visible_playlist_rows(&data, "", "all", "custom", true, false, "grid", false);
        assert_eq!(ids(&by_custom), vec!["2", "1", "local:l"]);
    }

    // --- filters ---------------------------------------------------------

    #[test]
    fn folder_mode_hides_filed_rows_of_both_kinds_from_the_flat_body() {
        let mut filed = pl(1, "Filed");
        filed.folder_id = Some("f1".into());
        let mut filed_local = local("l1", "FiledLocal");
        filed_local.folder_id = Some("f1".into());
        let data = PmData {
            playlists: vec![filed, pl(2, "Root")],
            locals: vec![filed_local, local("l2", "RootLocal")],
            folders: vec![folder("f1", "Jazz")],
        };
        let folder_mode =
            visible_playlist_rows(&data, "", "all", "name", true, true, "grid", false);
        assert_eq!(
            ids(&folder_mode),
            vec!["2", "local:l2"],
            "D6.1: a filed LOCAL is excluded exactly like a filed Qobuz row"
        );
        // Flat mode shows everything, both kinds.
        let flat = visible_playlist_rows(&data, "", "all", "name", true, false, "grid", false);
        assert_eq!(ids(&flat).len(), 4);
        // Tree mode publishes the flat set unconditionally too (§4.2).
        let tree_mode = visible_playlist_rows(&data, "", "all", "name", true, true, "tree", false);
        assert_eq!(ids(&tree_mode).len(), 4);
    }

    #[test]
    fn opened_folder_contains_only_its_qobuz_and_local_members() {
        let mut qobuz_member = pl(1, "Qobuz member");
        qobuz_member.folder_id = Some("f1".into());
        let mut local_member = local("l1", "Local member");
        local_member.folder_id = Some("f1".into());
        let mut other = pl(2, "Other folder");
        other.folder_id = Some("f2".into());
        let data = PmData {
            playlists: vec![qobuz_member, other, pl(3, "Root")],
            locals: vec![local_member, local("l2", "Root local")],
            folders: vec![folder("f1", "One"), folder("f2", "Two")],
        };

        let rows = visible_folder_playlist_rows(&data, "f1", "", "all", "name", true, false);
        assert_eq!(ids(&rows), vec!["local:l1", "1"]);
        assert!(rows.iter().all(|row| row.folder_id == "f1"));
    }

    #[test]
    fn visibility_filter_applies_to_locals_own_hidden_flag() {
        let mut hidden_q = pl(1, "HiddenQ");
        hidden_q.is_hidden = true;
        let mut hidden_l = local("l1", "HiddenL");
        hidden_l.is_hidden = true;
        let data = PmData {
            playlists: vec![hidden_q, pl(2, "VisibleQ")],
            locals: vec![hidden_l, local("l2", "VisibleL")],
            folders: vec![],
        };
        // Sorted by NAME ascending, so the locals interleave rather than
        // trailing the Qobuz rows: "VisibleL" < "VisibleQ" and
        // "HiddenL" < "HiddenQ" (D6 — see
        // `sort_entries_interleaves_locals_by_name`).
        let visible =
            visible_playlist_rows(&data, "", "visible", "name", true, false, "grid", false);
        assert_eq!(ids(&visible), vec!["local:l2", "2"]);
        let hidden = visible_playlist_rows(&data, "", "hidden", "name", true, false, "grid", false);
        assert_eq!(ids(&hidden), vec!["local:l1", "1"]);
    }

    #[test]
    fn offline_keeps_mixed_qobuz_rows_and_every_local() {
        let mut mixed = pl(1, "Mixed");
        mixed.local_count = 3;
        let data = PmData {
            playlists: vec![mixed, pl(2, "PureRemote")],
            locals: vec![local("l1", "Alpha")],
            folders: vec![],
        };
        let rows = visible_playlist_rows(&data, "", "all", "name", true, false, "grid", true);
        assert_eq!(
            ids(&rows),
            vec!["local:l1", "1"],
            "the offline filter is Qobuz-only; a local is local"
        );
    }

    #[test]
    fn search_is_case_insensitive_over_both_kinds() {
        let data = PmData {
            playlists: vec![pl(1, "Late Night"), pl(2, "Morning")],
            locals: vec![local("l1", "night owl")],
            folders: vec![],
        };
        let rows = visible_playlist_rows(&data, "night", "all", "name", true, false, "grid", false);
        assert_eq!(ids(&rows), vec!["1", "local:l1"]);
    }

    // --- counts ----------------------------------------------------------

    #[test]
    fn folder_counts_fold_locals_in_so_a_locals_only_folder_is_not_zero() {
        let mut filed_local = local("l1", "One");
        filed_local.folder_id = Some("f1".into());
        let mut other = local("l2", "Two");
        other.folder_id = Some("f1".into());
        let mut hidden_q = pl(1, "HiddenButCounted");
        hidden_q.folder_id = Some("f2".into());
        hidden_q.is_hidden = true;
        let data = PmData {
            playlists: vec![hidden_q],
            locals: vec![filed_local, other],
            folders: vec![folder("f1", "LocalsOnly"), folder("f2", "Mixed")],
        };
        let rows = folder_rows(&data);
        assert_eq!(rows[0].count, 2, "D6.3: a locals-only folder is not empty");
        assert_eq!(
            rows[1].count, 1,
            "total membership ignores the visibility filter"
        );
    }

    // --- the tree --------------------------------------------------------

    #[test]
    fn build_tree_files_locals_under_their_folder_and_leaves_only_unfiled_at_root() {
        let mut filed_q = pl(1, "QobuzMember");
        filed_q.folder_id = Some("f1".into());
        let mut filed_l = local("l1", "LocalMember");
        filed_l.folder_id = Some("f1".into());
        // Root names chosen so the merged name sort is unambiguous:
        // "Alpha Root" (Qobuz) < "Zulu Root" (local).
        let data = PmData {
            playlists: vec![filed_q, pl(2, "Alpha Root")],
            locals: vec![filed_l, local("l2", "Zulu Root")],
            folders: vec![folder("f1", "Jazz")],
        };
        let expanded: HashSet<String> = ["f1".to_string()].into_iter().collect();
        let rows = build_tree(&data, "", "all", "name", true, false, &expanded);
        let shape: Vec<(&str, String)> = rows
            .iter()
            .map(|r| {
                (
                    r.kind.as_str(),
                    r.folder
                        .as_ref()
                        .map(|f| f.id.clone())
                        .or_else(|| r.playlist.as_ref().map(|p| p.id.clone()))
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                ("folder", "f1".to_string()),
                ("playlist", "local:l1".to_string()),
                ("playlist", "1".to_string()),
                ("playlist", "2".to_string()),
                ("playlist", "local:l2".to_string()),
            ]
        );
        // The folder row's count is the POST-FILTER member count (§5.7).
        assert_eq!(rows[0].folder.as_ref().unwrap().count, 2);
        // Indent marks membership.
        assert!(rows[1].indent && rows[2].indent);
        assert!(!rows[3].indent && !rows[4].indent);
    }

    #[test]
    fn collapsed_folder_emits_its_header_only() {
        let mut filed = pl(1, "Member");
        filed.folder_id = Some("f1".into());
        let data = PmData {
            playlists: vec![filed],
            locals: vec![],
            folders: vec![folder("f1", "Jazz")],
        };
        let rows = build_tree(&data, "", "all", "name", true, false, &HashSet::new());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "folder");
        assert!(!rows[0].expanded);
    }

    #[test]
    fn searching_prunes_empty_folders_and_force_expands_the_survivors() {
        let mut hit = pl(1, "Night Jazz");
        hit.folder_id = Some("f1".into());
        let mut miss = pl(2, "Morning");
        miss.folder_id = Some("f2".into());
        let data = PmData {
            playlists: vec![hit, miss],
            locals: vec![],
            folders: vec![folder("f1", "Jazz"), folder("f2", "Rock")],
        };
        // Nothing expanded — search must expand anyway.
        let rows = build_tree(&data, "night", "all", "name", true, false, &HashSet::new());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].folder.as_ref().unwrap().id, "f1");
        assert!(rows[0].expanded);
        assert_eq!(rows[1].playlist.as_ref().unwrap().id, "1");
    }

    #[test]
    fn a_folder_holding_only_matching_locals_survives_the_search_prune() {
        let mut filed_l = local("l1", "Night Owl");
        filed_l.folder_id = Some("f1".into());
        let data = PmData {
            playlists: vec![],
            locals: vec![filed_l],
            folders: vec![folder("f1", "Jazz")],
        };
        let rows = build_tree(&data, "night", "all", "name", true, false, &HashSet::new());
        assert_eq!(
            rows.len(),
            2,
            "D6.3: without locals in the member set this folder is pruned away"
        );
    }

    // --- serialisation ---------------------------------------------------

    #[test]
    fn tree_row_omits_the_absent_half_and_camel_cases_its_keys() {
        let row = TreeRow {
            kind: "playlist".into(),
            expanded: false,
            indent: true,
            folder: None,
            playlist: Some(local_playlist_item(&local("l1", "Alpha"))),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(!json.contains("\"folder\""));
        assert!(json.contains("\"isLocal\":true"));
        assert!(json.contains("\"tracksLine\""));
        assert!(json.contains("\"offlineOnly\""));
    }

    #[test]
    fn local_rows_carry_covers_the_folder_id_and_no_qobuz_only_lines() {
        let mut p = local("l1", "Alpha");
        p.folder_id = Some("f1".into());
        p.cover_urls = vec!["https://a/1.jpg".into()];
        p.offline_only = true;
        let row = local_playlist_item(&p);
        assert_eq!(row.folder_id, "f1", "D6: the local's REAL folder id");
        assert_eq!(row.covers.len(), 1, "§5.6: locals get covers");
        assert_eq!(row.duration_line, "");
        assert_eq!(row.local_line, "");
        assert_eq!(row.local_status, "");
        assert_eq!(row.play_count, 0);
        assert!(row.offline_only);
        assert_eq!(row.total_count, 5);
    }

    #[test]
    fn qobuz_row_local_status_has_three_arms() {
        let mut p = pl(1, "A");
        assert_eq!(playlist_item(&p).local_status, "no");
        p.local_count = 3;
        assert_eq!(playlist_item(&p).local_status, "some_local");
        assert_eq!(playlist_item(&p).total_count, 13);
        p.tracks_count = 0;
        assert_eq!(playlist_item(&p).local_status, "all_local");
        assert_eq!(playlist_item(&p).local_line, "(3 local)");
    }
}
