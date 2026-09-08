//! Offline Cache Manager controller — the Qt port of
//! `crates/qbz/src/offline_manager.rs`.
//!
//! Reads `index.db`, rolls the cached tracks up into artist → album → track,
//! applies the toolbar filters and publishes ONE JSON document on
//! `QbzOffline.managerJson`. Per-item actions are NOT here: they reuse
//! `offline_cache_qt::*`, which is the same split the reference makes.
//!
//! ## Two deliberate deltas from the reference, both because Qt is not Slint
//!
//! 1. **Covers are PATHS, not decoded pixels.** The Slint controller decodes
//!    each album cover to a 96px `DecodedPixels` on the worker because
//!    `slint::Image` is not `Send` and cannot be built off the UI thread. Qt
//!    has no such constraint: the document carries the file path and
//!    `RoundedImage` loads it with `sourceSize` set, so the decode happens
//!    once, in Qt's own image thread, at the size the row draws.
//! 2. **Selection lives in QML.** The reference edits `selected` in place on
//!    the Slint model and asks Rust for the checked ids. This port's
//!    convention is the opposite and predates this view — the Local Library's
//!    multi-select is `property var tracksSelected: ({})` in the view, with
//!    the ids handed down as a JSON array on the bulk call
//!    (`LocalLibraryView.qml:100,755`). Following it keeps the whole document
//!    a pure function of the DB + the filters, so a rebuild can never fight
//!    the user's checkboxes. The `selected` field is therefore absent from the
//!    published rows.
//!
//! The filters DO live here, for the reference's own reason: a rebuild runs on
//! a worker and cannot read the view's state, so the artist selection, the
//! sort and the failed-only flag are Rust-side truth and travel back out in
//! the document.

use std::collections::BTreeMap;
use std::sync::Mutex;

use qbz_offline_cache::{CachedTrackInfo, OfflineCacheStatus};
use serde::Serialize;

use crate::offline_manager_bridge as bridge;
use cxx_qt_lib::QString;

const GB: u64 = 1024 * 1024 * 1024;

/// The route id this view records. `nav_qt` needs no registration for it —
/// a route is `record()` here plus an arm in `ContentRouter.qml`.
pub const ROUTE: &str = "offlinemanager";

// ---------------------------------------------------------------------------
// Toolbar filter state (Rust-side truth — see the module header)
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Filters {
    /// "" = all artists.
    selected_artist: String,
    /// 0 alpha · 1 recent · 2 largest · 3 smallest.
    sort: i32,
    show_only_failed: bool,
}

static FILTERS: Mutex<Option<Filters>> = Mutex::new(None);

fn current_filters() -> Filters {
    FILTERS
        .lock()
        .ok()
        .and_then(|f| f.clone())
        .unwrap_or_default()
}

fn edit_filters(f: impl FnOnce(&mut Filters)) {
    if let Ok(mut guard) = FILTERS.lock() {
        let mut cur = guard.clone().unwrap_or_default();
        f(&mut cur);
        *guard = Some(cur);
    }
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ArtistRow {
    name: String,
    /// "3 albums · 41 tracks", pre-formatted (the reference composes it in
    /// Rust too — one plural form per locale, not two interpolations in QML).
    meta: String,
    selected: bool,
}

#[derive(Serialize)]
struct Row {
    /// "album" | "track".
    kind: String,
    #[serde(rename = "albumId")]
    album_id: String,
    #[serde(rename = "trackId")]
    track_id: String,
    title: String,
    subtitle: String,
    /// Album: "12 tracks · 480 MB". Track: "3:07".
    meta: String,
    /// 0 none · 2 downloading · 3 ready · 4 failed (the app-wide row-status
    /// vocabulary — `offline_cache_qt` pushes the same integers live).
    status: i32,
    /// 0.0..1.0, only meaningful while `status == 2`.
    progress: f32,
    /// Absolute path to the album cover on disk, or "" — QML prefixes
    /// `file://`. Album rows only.
    cover: String,
    /// 1-based position within its album. Track rows only.
    number: String,
}

#[derive(Serialize, Default)]
struct ManagerDoc {
    #[serde(rename = "tracksCount")]
    tracks_count: i32,
    /// "41 tracks" — the PLURAL-CORRECT string, formatted here because there
    /// is no plural seam in QML: `QbzSession.tr` takes one msgid and applies
    /// no count (`session_bridge.rs:99`). Every other plural in the port that
    /// tried to do it QML-side lost the singular form.
    #[serde(rename = "tracksText")]
    tracks_text: String,
    /// "3.4 GB".
    #[serde(rename = "sizeText")]
    size_text: String,
    /// "· of 5.0 GB" or "· Unlimited" — the reference keeps the separator
    /// inside the string so the two halves concatenate without a conditional.
    #[serde(rename = "limitText")]
    limit_text: String,
    /// 0.0..1.0 for the usage bar; 0.0 when there is no limit.
    usage: f32,
    /// The number in the GB field. 5 when unlimited (the reference's default
    /// suggestion, not a limit that is in force).
    #[serde(rename = "limitGb")]
    limit_gb: i32,
    #[serde(rename = "selectedArtist")]
    selected_artist: String,
    #[serde(rename = "sortIndex")]
    sort_index: i32,
    #[serde(rename = "showOnlyFailed")]
    show_only_failed: bool,
    artists: Vec<ArtistRow>,
    rows: Vec<Row>,
}

// ---------------------------------------------------------------------------
// Formatting (verbatim from the reference)
// ---------------------------------------------------------------------------

/// `pub(crate)`: the lyrics-cache Settings row formats its size with the same
/// function in the reference (`offline_manager::human_size`).
pub(crate) fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", b / GB as f64)
    } else if bytes >= 1024 * 1024 {
        format!("{:.0} MB", b / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn track_status_int(s: &OfflineCacheStatus) -> i32 {
    match s {
        OfflineCacheStatus::Ready => 3,
        OfflineCacheStatus::Failed => 4,
        _ => 2,
    }
}

fn fmt_duration(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn album_size(group: &[CachedTrackInfo]) -> u64 {
    group.iter().map(|t| t.file_size_bytes).sum()
}

// ---------------------------------------------------------------------------
// Publish
// ---------------------------------------------------------------------------

fn publish(doc: &ManagerDoc, loading: bool) {
    let json = serde_json::to_string(doc).unwrap_or_else(|_| "{}".into());
    bridge::ui(move |mut b| {
        b.as_mut().set_manager_json(QString::from(json.as_str()));
        b.as_mut().set_manager_loading(loading);
    });
}

/// Read the index, roll it up, publish. `pub` so the cache mutators can
/// refresh the view after their DB write (`refresh_if_open`).
pub async fn rebuild() {
    let f = current_filters();
    let (tracks, limit, cache_path, healed): (
        Vec<CachedTrackInfo>,
        Option<u64>,
        String,
        std::collections::HashMap<u64, String>,
    ) = match crate::offline_qt::get().await {
        Some(off) => {
            let limit = *off.limit_bytes.lock().await;
            let cp = off.get_cache_path();
            let tracks = {
                let guard = off.db.lock().await;
                guard
                    .as_ref()
                    .and_then(|db| db.get_all_tracks().ok())
                    .unwrap_or_default()
            };
            // HEAL, only when something needs it. Rows queued through the
            // album button before `track_cache_info` learned to stamp the
            // album carry a NULL album title, and no re-download will fix the
            // ones already on disk. library.db DOES know their album (the
            // downloader writes a `qobuz_download` row per track), so the
            // title is recovered from there by qobuz track id.
            //
            // Titles only — library.db keys albums by `"<title>|<artist>"`,
            // not by the Qobuz album id, so `album_id` stays NULL for those
            // rows and the grouping below falls to its (artist, title) key.
            // That is enough to render them correctly; it is NOT enough for
            // `remove_album` / `redownload_album`, which look up BY id — those
            // stay per-track for pre-existing rows, and the album buttons on
            // such a group act on nothing. Re-downloading the album restamps
            // it properly.
            let needs_heal = tracks
                .iter()
                .any(|t| t.album.as_deref().unwrap_or("").is_empty());
            let healed = if needs_heal {
                let guard = off.library_db.lock().await;
                guard
                    .as_ref()
                    .and_then(|db| db.get_qobuz_download_tracks().ok())
                    .map(|rows| {
                        rows.into_iter()
                            .filter_map(|r| {
                                let id = r.qobuz_track_id?;
                                let album = r.album.trim();
                                (!album.is_empty()).then(|| (id as u64, album.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                std::collections::HashMap::new()
            };
            (tracks, limit, cp, healed)
        }
        // Logged out: an empty manager, not a spinner that never ends.
        None => (Vec::new(), None, String::new(), Default::default()),
    };

    let total_size: u64 = tracks.iter().map(|t| t.file_size_bytes).sum();
    let tracks_count = tracks.len() as i32;

    // group key -> (artist, album_title, tracks) in FIRST-SEEN order. The DB
    // returns rows most-recently-accessed first, so that order IS the
    // "recent" sort — which is why the sort below leaves it alone for index 1.
    //
    // THE KEY IS NOT `album_id` ALONE. It used to be, with every row missing
    // one collapsing into a single `"__singles__"` bucket — so three complete
    // albums by three different artists rendered as ONE album header carrying
    // the first artist's name, and the artist rail credited all 34 tracks to
    // that one artist while the other two vanished from it (owner smoke,
    // 2026-08-16; reproduced against the real index). The reference has the
    // same key and the same bug (`offline_manager.rs:142`).
    //
    // The ladder, most authoritative first:
    //   1. the Qobuz album id — an album is an album, even a compilation
    //      whose tracks each name a different artist;
    //   2. otherwise (artist, album title) — two artists can ship an album
    //      with the same title, so the artist is part of the key;
    //   3. otherwise the artist alone: a per-ARTIST "Singles" bucket. Tracks
    //      with no album at all still group, but they can never again be
    //      mixed with another artist's.
    let mut album_order: Vec<String> = Vec::new();
    let mut albums: BTreeMap<String, (String, String, Vec<CachedTrackInfo>)> = BTreeMap::new();
    for t in tracks {
        let title = t
            .album
            .clone()
            .filter(|a| !a.trim().is_empty())
            .or_else(|| healed.get(&t.track_id).cloned())
            .unwrap_or_default();
        let key = match t.album_id.as_deref().filter(|id| !id.trim().is_empty()) {
            Some(id) => format!("id:{id}"),
            // \x1f (unit separator) is the join: it cannot occur in an artist
            // name or an album title, so "A\x1fB C" and "A B\x1fC" can never
            // collide the way a "-" or a "|" would.
            None if !title.is_empty() => format!("t:{}\x1f{title}", t.artist),
            None => format!("s:{}", t.artist),
        };
        if !albums.contains_key(&key) {
            album_order.push(key.clone());
        }
        // Raw literal, NOT a msgid — 1:1 with the reference
        // (`offline_manager.rs:146`), and "Singles" is absent from all eight
        // catalogs. Translating it here would add a ninth string that only
        // this port has, which is exactly the drift the shared catalog exists
        // to prevent.
        let display_title = if title.is_empty() {
            "Singles".to_string()
        } else {
            title
        };
        albums
            .entry(key)
            .or_insert_with(|| (t.artist.clone(), display_title, Vec::new()))
            .2
            .push(t);
    }

    // Artist rail: A-Z (BTreeMap order), with per-artist album + track counts.
    let mut artist_stats: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for aid in &album_order {
        let (artist, _title, group) = &albums[aid];
        let e = artist_stats.entry(artist.clone()).or_insert((0, 0));
        e.0 += 1;
        e.1 += group.len();
    }
    let artists: Vec<ArtistRow> = artist_stats
        .iter()
        .map(|(name, (albums_n, tracks_n))| ArtistRow {
            name: name.clone(),
            meta: qbz_i18n::t_args(
                "{} albums · {} tracks",
                &[&albums_n.to_string(), &tracks_n.to_string()],
            ),
            selected: *name == f.selected_artist,
        })
        .collect();

    let mut order = album_order.clone();
    match f.sort {
        0 => order.sort_by(|a, b| albums[a].1.to_lowercase().cmp(&albums[b].1.to_lowercase())),
        2 => order.sort_by(|a, b| album_size(&albums[b].2).cmp(&album_size(&albums[a].2))),
        3 => order.sort_by(|a, b| album_size(&albums[a].2).cmp(&album_size(&albums[b].2))),
        // 1 = recent — the DB's last_accessed_at DESC order, already there.
        _ => {}
    }

    let mut rows: Vec<Row> = Vec::new();
    for aid in &order {
        let (artist, title, group) = &albums[aid];
        if !f.selected_artist.is_empty() && *artist != f.selected_artist {
            continue;
        }
        let any_failed = group
            .iter()
            .any(|t| matches!(t.status, OfflineCacheStatus::Failed));
        if f.show_only_failed && !any_failed {
            continue;
        }
        let any_active = group.iter().any(|t| {
            matches!(
                t.status,
                OfflineCacheStatus::Queued | OfflineCacheStatus::Downloading
            )
        });
        let all_ready = group
            .iter()
            .all(|t| matches!(t.status, OfflineCacheStatus::Ready));
        let album_status = if any_failed {
            4
        } else if any_active {
            2
        } else if all_ready {
            3
        } else {
            0
        };
        // The FIRST track whose cover resolves — within one album only some
        // tracks carry one (per-track CMAF folders, mixed v1/v2 rows).
        let cover = group
            .iter()
            .find_map(|t| t.resolve_cover_path(&cache_path))
            .unwrap_or_default();
        // The row's albumId is the REAL Qobuz id or "" — never the synthetic
        // group key. The album buttons pass it to `remove_album` /
        // `redownload_album`, which query `album_id` in SQL; handing them
        // `t:Artist\u{1}Title` would match nothing while looking like it had.
        let real_album_id = group
            .first()
            .and_then(|t| t.album_id.clone())
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_default();
        rows.push(Row {
            kind: "album".into(),
            album_id: real_album_id.clone(),
            track_id: String::new(),
            title: title.clone(),
            subtitle: artist.clone(),
            meta: qbz_i18n::t_args(
                "{} tracks · {}",
                &[&group.len().to_string(), &human_size(album_size(group))],
            ),
            status: album_status,
            progress: 0.0,
            cover,
            number: String::new(),
        });
        for (i, t) in group.iter().enumerate() {
            if f.show_only_failed && !matches!(t.status, OfflineCacheStatus::Failed) {
                continue;
            }
            rows.push(Row {
                kind: "track".into(),
                album_id: real_album_id.clone(),
                track_id: t.track_id.to_string(),
                title: t.title.clone(),
                subtitle: t.artist.clone(),
                meta: fmt_duration(t.duration_secs),
                status: track_status_int(&t.status),
                progress: t.progress_percent as f32 / 100.0,
                cover: String::new(),
                number: (i + 1).to_string(),
            });
        }
    }

    let (limit_text, usage, limit_gb) = match limit {
        Some(l) if l > 0 => (
            qbz_i18n::t_args("· of {}", &[&human_size(l)]),
            (total_size as f32 / l as f32).clamp(0.0, 1.0),
            (l / GB).max(1) as i32,
        ),
        _ => (qbz_i18n::t("· Unlimited"), 0.0, 5),
    };

    publish(
        &ManagerDoc {
            tracks_count,
            tracks_text: qbz_i18n::tf(
                "{} track",
                "{} tracks",
                tracks_count as i64,
                &[&tracks_count.to_string()],
            ),
            size_text: human_size(total_size),
            limit_text,
            usage,
            limit_gb,
            selected_artist: f.selected_artist,
            sort_index: f.sort,
            show_only_failed: f.show_only_failed,
            artists,
            rows,
        },
        false,
    );
}

/// Settings > Offline > "Open manager": push the route, then load.
pub fn open() {
    crate::nav_qt::record(ROUTE);
    load();
}

/// Mark loading and rebuild. The view also calls this from
/// `Component.onCompleted` — nav back/forward runs no per-view load.
pub fn load() {
    bridge::ui(|mut b| b.as_mut().set_manager_loading(true));
    crate::spawn(async move { rebuild().await });
}

/// Rebuild ONLY when the manager is the route on screen.
///
/// The reference rebuilds unconditionally after every cache mutation, which is
/// free for it because the same call also refreshes its Slint model. Here the
/// mutators fire from the album page and the track rows far more often than
/// from this view, and a rebuild is a full `get_all_tracks()` — so the gate
/// keeps a download started from an album page off the SQLite file it does not
/// need to touch. Live per-row status still reaches the view either way: it
/// rides `QbzShell.trackCacheStatusChanged`, the same signal every other view
/// patches its rows from.
pub async fn refresh_if_open() {
    if crate::nav_qt::current_view() == ROUTE {
        rebuild().await;
    }
}

// ---------------------------------------------------------------------------
// Toolbar actions
// ---------------------------------------------------------------------------

pub fn select_artist(name: String) {
    edit_filters(|f| f.selected_artist = name);
    crate::spawn(async move { rebuild().await });
}

pub fn set_sort(index: i32) {
    edit_filters(|f| f.sort = index);
    crate::spawn(async move { rebuild().await });
}

pub fn toggle_failed() {
    edit_filters(|f| f.show_only_failed = !f.show_only_failed);
    crate::spawn(async move { rebuild().await });
}

/// Set the cache size limit in GB, persist it, refresh.
///
/// Clamped at 1 GB like the reference: the field is a free-text number input,
/// and a 0 would read as "unlimited" everywhere downstream while the user
/// believed they had just forbidden caching entirely.
pub fn set_limit(gb: i32) {
    crate::spawn(async move {
        let bytes = (gb.max(1) as u64) * GB;
        if let Some(off) = crate::offline_qt::get().await {
            *off.limit_bytes.lock().await = Some(bytes);
        }
        crate::offline_qt::persist_limit(bytes).await;
        rebuild().await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_matches_the_reference_thresholds() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5 MB");
        // The GB arm is the only one with a decimal — the stats bar reads
        // "3.4 GB", never "3 GB".
        assert_eq!(human_size(3 * GB + GB / 2), "3.5 GB");
    }

    /// The status integers are the app-wide row vocabulary that
    /// `offline_cache_qt::push_status` also emits. A mismatch here would show
    /// a spinner on a ready row (or nothing on a failed one) with no error
    /// anywhere — the album/track glyph is the only symptom.
    #[test]
    fn status_ints_match_the_row_vocabulary() {
        assert_eq!(track_status_int(&OfflineCacheStatus::Ready), 3);
        assert_eq!(track_status_int(&OfflineCacheStatus::Failed), 4);
        assert_eq!(track_status_int(&OfflineCacheStatus::Queued), 2);
        assert_eq!(track_status_int(&OfflineCacheStatus::Downloading), 2);
    }

    #[test]
    fn duration_is_mss_with_a_padded_seconds_field() {
        assert_eq!(fmt_duration(0), "0:00");
        assert_eq!(fmt_duration(7), "0:07");
        assert_eq!(fmt_duration(187), "3:07");
        // No hours field, exactly like the reference.
        assert_eq!(fmt_duration(3671), "61:11");
    }

    /// The filters are process-global; the actions must round-trip through
    /// them or the rebuild reads defaults and the toolbar silently resets.
    #[test]
    fn filter_edits_round_trip() {
        edit_filters(|f| {
            f.selected_artist = "Boards of Canada".into();
            f.sort = 2;
            f.show_only_failed = true;
        });
        let f = current_filters();
        assert_eq!(f.selected_artist, "Boards of Canada");
        assert_eq!(f.sort, 2);
        assert!(f.show_only_failed);
        // Leave the global as the rest of the suite expects to find it.
        edit_filters(|f| *f = Filters::default());
    }
}
