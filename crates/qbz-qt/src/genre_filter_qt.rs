//! Filter-by-genre controller — Slint-free port of
//! `crates/qbz/src/genre_filter.rs`.
//!
//! Owns the parent-genre list, the lazily loaded 3-level child tree, and the
//! genre SELECTION. The selection is **per context** ("discover" for the
//! three Discover tabs, "library-all" for the Library > All mixed feed), so
//! the two surfaces filter independently; the shared popup edits whichever
//! context was passed to `open()`. Persisted to
//! `<data-dir>/qbz/genre_filter.json` when "Remember selection" is on —
//! the SAME path and the SAME JSON shape the Slint app uses, so a profile is
//! interchangeable between the two frontends (no new store invented).
//!
//! Transport: ONE JSON document on the bridge (`genreFilterJson`), parsed by
//! `qml/controls/GenreFilterPopup.qml`. It carries the popup model (chips /
//! tree / flags) AND, per context, the selected genre NAMES (+ descendants)
//! so a client-side consumer (Library > All) can filter in QML JS without a
//! second round trip.
//!
//! Consumers:
//! - "discover"    -> `genre_ids()` feeds `get_discover_index(genre_ids)`
//!                    (`home_qt::load_home`); a change re-fetches the index.
//! - "library-all" -> `names.library-all` feeds the QML feed derive
//!                    (`library_all.rs::derive`'s genre arm, 1:1).
//!
//! Qt adaptation vs the Slint controller:
//! - The parent list loads LAZILY (first popup open) instead of at shell
//!   entry, avoiding an eager catalog request. The persisted
//!   SELECTION is read eagerly (no network), so a remembered filter applies
//!   to the very first discover fetch exactly like Slint.
//! - The "favorites" context does not exist here: the Slint FavoritesView
//!   draws its genre button only on the Library "All" tab
//!   (FavoritesView.slint:864, `context: "library-all"`), and so does this
//!   frontend. The per-context store can support another context if one lands.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use cxx_qt_lib::QString;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct GenreItem {
    id: u64,
    name: String,
}

/// On-disk shape — IDENTICAL to the Slint `Persisted` (same file), so the
/// two frontends read each other's selection.
#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    /// Per-context selections ("discover" / "library-all" / ...).
    #[serde(default)]
    contexts: HashMap<String, Vec<u64>>,
    /// Legacy single-list selection — migrated into the "discover" context.
    #[serde(default)]
    selected: Vec<u64>,
    #[serde(default = "default_true")]
    remember: bool,
}

fn default_true() -> bool {
    true
}

struct State {
    /// The persisted selection has been read (one-shot, no network).
    hydrated: bool,
    parents: Vec<GenreItem>,
    /// Lazily loaded children, keyed by parent id (levels 2 and 3).
    children: HashMap<u64, Vec<GenreItem>>,
    /// Selected genre ids per context.
    selected: HashMap<String, Vec<u64>>,
    /// The context the popup is currently editing.
    current: String,
    expanded: HashSet<u64>,
    search: String,
    remember: bool,
    advanced: bool,
    loading: bool,
}

impl State {
    fn cur_mut(&mut self) -> &mut Vec<u64> {
        let key = self.current.clone();
        self.selected.entry(key).or_default()
    }
    fn is_selected(&self, id: u64) -> bool {
        self.selected
            .get(&self.current)
            .map(|v| v.contains(&id))
            .unwrap_or(false)
    }
    fn count_for(&self, ctx: &str) -> usize {
        self.selected.get(ctx).map(|v| v.len()).unwrap_or(0)
    }
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        hydrated: false,
        parents: Vec::new(),
        children: HashMap::new(),
        selected: HashMap::new(),
        current: "discover".to_string(),
        expanded: HashSet::new(),
        search: String::new(),
        remember: true,
        advanced: false,
        loading: false,
    })
});

/// The contexts this port draws a genre button for. Both are published in
/// `counts` / `names` so each button shows ITS OWN badge regardless of which
/// context the popup last edited.
const CONTEXTS: [&str; 2] = ["discover", "library-all"];

// ---------------------------------------------------------------------------
// Persistence (same file + shape as the Slint controller)
// ---------------------------------------------------------------------------

fn store_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("qbz").join("genre_filter.json"))
}

fn load_persisted() -> Persisted {
    let Some(path) = store_path() else {
        return Persisted::default();
    };
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Persisted::default(),
    }
}

fn save_persisted(contexts: &HashMap<String, Vec<u64>>, remember: bool) {
    let Some(path) = store_path() else {
        return;
    };
    if !remember {
        // Remember off — drop any persisted selection (1:1 Slint).
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = Persisted {
        contexts: contexts.clone(),
        selected: Vec::new(),
        remember,
    };
    if let Ok(json) = serde_json::to_vec_pretty(&data) {
        let _ = std::fs::write(&path, json);
    }
}

/// Read the persisted selection into `s` exactly once. Network-free, so it
/// is safe to call from the first `load_home` — a remembered filter applies
/// to the very first discover fetch.
fn hydrate(s: &mut State) {
    if s.hydrated {
        return;
    }
    s.hydrated = true;
    let persisted = load_persisted();
    let mut contexts = persisted.contexts;
    // Migrate a legacy flat selection into the discover context.
    if contexts.is_empty() && !persisted.selected.is_empty() {
        contexts.insert("discover".to_string(), persisted.selected);
    }
    s.selected = contexts;
    s.remember = persisted.remember;
}

// ---------------------------------------------------------------------------
// Read API (consumers)
// ---------------------------------------------------------------------------

/// The RAW genre selection for `ctx` (no expansion, no ancestor mapping):
/// the exact ids the user toggled, parent or sub-genre. This is what goes to
/// /discover/* in `genre_ids` — Qobuz honors sub-genre ids server-side
/// (1:1 Tauri/Slint; narrowing to the ancestor silently widened the filter).
pub fn selected_ids_for(ctx: &str) -> Vec<u64> {
    let Ok(mut s) = STATE.lock() else {
        return Vec::new();
    };
    hydrate(&mut s);
    s.selected.get(ctx).cloned().unwrap_or_default()
}

/// The discover selection as the `Option<Vec<u64>>` the discover endpoints
/// take (None = no filter) — `home_qt::load_home`'s parameter.
pub fn discover_genre_ids() -> Option<Vec<u64>> {
    let ids = selected_ids_for("discover");
    (!ids.is_empty()).then_some(ids)
}

fn collect_descendants(children: &HashMap<u64, Vec<GenreItem>>, id: u64, out: &mut HashSet<u64>) {
    if let Some(kids) = children.get(&id) {
        for kid in kids {
            if out.insert(kid.id) {
                collect_descendants(children, kid.id, out);
            }
        }
    }
}

/// Selected genre NAMES (+ descendant names) for `ctx` — the client-side
/// album/track genre filter used by Library > All.
fn selected_names(s: &State, ctx: &str) -> Vec<String> {
    let mut ids: HashSet<u64> = HashSet::new();
    if let Some(sel) = s.selected.get(ctx) {
        for id in sel {
            ids.insert(*id);
            collect_descendants(&s.children, *id, &mut ids);
        }
    }
    let mut names: Vec<String> = Vec::new();
    for id in ids {
        if let Some(g) = s.parents.iter().find(|g| g.id == id) {
            names.push(g.name.clone());
        } else if let Some(g) = s.children.values().flatten().find(|g| g.id == id) {
            names.push(g.name.clone());
        }
    }
    names
}

// ---------------------------------------------------------------------------
// The published document
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ChipDoc {
    id: String,
    name: String,
    selected: bool,
}

#[derive(Serialize)]
struct TreeRowDoc {
    id: String,
    name: String,
    level: i32,
    selected: bool,
    expanded: bool,
    #[serde(rename = "hasChildren")]
    has_children: bool,
    count: i32,
}

#[derive(Serialize)]
struct FilterDoc {
    context: String,
    advanced: bool,
    remember: bool,
    loading: bool,
    #[serde(rename = "searchQuery")]
    search_query: String,
    /// Selection size of the CURRENT context (the popup's footer + header).
    #[serde(rename = "selectedCount")]
    selected_count: i32,
    /// Per-context selection sizes — each surface's button badge.
    counts: HashMap<String, i32>,
    /// Per-context selected names (+ descendants) for client-side filters.
    names: HashMap<String, Vec<String>>,
    genres: Vec<ChipDoc>,
    tree: Vec<TreeRowDoc>,
}

fn tree_row(item: &GenreItem, level: i32, s: &State) -> TreeRowDoc {
    let loaded = s.children.get(&item.id);
    let count = loaded.map(|c| c.len()).unwrap_or(0);
    // Parents always have children; deeper levels show an expand arrow
    // optimistically until a load proves them empty (1:1 Slint).
    let has_children = if level == 0 {
        true
    } else if level == 1 {
        count > 0 || loaded.is_none()
    } else {
        false
    };
    TreeRowDoc {
        id: item.id.to_string(),
        name: item.name.clone(),
        level,
        selected: s.is_selected(item.id),
        expanded: s.expanded.contains(&item.id),
        has_children,
        count: count as i32,
    }
}

/// Flatten the genre tree into the currently-visible rows. With a search
/// query, a flat list of every loaded genre matching it (expansion ignored,
/// no chevrons); otherwise per-node expansion down three levels.
fn build_tree_rows(s: &State) -> Vec<TreeRowDoc> {
    let query = s.search.trim().to_lowercase();
    let mut rows: Vec<TreeRowDoc> = Vec::new();

    if !query.is_empty() {
        let matches = |g: &GenreItem| g.name.to_lowercase().contains(&query);
        let flat_row = |g: &GenreItem| {
            let mut row = tree_row(g, 0, s);
            row.has_children = false;
            row
        };
        for p in &s.parents {
            if matches(p) {
                rows.push(flat_row(p));
            }
        }
        for kids in s.children.values() {
            for k in kids {
                if matches(k) {
                    rows.push(flat_row(k));
                }
            }
        }
        return rows;
    }

    for parent in &s.parents {
        rows.push(tree_row(parent, 0, s));
        if !s.expanded.contains(&parent.id) {
            continue;
        }
        let Some(children) = s.children.get(&parent.id) else {
            continue;
        };
        for child in children {
            rows.push(tree_row(child, 1, s));
            if !s.expanded.contains(&child.id) {
                continue;
            }
            if let Some(grandchildren) = s.children.get(&child.id) {
                for gc in grandchildren {
                    rows.push(tree_row(gc, 2, s));
                }
            }
        }
    }
    rows
}

fn build_doc(s: &State) -> String {
    let genres: Vec<ChipDoc> = s
        .parents
        .iter()
        .map(|g| ChipDoc {
            id: g.id.to_string(),
            name: g.name.clone(),
            selected: s.is_selected(g.id),
        })
        .collect();
    let mut counts: HashMap<String, i32> = HashMap::new();
    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    for ctx in CONTEXTS {
        counts.insert(ctx.to_string(), s.count_for(ctx) as i32);
        names.insert(ctx.to_string(), selected_names(s, ctx));
    }
    let doc = FilterDoc {
        context: s.current.clone(),
        advanced: s.advanced,
        remember: s.remember,
        loading: s.loading,
        search_query: s.search.clone(),
        selected_count: s.count_for(&s.current) as i32,
        counts,
        names,
        genres,
        tree: build_tree_rows(s),
    };
    serde_json::to_string(&doc).unwrap_or_else(|_| "{}".to_string())
}

/// The document as of right now — the bridge's construction-time seed, so a
/// remembered selection colors both buttons before the first publish.
pub(crate) fn current_json() -> String {
    let Ok(mut s) = STATE.lock() else {
        return "{}".to_string();
    };
    hydrate(&mut s);
    build_doc(&s)
}

fn publish() {
    let json = current_json();
    crate::ui(move |mut b| {
        b.as_mut()
            .set_genre_filter_json(QString::from(json.as_str()));
    });
}

// ---------------------------------------------------------------------------
// Network loads (worker thread)
// ---------------------------------------------------------------------------

async fn fetch_genres(parent_id: Option<u64>) -> Vec<GenreItem> {
    // Bound, not inlined: the Arc must outlive the await, and a temporary in
    // the match scrutinee is a needless lifetime puzzle inside a future.
    let runtime = crate::app();
    match runtime.core().get_genres(parent_id).await {
        Ok(list) => list
            .into_iter()
            .map(|g| GenreItem {
                id: g.id,
                name: g.name,
            })
            .collect(),
        Err(e) => {
            log::warn!("[qbz-qt] genre filter: get_genres({parent_id:?}) failed: {e}");
            Vec::new()
        }
    }
}

/// Fetch the parent genres once. The persisted selection is NOT validated
/// against them — it may reference child genres that are not loaded yet
/// (advanced view), so validating would wrongly drop them (1:1 Slint).
async fn load_parents() {
    let mut parents = fetch_genres(None).await;
    parents.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    if let Ok(mut s) = STATE.lock() {
        s.parents = parents;
        s.loading = false;
    }
    publish();
}

/// Load one genre level (children of `parent_id`). No-op if already loaded.
async fn load_children(parent_id: u64) {
    let already = STATE
        .lock()
        .map(|s| s.children.contains_key(&parent_id))
        .unwrap_or(true);
    if already {
        return;
    }
    let kids = fetch_genres(Some(parent_id)).await;
    if let Ok(mut s) = STATE.lock() {
        s.children.insert(parent_id, kids);
    }
}

/// Children + grandchildren of `id`, so `selected_names` covers the whole
/// subtree (Library > All) and the tree shows counts.
async fn load_descendants(id: u64) {
    load_children(id).await;
    let kids: Vec<u64> = STATE
        .lock()
        .ok()
        .and_then(|s| {
            s.children
                .get(&id)
                .map(|k| k.iter().map(|c| c.id).collect())
        })
        .unwrap_or_default();
    for kid in kids {
        load_children(kid).await;
    }
}

/// Every parent's children (level 2), so the advanced tree shows child
/// counts up front. Grandchildren stay lazy.
async fn load_all_parent_children() {
    let parents: Vec<u64> = STATE
        .lock()
        .map(|s| s.parents.iter().map(|p| p.id).collect())
        .unwrap_or_default();
    for parent_id in parents {
        load_children(parent_id).await;
    }
}

// ---------------------------------------------------------------------------
// Actions (bridge invokables)
// ---------------------------------------------------------------------------

/// A selection change re-fetches the discover index; the Library > All feed
/// re-derives in QML off the republished `names`, so it needs nothing here
/// (1:1 with the Slint split: server-side for discover, client-side for
/// library-all).
fn after_selection_change(ctx: &str) {
    if ctx == "discover" {
        crate::reload_home();
    }
}

/// Warm the genre tree once a live session exists: the parent list (so the
/// popup opens instantly) plus the descendants of every persisted selection.
/// The second half matters — the Library > All filter matches on genre
/// NAMES, and a remembered SUB-genre only resolves to a name once its branch
/// is loaded, so without this a remembered selection would color the button
/// and filter nothing until the popup was opened.
///
/// Called from `home_qt::load_home`: the first point in this port that runs
/// with a session (the Slint app warms the same state at shell entry, a hook
/// this package may not edit).
pub(crate) fn warm_up() {
    let needed = {
        let Ok(mut s) = STATE.lock() else {
            return;
        };
        hydrate(&mut s);
        let need = s.parents.is_empty() && !s.loading;
        if need {
            s.loading = true;
        }
        need
    };
    if !needed {
        return;
    }
    crate::spawn(async {
        load_parents().await;
        let ids: Vec<u64> = STATE
            .lock()
            .map(|s| {
                let mut all: Vec<u64> = s.selected.values().flatten().copied().collect();
                all.sort_unstable();
                all.dedup();
                all
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return;
        }
        for id in ids {
            load_descendants(id).await;
        }
        publish();
    });
}

/// Popup opened for a surface: switch the edited context, publish, and kick
/// the one-shot parent load if it has not run yet.
pub(crate) fn open(ctx: &str) {
    let needs_parents = {
        let Ok(mut s) = STATE.lock() else {
            return;
        };
        hydrate(&mut s);
        s.current = ctx.to_string();
        s.selected.entry(ctx.to_string()).or_default();
        s.search.clear();
        let need = s.parents.is_empty() && !s.loading;
        if need {
            s.loading = true;
        }
        need
    };
    publish();
    if needs_parents {
        crate::spawn(async {
            load_parents().await;
        });
    }
}

pub(crate) fn toggle(id_str: &str) {
    let Ok(id) = id_str.parse::<u64>() else {
        return;
    };
    let (ctx, was_selected) = {
        let Ok(mut s) = STATE.lock() else {
            return;
        };
        hydrate(&mut s);
        let was = s.is_selected(id);
        {
            let sel = s.cur_mut();
            if let Some(pos) = sel.iter().position(|x| *x == id) {
                sel.remove(pos);
            } else {
                sel.push(id);
            }
        }
        let (contexts, remember) = (s.selected.clone(), s.remember);
        save_persisted(&contexts, remember);
        (s.current.clone(), was)
    };
    publish();
    // Newly selected: eager-load its descendants so the NAME filter covers
    // the sub-genres and the tree shows counts (1:1 Slint).
    if !was_selected {
        crate::spawn(async move {
            load_descendants(id).await;
            publish();
        });
    }
    after_selection_change(&ctx);
}

pub(crate) fn toggle_expand(id_str: &str) {
    let Ok(id) = id_str.parse::<u64>() else {
        return;
    };
    let (now_expanded, needs_children) = {
        let Ok(mut s) = STATE.lock() else {
            return;
        };
        let expanded = if s.expanded.contains(&id) {
            s.expanded.remove(&id);
            false
        } else {
            s.expanded.insert(id);
            true
        };
        (expanded, expanded && !s.children.contains_key(&id))
    };
    publish();
    if now_expanded && needs_children {
        crate::spawn(async move {
            load_children(id).await;
            publish();
        });
    }
}

pub(crate) fn set_search(query: &str) {
    if let Ok(mut s) = STATE.lock() {
        s.search = query.to_string();
    }
    publish();
}

pub(crate) fn clear() {
    let ctx = {
        let Ok(mut s) = STATE.lock() else {
            return;
        };
        hydrate(&mut s);
        s.cur_mut().clear();
        let (contexts, remember) = (s.selected.clone(), s.remember);
        save_persisted(&contexts, remember);
        s.current.clone()
    };
    publish();
    after_selection_change(&ctx);
}

pub(crate) fn set_remember(remember: bool) {
    {
        let Ok(mut s) = STATE.lock() else {
            return;
        };
        hydrate(&mut s);
        s.remember = remember;
        let contexts = s.selected.clone();
        save_persisted(&contexts, remember);
    }
    publish();
}

pub(crate) fn set_advanced(advanced: bool) {
    {
        let Ok(mut s) = STATE.lock() else {
            return;
        };
        s.advanced = advanced;
        if !advanced {
            s.search.clear();
        }
    }
    publish();
    // First time the advanced view opens, eager-load every parent's children
    // so the tree shows child counts (1:1 Slint).
    if advanced {
        crate::spawn(async {
            load_all_parent_children().await;
            publish();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, name: &str) -> GenreItem {
        GenreItem {
            id,
            name: name.to_string(),
        }
    }

    fn fixture() -> State {
        let mut children: HashMap<u64, Vec<GenreItem>> = HashMap::new();
        children.insert(1, vec![item(10, "Bebop"), item(11, "Cool Jazz")]);
        children.insert(10, vec![item(100, "Hard Bop")]);
        let mut selected: HashMap<String, Vec<u64>> = HashMap::new();
        selected.insert("discover".to_string(), vec![1]);
        selected.insert("library-all".to_string(), vec![11]);
        State {
            hydrated: true,
            parents: vec![item(1, "Jazz"), item(2, "Rock")],
            children,
            selected,
            current: "discover".to_string(),
            expanded: HashSet::new(),
            search: String::new(),
            remember: true,
            advanced: false,
            loading: false,
        }
    }

    #[test]
    fn names_expand_to_the_whole_subtree() {
        let s = fixture();
        let mut got = selected_names(&s, "discover");
        got.sort();
        assert_eq!(got, vec!["Bebop", "Cool Jazz", "Hard Bop", "Jazz"]);
    }

    #[test]
    fn contexts_are_independent() {
        let s = fixture();
        assert_eq!(s.count_for("discover"), 1);
        assert_eq!(s.count_for("library-all"), 1);
        let names = selected_names(&s, "library-all");
        assert_eq!(names, vec!["Cool Jazz"]);
    }

    #[test]
    fn tree_rows_honor_expansion_and_search() {
        let mut s = fixture();
        // Collapsed: parents only.
        assert_eq!(build_tree_rows(&s).len(), 2);
        // Expanded parent: + its two children.
        s.expanded.insert(1);
        let rows = build_tree_rows(&s);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].level, 1);
        assert!(rows[0].has_children);
        // Search is flat and chevron-less.
        s.search = "bop".to_string();
        let rows = build_tree_rows(&s);
        assert_eq!(rows.len(), 2); // Bebop + Hard Bop
        assert!(rows.iter().all(|r| !r.has_children));
    }

    #[test]
    fn chip_selection_follows_the_current_context() {
        let mut s = fixture();
        let doc = build_doc(&s);
        assert!(doc.contains("\"selectedCount\":1"));
        // Jazz (id 1) is selected in discover, not in library-all.
        assert!(s.is_selected(1));
        s.current = "library-all".to_string();
        assert!(!s.is_selected(1));
    }
}
