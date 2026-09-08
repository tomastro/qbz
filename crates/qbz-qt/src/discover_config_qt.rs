//! Discover section configurator — Slint-free port of the mutation half of
//! `crates/qbz/src/discover_prefs.rs` (`push_config_rows` /
//! `on_toggle` / `on_move` / `on_reset`).
//!
//! The gear in the Discover toolbar opens the per-tab "Customize" modal:
//! show/hide + reorder the active tab's rails. There is no Save button —
//! every toggle / move / reset persists live to the SHARED per-user
//! `discover_prefs.db` (`qbz_app::settings::discover_prefs`, the same store
//! `home_qt::load_prefs` already reads) and then re-renders the three tabs
//! from the cached candidate set, so the change lands on the next frame with
//! no network round trip.
//!
//! Transport: ONE JSON document on the bridge (`discoverConfigJson`) —
//! `{ tab, rows: [{ id, label, enabled }], enabled, total }` — parsed by
//! `qml/controls/DiscoverConfigModal.qml`.
//!
//! Rows are filtered to the sections the active tab can render. The
//! Recommendations tab has no orderable rows and uses the modal's dedicated
//! explainer, cache-window and "Refresh now" arm instead.

use cxx_qt_lib::QString;
use qbz_app::settings::discover_prefs::{
    DiscoverPrefs, DiscoverPrefsStore, DiscoverySectionId, DiscoveryTab, DEFAULT_RAIL_SIZE,
    RAIL_SIZE_PRESETS,
};
use serde::Serialize;

/// id -> label. Verbatim copy of `discover_prefs::label_for` in the Slint
/// binary crate (not reachable from here — it lives in `crates/qbz`, and the
/// headless prefs crate deliberately carries no frontend strings, ADR-006).
/// `mark` registers the msgid for the extractor; `t` does the lookup once at
/// the consumer.
fn label_for(id: DiscoverySectionId) -> &'static str {
    use DiscoverySectionId::*;
    match id {
        NewReleases => qbz_i18n::mark("New Releases"),
        PressAwards => qbz_i18n::mark("Press Accolades"),
        QobuzPlaylists => qbz_i18n::mark("Qobuz Playlists"),
        RecentlyPlayedAlbums => qbz_i18n::mark("Recently Played"),
        ContinueListening => qbz_i18n::mark("Continue Listening"),
        IdealDiscography => qbz_i18n::mark("Ideal Discography"),
        MostStreamed => qbz_i18n::mark("Most Streamed"),
        ReleaseWatch => qbz_i18n::mark("Release Watch"),
        EditorPicks => qbz_i18n::mark("Albums of the Week"),
        Qobuzissimes => qbz_i18n::mark("Qobuzissimes"),
        TopArtists => qbz_i18n::mark("Your Top Artists"),
        FavoriteAlbums => qbz_i18n::mark("Library Albums"),
        QobuzMixes => qbz_i18n::mark("Qobuz Mixes"),
        RadioStations => qbz_i18n::mark("Radio Stations"),
        SimilarAlbums => qbz_i18n::mark("More From Your Library"),
        RediscoverLibrary => qbz_i18n::mark("Rediscover Your Library"),
        EssentialsByGenre => qbz_i18n::mark("Essentials by Genre"),
        ArtistsToFollow => qbz_i18n::mark("Artists to Follow"),
        ArtistSpotlight => qbz_i18n::mark("Artist Spotlight"),
        Pinned => qbz_i18n::mark("Pinned"),
        MostPlayedAlbums => qbz_i18n::mark("Most Played Albums"),
        RecentlyPlayedPlaylists => qbz_i18n::mark("Recently Played Playlists"),
    }
}

#[derive(Serialize)]
struct RowDoc {
    id: String,
    label: String,
    enabled: bool,
    /// The rail's item cap as an index into `RAIL_SIZE_PRESETS` — the contract
    /// with the per-row select's option order in `DiscoverConfigModal.qml`.
    #[serde(rename = "sizeIndex")]
    size_index: i32,
}

#[derive(Serialize)]
struct ConfigDoc {
    tab: String,
    rows: Vec<RowDoc>,
    enabled: i32,
    total: i32,
    /// The default items-per-rail, so the select's first entry can say
    /// "Default (N)" without QML owning the number.
    #[serde(rename = "defaultRailSize")]
    default_rail_size: i64,
}

/// `pub(crate)` because the Recommendations cache window is stored in the SAME
/// per-user prefs file as the section order, and `recommendations_qt` writes it
/// — one accessor beats two copies of the user-dir lookup drifting apart.
pub(crate) fn store() -> Option<DiscoverPrefsStore> {
    crate::sidebar_qt::user_dir().and_then(|dir| DiscoverPrefsStore::new_at(&dir).ok())
}

// ---------------------------------------------------------------------------
// Items per rail
// ---------------------------------------------------------------------------
//
// Tauri had this and Slint never got it: `HomeSettingsModal.svelte` let the
// user say how many items each carousel showed, over `homeSettingsStore.ts`.
// The modal that replaced it in the discovery-v2 rewrite
// (`DiscoverySettingsModal.svelte`) only ever modelled `{id, enabled}`, the
// Slint port took THAT modal 1:1, and so it copied the absence — which Qt then
// inherited from Slint. This is the recovery.
//
// It is GLOBAL, not per rail. Tauri had six keys but five of its rails already
// shared one, and a per-rail knob would put twenty spinners in a modal whose
// job is to be glanced at.

/// Every rail's stored cap, by section id. Absent = uncapped.
pub(crate) fn rail_sizes() -> std::collections::HashMap<String, i64> {
    store().map(|s| s.load_rail_sizes()).unwrap_or_default()
}

/// One section's size as the select's INDEX into `RAIL_SIZE_PRESETS`.
fn rail_size_index_of(sizes: &std::collections::HashMap<String, i64>, id: &str) -> i32 {
    let n = sizes.get(id).copied().unwrap_or(DEFAULT_RAIL_SIZE);
    RAIL_SIZE_PRESETS
        .iter()
        .position(|&p| p == n)
        .unwrap_or_else(|| {
            RAIL_SIZE_PRESETS
                .iter()
                .position(|&p| p == DEFAULT_RAIL_SIZE)
                .unwrap_or(0)
        }) as i32
}

/// Configurator select -> persist -> re-render the tabs -> republish the rows
/// (which carry the RESOLVED index back, so a failed write snaps the select
/// to what is actually stored rather than to what was clicked).
///
/// `republish_cached()` and not a reload: the cut happens at assembly time over
/// the candidates already in memory, so a new size lands on the next frame with
/// no network round trip — exactly like a section toggle.
pub(crate) fn set_rail_size_index(tab_key: &str, id: &str, index: i32) {
    if DiscoverySectionId::from_str(id).is_none() {
        return;
    }
    let idx = index.clamp(0, RAIL_SIZE_PRESETS.len() as i32 - 1) as usize;
    let size = RAIL_SIZE_PRESETS[idx];
    let Some(store) = store() else {
        log::warn!("[qbz-qt] discover rail size: no per-user prefs store (not logged in?)");
        publish(tab_key);
        return;
    };
    if let Err(e) = store.save_rail_size(id, size) {
        log::warn!("[qbz-qt] discover rail size: save failed: {e}");
        publish(tab_key);
        return;
    }
    log::info!("[qbz-qt] discover rail {id} size set to {size} (0 = all)");
    crate::home_qt::republish_cached();
    publish(tab_key);
}

/// The tab's ordered pref ids, narrowed to what this port renders.
fn visible_ids(prefs: &DiscoverPrefs, tab: DiscoveryTab) -> Vec<DiscoverySectionId> {
    let renderable = crate::home_qt::renderable_ids(tab);
    prefs
        .tab(tab)
        .iter()
        .map(|p| p.id)
        .filter(|id| renderable.is_empty() || renderable.iter().any(|r| r.as_str() == id.as_str()))
        .collect()
}

/// The modal payload for one tab.
pub(crate) fn rows_json(tab_key: &str) -> String {
    let Some(tab) = DiscoveryTab::from_key(tab_key) else {
        // Recommendations (or an unknown key): no configurable sections.
        return serde_json::to_string(&ConfigDoc {
            tab: tab_key.to_string(),
            rows: Vec::new(),
            enabled: 0,
            total: 0,
            default_rail_size: DEFAULT_RAIL_SIZE,
        })
        .unwrap_or_else(|_| "{}".to_string());
    };
    let prefs = crate::home_qt::load_prefs();
    let visible = visible_ids(&prefs, tab);
    // ONE store read for the whole tab, not one per row.
    let sizes = rail_sizes();
    let rows: Vec<RowDoc> = prefs
        .tab(tab)
        .iter()
        .filter(|p| visible.contains(&p.id))
        .map(|p| RowDoc {
            id: p.id.as_str().to_string(),
            label: qbz_i18n::t(label_for(p.id)),
            enabled: p.enabled,
            size_index: rail_size_index_of(&sizes, p.id.as_str()),
        })
        .collect();
    let total = rows.len() as i32;
    let enabled = rows.iter().filter(|r| r.enabled).count() as i32;
    serde_json::to_string(&ConfigDoc {
        tab: tab_key.to_string(),
        rows,
        enabled,
        total,
        default_rail_size: DEFAULT_RAIL_SIZE,
    })
    .unwrap_or_else(|_| "{}".to_string())
}

fn publish(tab_key: &str) {
    let json = rows_json(tab_key);
    crate::ui(move |mut b| {
        b.as_mut()
            .set_discover_config_json(QString::from(json.as_str()));
    });
}

/// Gear click: (re)publish the rows for the tab the modal is about to show.
pub(crate) fn open(tab_key: &str) {
    publish(tab_key);
}

/// Load -> mutate -> persist -> re-render the tabs -> re-publish the rows.
fn mutate(tab_key: &str, f: impl FnOnce(&mut DiscoverPrefs, DiscoveryTab)) {
    let Some(tab) = DiscoveryTab::from_key(tab_key) else {
        return;
    };
    let Some(store) = store() else {
        log::warn!("[qbz-qt] discover config: no per-user prefs store (not logged in?)");
        return;
    };
    let mut prefs = store.load();
    f(&mut prefs, tab);
    if let Err(e) = store.save(&prefs) {
        log::warn!("[qbz-qt] discover config: save failed: {e}");
        return;
    }
    crate::home_qt::republish_cached();
    publish(tab_key);
}

pub(crate) fn toggle_section(tab_key: &str, id: &str) {
    let Some(section) = DiscoverySectionId::from_str(id) else {
        return;
    };
    mutate(tab_key, |prefs, tab| prefs.toggle(tab, section));
}

/// Move one step through the VISIBLE order (`dir` = -1 up / +1 down).
/// `DiscoverPrefs::move_section` swaps with the adjacent PREF entry, which
/// may be a row this port filtered out — the click would then look dead. So
/// the neighbour is picked from the visible list and the two entries are
/// swapped in the full list (hidden entries keep their relative order).
pub(crate) fn move_section(tab_key: &str, id: &str, dir: i32) {
    let Some(section) = DiscoverySectionId::from_str(id) else {
        return;
    };
    if dir == 0 {
        return;
    }
    mutate(tab_key, |prefs, tab| {
        let visible = visible_ids(prefs, tab);
        let Some(vi) = visible.iter().position(|v| *v == section) else {
            return;
        };
        let target = if dir < 0 {
            if vi == 0 {
                return;
            }
            visible[vi - 1]
        } else {
            if vi + 1 >= visible.len() {
                return;
            }
            visible[vi + 1]
        };
        let list = prefs.tab_mut(tab);
        let (Some(a), Some(b)) = (
            list.iter().position(|p| p.id == section),
            list.iter().position(|p| p.id == target),
        ) else {
            return;
        };
        list.swap(a, b);
    });
}

pub(crate) fn reset_tab(tab_key: &str) {
    mutate(tab_key, |prefs, tab| prefs.reset_tab(tab));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_id_has_a_label() {
        // A missing arm would not compile; this pins the lookup itself.
        for id in DiscoverPrefs::available_ids(DiscoveryTab::Home) {
            assert!(!label_for(id).is_empty());
        }
        for id in DiscoverPrefs::available_ids(DiscoveryTab::ForYou) {
            assert!(!label_for(id).is_empty());
        }
    }

    #[test]
    fn rows_json_for_an_unknown_tab_is_empty_not_broken() {
        let doc = rows_json("recommendations");
        assert!(doc.contains("\"rows\":[]"));
        assert!(doc.contains("\"total\":0"));
    }
}
