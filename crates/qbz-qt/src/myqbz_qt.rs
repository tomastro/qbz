//! MyQBZ grid controller — the Slint-free port of `crates/qbz/src/myqbz.rs`
//! (the Mixtapes and Collections index grids) plus the Create modal's state
//! (`qbz/src/main.rs:5795-5895`).
//!
//! Publishes TWO documents, one per grid: `mixtapesJson` (kind == mixtape) and
//! `collectionsJson` (collection + artist_collection). They are separate
//! properties on purpose — `nav_qt::back()` publishes only `currentView` and
//! never re-dispatches a load, so a single discriminated document would render
//! the Collections rows under the Mixtapes route after
//! mixtapes -> collections -> back (spec 02 §4.1).
//!
//! Data comes from the per-user `library.db` through
//! `qbz_mixtape::repo::list_collections`, reached with
//! `library_db_qt::with_db` + `LibraryDatabase::with_connection`. The repo
//! hydrates each collection's items, so counts and mosaic artwork are accurate
//! with no per-row lookup in QML. Every display string (eyebrow label,
//! "N albums" plural, the pre-downscaled mosaic urls) is computed here.
//!
//! Threading: every DB touch runs inside `tokio::task::spawn_blocking`
//! (`LibraryDatabase` wraps a `!Send` rusqlite `Connection`), the artwork fill
//! is ONE `download_missing` pass followed by ONE republish, and the toolbar
//! setters are pure in-memory rebuilds off the per-grid cache — no refetch.
//!
//! Offline loads filter against one availability snapshot: local items remain,
//! Plex is allowed only in induced offline mode, and Qobuz items require a
//! ready cache entry plus a valid subscription grace verdict.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{LazyLock, Mutex, MutexGuard};

use cxx_qt_lib::QString;
use qbz_models::mixtape::{
    AlbumSource, CollectionKind, ItemType, MixtapeCollection, MixtapeCollectionItem,
};
use serde::Serialize;

/// The grid-card mosaic renders at 184px (spec 01 §5.2), which is what the
/// per-cell downscale target is derived from.
const CARD_MOSAIC_SIZE: u32 = 184;

/// Which grid a navigation targets (`myqbz.rs:37`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Grid {
    Mixtapes,
    Collections,
}

impl Grid {
    /// The document's own identity, also the shell route id.
    fn id(self) -> &'static str {
        match self {
            Grid::Mixtapes => "mixtapes",
            Grid::Collections => "collections",
        }
    }
}

/// Parse the grid discriminator QML passes ("mixtapes" | "collections").
pub(crate) fn grid_from_str(s: &str) -> Grid {
    match s {
        "collections" => Grid::Collections,
        _ => Grid::Mixtapes,
    }
}

/// Parse the create modal's `kind` string into a `CollectionKind`.
/// `artist_collection` is NOT creatable from this path (Discography Builder
/// only); unknown values default to `Mixtape` (`myqbz.rs:91-96`).
pub(crate) fn kind_from_str(s: &str) -> CollectionKind {
    match s {
        "collection" => CollectionKind::Collection,
        "artist_collection" => CollectionKind::ArtistCollection,
        _ => CollectionKind::Mixtape,
    }
}

// ---------------------------------------------------------------------------
// Offline availability
// ---------------------------------------------------------------------------

/// One batch snapshot used by both the My QBZ grids and detail view. Building
/// it once prevents a database/index read for every card.
pub(crate) struct OfflineAvailability {
    cached_track_ids: HashSet<u64>,
    cached_album_ids: HashSet<String>,
    plex_local_rows: HashSet<i64>,
    qobuz_allowed: bool,
    plex_allowed: bool,
}

impl OfflineAvailability {
    pub(crate) fn item_available(&self, item: &MixtapeCollectionItem) -> bool {
        match item.source {
            AlbumSource::Qobuz => {
                if !self.qobuz_allowed {
                    return false;
                }
                match item.item_type {
                    ItemType::Album => self.cached_album_ids.contains(&item.source_item_id),
                    ItemType::Track => item
                        .source_item_id
                        .parse::<u64>()
                        .map(|id| self.cached_track_ids.contains(&id))
                        .unwrap_or(false),
                    ItemType::Playlist => false,
                }
            }
            AlbumSource::Local => {
                if item.source_item_id.starts_with("plex:") {
                    return self.plex_allowed;
                }
                match item.item_type {
                    ItemType::Track => match item.source_item_id.parse::<i64>() {
                        Ok(id) if self.plex_local_rows.contains(&id) => self.plex_allowed,
                        Ok(_) => true,
                        Err(_) => false,
                    },
                    ItemType::Playlist => false,
                    ItemType::Album => true,
                }
            }
        }
    }
}

/// Build the offline availability snapshot for a set of collection items.
pub(crate) async fn offline_availability(items: &[&MixtapeCollectionItem]) -> OfflineAvailability {
    let (cached_track_ids, cached_album_ids) = match crate::offline_qt::get().await {
        Some(offline) => {
            let guard = offline.db.lock().await;
            match guard.as_ref().map(|db| db.get_all_tracks()) {
                Some(Ok(tracks)) => {
                    let mut ids = HashSet::new();
                    let mut albums = HashSet::new();
                    for track in tracks {
                        if matches!(track.status, qbz_offline_cache::OfflineCacheStatus::Ready) {
                            ids.insert(track.track_id);
                            if let Some(album_id) = track.album_id {
                                albums.insert(album_id);
                            }
                        }
                    }
                    (ids, albums)
                }
                _ => (HashSet::new(), HashSet::new()),
            }
        }
        None => (HashSet::new(), HashSet::new()),
    };

    let local_track_ids: Vec<i64> = items
        .iter()
        .filter(|item| item.source == AlbumSource::Local && item.item_type == ItemType::Track)
        .filter_map(|item| item.source_item_id.parse::<i64>().ok())
        .collect();
    let plex_local_rows = if local_track_ids.is_empty() {
        HashSet::new()
    } else {
        tokio::task::spawn_blocking(move || {
            crate::library_db_qt::with_db(false, |db| {
                let mut plex = HashSet::new();
                for id in local_track_ids {
                    if let Some(track) = db.get_track(id)? {
                        if track.source.as_deref() == Some("plex") {
                            plex.insert(id);
                        }
                    }
                }
                Ok(plex)
            })
            .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    };

    OfflineAvailability {
        cached_track_ids,
        cached_album_ids,
        plex_local_rows,
        qobuz_allowed: crate::offline_fwd::offline_playback_allowed(),
        plex_allowed: crate::offline_fwd::engine().status().mode
            == qbz_app::offline_mode::OfflineMode::InducedOffline,
    }
}

pub(crate) async fn retain_available_offline(
    rows: Vec<MixtapeCollection>,
) -> Vec<MixtapeCollection> {
    let items: Vec<&MixtapeCollectionItem> = rows
        .iter()
        .flat_map(|collection| collection.items.iter())
        .collect();
    let availability = offline_availability(&items).await;
    drop(items);
    rows.into_iter()
        .filter_map(|mut collection| {
            collection
                .items
                .retain(|item| availability.item_available(item));
            (!collection.items.is_empty()).then_some(collection)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Documents (spec 02 §5.1 GridDoc / §5.4 CreateDoc)
// ---------------------------------------------------------------------------

/// One ready-to-render grid card (1:1 with Slint's `MixtapeCardItem`, with the
/// nine flat `url*`/`cover*` pairs collapsed into two arrays — Slint structs
/// cannot hold nested models, JSON can).
#[derive(Clone, Default, Serialize)]
pub struct GridCard {
    pub id: String,
    pub name: String,
    /// "mixtape" | "collection" | "artist_collection".
    pub kind: String,
    /// Eyebrow, uppercase, already translated.
    pub label: String,
    /// "1 album" / "N albums" — always "album(s)" regardless of item type.
    pub meta: String,
    #[serde(rename = "itemCount")]
    pub item_count: i32,
    #[serde(rename = "playCount")]
    pub play_count: i32,
    /// Epoch MILLISECONDS. i64 end-to-end: Slint truncates this to i32 and the
    /// value is ~1.7e12 (spec 02 §7 T5). JSON numbers are f64 in QML, exact to
    /// 2^53, so the wire type is safe.
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(rename = "hasCustomCover")]
    pub has_custom_cover: bool,
    /// `file://…` for a readable custom cover, "" otherwise.
    #[serde(rename = "customCoverPath")]
    pub custom_cover_path: String,
    /// Mosaic cells actually used: 0 (custom cover or empty) | 4 | 9.
    #[serde(rename = "coverCount")]
    pub cover_count: i32,
    /// 2 or 3 — decided here so QML never re-derives the 3x3 rule.
    pub cols: i32,
    /// Pre-downscaled cell urls, `coverCount` entries ("" pads a short list).
    #[serde(rename = "cellUrls")]
    pub cell_urls: Vec<String>,
    /// `file://…` per cell, "" while the download is pending.
    #[serde(rename = "cellPaths")]
    pub cell_paths: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct GridDoc {
    /// "mixtapes" | "collections".
    pub grid: String,
    pub loading: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    pub search: String,
    /// "position" | "name" | "items" | "updated".
    pub sort: String,
    /// "asc" | "desc".
    #[serde(rename = "sortDir")]
    pub sort_dir: String,
    /// "grid" | "list".
    pub view: String,
    /// Collections only: "all" | "collection" | "artist_collection".
    #[serde(rename = "kindFilter")]
    pub kind_filter: String,
    pub cards: Vec<GridCard>,
}

impl GridDoc {
    fn new(grid: Grid) -> Self {
        Self {
            grid: grid.id().to_string(),
            loading: false,
            error: String::new(),
            search: String::new(),
            sort: "position".to_string(),
            sort_dir: "asc".to_string(),
            view: "grid".to_string(),
            kind_filter: "all".to_string(),
            cards: Vec::new(),
        }
    }
}

#[derive(Clone, Serialize)]
struct CreateDoc {
    open: bool,
    /// "mixtape" | "collection".
    kind: String,
    creating: bool,
}

impl Default for CreateDoc {
    fn default() -> Self {
        Self {
            open: false,
            kind: "mixtape".to_string(),
            creating: false,
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Last-loaded rows plus the published document, per grid. The cache is what
/// makes a toolbar change (search / sort / kind filter) a rebuild instead of a
/// refetch (`myqbz.rs:28-33`).
struct GridState {
    cache: Vec<MixtapeCollection>,
    doc: GridDoc,
}

impl GridState {
    fn new(grid: Grid) -> Self {
        Self {
            cache: Vec::new(),
            doc: GridDoc::new(grid),
        }
    }
}

static MIXTAPES: LazyLock<Mutex<GridState>> =
    LazyLock::new(|| Mutex::new(GridState::new(Grid::Mixtapes)));
static COLLECTIONS: LazyLock<Mutex<GridState>> =
    LazyLock::new(|| Mutex::new(GridState::new(Grid::Collections)));
static CREATE: LazyLock<Mutex<CreateDoc>> = LazyLock::new(|| Mutex::new(CreateDoc::default()));

fn lock_grid(grid: Grid) -> MutexGuard<'static, GridState> {
    let cell = match grid {
        Grid::Mixtapes => &MIXTAPES,
        Grid::Collections => &COLLECTIONS,
    };
    cell.lock().unwrap_or_else(|e| e.into_inner())
}

fn with_grid<R>(grid: Grid, f: impl FnOnce(&mut GridState) -> R) -> R {
    f(&mut lock_grid(grid))
}

// ---------------------------------------------------------------------------
// Publish
// ---------------------------------------------------------------------------

fn publish(grid: Grid) {
    let json = with_grid(grid, |g| {
        serde_json::to_string(&g.doc).unwrap_or_else(|_| "{}".into())
    });
    match grid {
        Grid::Mixtapes => crate::myqbz_bridge::ui(move |mut b| {
            b.as_mut().set_mixtapes_json(QString::from(json.as_str()));
        }),
        Grid::Collections => crate::myqbz_bridge::ui(move |mut b| {
            b.as_mut()
                .set_collections_json(QString::from(json.as_str()));
        }),
    }
}

fn publish_create() {
    let json = {
        let doc = CREATE.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::to_string(&*doc).unwrap_or_else(|_| "{}".into())
    };
    crate::myqbz_bridge::ui(move |mut b| {
        b.as_mut().set_create_json(QString::from(json.as_str()));
    });
}

// ---------------------------------------------------------------------------
// Session binding
// ---------------------------------------------------------------------------

/// Bind the domain to a session and run the mixtape schema migrations.
///
/// ONE entry point (spec 02 §2.3): the migrations create
/// `mixtape_collections`, `mixtape_collection_items` and the additive
/// `session_queue_state.source_collection_id` column, and WITHOUT them every
/// read fails with `no such table`. `library_db_qt::with_db(true, …)` is the
/// only create-capable accessor in the port, which a fresh account needs
/// (`local_state::with_db` returns `None` when `library.db` does not exist yet,
/// so the migrations would never run and the first create would no-op).
///
/// Best-effort and SYNCHRONOUS, 1:1 with `qbz/src/main.rs:183-196`: the DDL is
/// idempotent and takes milliseconds, and deferring it onto a blocking task
/// would let the first grid read race ahead of the tables it needs. Errors are
/// logged and never block shell entry.
pub(crate) fn init_for_user(dir: &Path, user_id: u64) {
    let done = crate::library_db_qt::with_db(true, |db| {
        db.with_connection(|conn| {
            qbz_mixtape::schema::run_mixtape_migrations(conn).map_err(|e| {
                qbz_library::LibraryError::Database(format!("mixtape migrations failed: {e}"))
            })
        })
    });
    match done {
        Some(()) => log::info!(
            "[qbz-qt] myqbz: mixtape migrations ok (user {user_id}, {})",
            dir.display()
        ),
        None => log::warn!(
            "[qbz-qt] myqbz: mixtape migrations did NOT run (user {user_id}, {}) — \
             the grids will read empty",
            dir.display()
        ),
    }
}

/// Drop every per-user grid trace on logout, or the next account inherits the
/// previous one's collections (`auth_qt.rs:358-362`).
pub(crate) fn teardown() {
    for grid in [Grid::Mixtapes, Grid::Collections] {
        with_grid(grid, |g| {
            g.cache.clear();
            g.doc = GridDoc::new(grid);
        });
        publish(grid);
    }
    *CREATE.lock().unwrap_or_else(|e| e.into_inner()) = CreateDoc::default();
    publish_create();
}

// ---------------------------------------------------------------------------
// DB read path
// ---------------------------------------------------------------------------

/// List collections of the given kind filter from the per-user library.db.
/// `None` returns all kinds. Items come hydrated by the repo, so counts and
/// mosaics are accurate. Empty when the DB is unavailable (logged by
/// `with_db`). BLOCKING — call from `spawn_blocking`.
pub(crate) fn list_collections(kind: Option<CollectionKind>) -> Vec<MixtapeCollection> {
    crate::library_db_qt::with_db(false, |db| {
        Ok(db.with_connection(|conn| {
            qbz_mixtape::repo::list_collections(conn, kind).unwrap_or_else(|e| {
                log::warn!("[qbz-qt] myqbz list_collections failed: {e}");
                Vec::new()
            })
        }))
    })
    .unwrap_or_default()
}

/// Kiosk needs to distinguish a failed query from a successful empty library.
fn list_collections_kiosk(kind: Option<CollectionKind>) -> Result<Vec<MixtapeCollection>, String> {
    crate::library_db_qt::with_db(false, |db| {
        Ok(db.with_connection(|conn| {
            qbz_mixtape::repo::list_collections(conn, kind).map_err(|e| e.to_string())
        }))
    })
    .ok_or_else(|| "MyQBZ database unavailable".to_string())?
}

/// Create a new manual collection of `kind` named `name` and return the created
/// row. The repo shifts existing positions so the new row lands at position 0
/// (the TOP of its kind). `source_type` is always `Manual` and
/// `description`/`source_ref` are NULL — artist_collections come from the
/// Discography Builder, never this path. BLOCKING — call from
/// `spawn_blocking`. `None` when the DB is unavailable or the insert failed.
pub(crate) fn create_collection(kind: CollectionKind, name: &str) -> Option<MixtapeCollection> {
    // Cap at 80 Unicode scalar values (HTML `maxlength` semantics, the shape
    // the Tauri input had — `myqbz.rs:68-70`).
    let name: String = name.chars().take(80).collect();
    crate::library_db_qt::with_db(true, |db| {
        db.with_connection(|conn| {
            qbz_mixtape::repo::create_collection(
                conn,
                kind,
                &name,
                None,
                qbz_models::mixtape::CollectionSourceType::Manual,
                None,
            )
        })
        .map_err(|e| {
            qbz_library::LibraryError::Database(format!("myqbz create_collection failed: {e}"))
        })
    })
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

fn kind_str(kind: CollectionKind) -> &'static str {
    match kind {
        CollectionKind::Mixtape => "mixtape",
        CollectionKind::Collection => "collection",
        CollectionKind::ArtistCollection => "artist_collection",
    }
}

/// Eyebrow label, uppercase and translated (`myqbz.rs:255`).
pub(crate) fn label_for(kind: CollectionKind) -> String {
    match kind {
        CollectionKind::Mixtape => qbz_i18n::t("MIXTAPE"),
        CollectionKind::Collection => qbz_i18n::t("COLLECTION"),
        CollectionKind::ArtistCollection => qbz_i18n::t("ARTIST"),
    }
}

/// "1 album" / "N albums" — always "album(s)" regardless of item type
/// (`myqbz.rs:265`).
pub(crate) fn album_count_label(count: usize) -> String {
    qbz_i18n::tf("{} album", "{} albums", count as i64, &[&count.to_string()])
}

/// Pre-downscale a Qobuz cover url to a per-cell target size (`myqbz.rs:273`).
/// Lowercase scan for the size token, rewritten in place so the rest of the
/// url keeps its original case. Non-matching urls pass through unchanged.
///
/// CALLERS MUST GATE THIS ON `AlbumSource::Qobuz`: a local filesystem path or
/// a Plex `/library/...` path has to pass through raw. `myqbz.rs:353-361` does
/// not gate it — that is a latent Slint bug the hero already avoids
/// (`myqbz_detail.rs:383-392`), and this port gates it in both places.
pub(crate) fn small_qobuz_url(url: &str, target: u32) -> String {
    if url.is_empty() {
        return String::new();
    }
    const TOKENS: [&str; 8] = [
        "_50.jpg", "_100.jpg", "_150.jpg", "_230.jpg", "_300.jpg", "_600.jpg", "_max.jpg",
        "_org.jpg",
    ];
    let lower = url.to_lowercase();
    for tok in TOKENS {
        if let Some(pos) = lower.rfind(tok) {
            let mut out = String::with_capacity(url.len());
            out.push_str(&url[..pos]);
            out.push_str(&format!("_{target}.jpg"));
            out.push_str(&url[pos + tok.len()..]);
            return out;
        }
    }
    url.to_string()
}

/// Per-cell downscale target for a mosaic of `size` px in `cols` columns
/// (`cellPx = round(size/cols)`; `<=80 -> 50`, `<=200 -> 150`, else 300).
/// The 184px grid card yields 2x2 -> 150 and 3x3 -> 50 (`myqbz.rs:296`).
pub(crate) fn cell_target(size: u32, cols: u32) -> u32 {
    let cell_px = ((size as f32) / (cols as f32)).round() as u32;
    if cell_px <= 80 {
        50
    } else if cell_px <= 200 {
        150
    } else {
        300
    }
}

// ---------------------------------------------------------------------------
// Card mapping
// ---------------------------------------------------------------------------

/// Build one ready-to-render card: the 2x2-vs-3x3 decision, the cover count,
/// and the pre-downscaled + disk-resolved cell urls (`myqbz.rs:311`).
///
/// `hasCustomCover` folds Slint's three-part predicate (path non-empty AND
/// `Path::exists()` AND decodable) into one: `artwork_qt::cached_path` returns
/// "" for a path that is not a readable file, so an unreadable cover degrades
/// to the mosaic — and the SAME predicate drives `coverCount`. The detail
/// hero deliberately does NOT do this (`myqbz_detail.rs:367` is a bare
/// `is_some()`), so a deleted cover file shows the kind glyph in the hero and
/// the full mosaic here. Do not unify them.
fn card_item(c: &MixtapeCollection) -> GridCard {
    let item_count = c.items.len();

    let custom_cover_path = c
        .custom_artwork_path
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(crate::artwork_qt::cached_path)
        .unwrap_or_default();
    let has_custom = !custom_cover_path.is_empty();

    // 3x3 only for a Collection with >= 9 items; else 2x2.
    let cols: u32 = if c.kind == CollectionKind::Collection && item_count >= 9 {
        3
    } else {
        2
    };
    let cell_count = (cols * cols) as usize;
    let cover_count = if has_custom || item_count == 0 {
        0
    } else {
        cell_count
    };

    let target = cell_target(CARD_MOSAIC_SIZE, cols);
    let mut cell_urls: Vec<String> = Vec::with_capacity(cover_count);
    let mut cell_paths: Vec<String> = Vec::with_capacity(cover_count);
    for i in 0..cover_count {
        let url = match c.items.get(i) {
            Some(it) => match it.artwork_url.as_deref().filter(|u| !u.is_empty()) {
                // Qobuz only: a local/Plex path must pass through raw.
                Some(u) if it.source == AlbumSource::Qobuz => small_qobuz_url(u, target),
                Some(u) => u.to_string(),
                None => String::new(),
            },
            None => String::new(),
        };
        let path = if url.is_empty() {
            String::new()
        } else {
            crate::artwork_qt::cached_path(&url)
        };
        cell_urls.push(url);
        cell_paths.push(path);
    }

    GridCard {
        id: c.id.clone(),
        name: c.name.clone(),
        kind: kind_str(c.kind).to_string(),
        label: label_for(c.kind),
        meta: album_count_label(item_count),
        item_count: item_count as i32,
        play_count: c.play_count,
        updated_at: c.updated_at,
        has_custom_cover: has_custom,
        custom_cover_path,
        cover_count: cover_count as i32,
        cols: cols as i32,
        cell_urls,
        cell_paths,
    }
}

// ---------------------------------------------------------------------------
// Sort / filter
// ---------------------------------------------------------------------------

/// Sort by the active toolbar sort (`myqbz.rs:418`). `dir` = "asc" | "desc".
fn sort_collections(list: &mut [MixtapeCollection], sort: &str, dir: &str) {
    match sort {
        "name" => list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        "items" => list.sort_by(|a, b| a.items.len().cmp(&b.items.len())),
        "updated" => list.sort_by(|a, b| a.updated_at.cmp(&b.updated_at)),
        // default "position"
        _ => list.sort_by(|a, b| a.position.cmp(&b.position)),
    }
    if dir == "desc" {
        list.reverse();
    }
}

/// Search filter: case-insensitive substring over name OR description. An
/// empty query passes (`myqbz.rs:434`). `query` must already be lowercased.
fn passes_search(c: &MixtapeCollection, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if c.name.to_lowercase().contains(query) {
        return true;
    }
    c.description
        .as_deref()
        .map(|d| d.to_lowercase().contains(query))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Apply / rebuild
// ---------------------------------------------------------------------------

fn set_loading(grid: Grid, loading: bool) {
    with_grid(grid, |g| {
        g.doc.loading = loading;
        if loading {
            g.doc.error.clear();
        }
    });
    publish(grid);
}

/// Rebuild one grid's card list from its cache through the active toolbar
/// state (`myqbz.rs:473`). Pure in-memory; clears `loading`.
fn rebuild(grid: Grid) {
    with_grid(grid, |g| {
        let query = g.doc.search.trim().to_lowercase();
        let kind_filter = g.doc.kind_filter.clone();
        let sort = g.doc.sort.clone();
        let dir = g.doc.sort_dir.clone();
        let mut filtered: Vec<MixtapeCollection> = g
            .cache
            .iter()
            .filter(|c| match grid {
                Grid::Mixtapes => true,
                Grid::Collections => match kind_filter.as_str() {
                    "collection" => c.kind == CollectionKind::Collection,
                    "artist_collection" => c.kind == CollectionKind::ArtistCollection,
                    _ => true,
                },
            })
            .filter(|c| passes_search(c, &query))
            .cloned()
            .collect();
        sort_collections(&mut filtered, &sort, &dir);
        g.doc.cards = filtered.iter().map(card_item).collect();
        g.doc.loading = false;
    });
}

/// Store freshly-loaded rows and render them through the active toolbar state.
fn apply(grid: Grid, rows: Vec<MixtapeCollection>) {
    with_grid(grid, |g| g.cache = rows);
    rebuild(grid);
    publish(grid);
}

// ---------------------------------------------------------------------------
// Artwork
// ---------------------------------------------------------------------------

/// The cell urls that are still not on disk, deduped. Custom covers are local
/// files — `cached_path` already resolved them synchronously and
/// `download_missing` skips them, so they are not collected.
fn missing_cell_urls(grid: Grid) -> Vec<String> {
    with_grid(grid, |g| {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for card in &g.doc.cards {
            for (i, url) in card.cell_urls.iter().enumerate() {
                if url.is_empty() {
                    continue;
                }
                let on_disk = card
                    .cell_paths
                    .get(i)
                    .map(|p| !p.is_empty())
                    .unwrap_or(false);
                if on_disk {
                    continue;
                }
                if seen.insert(url.clone()) {
                    out.push(url.clone());
                }
            }
        }
        out
    })
}

/// Shared cache and provider bucket selection; no parallel network pipeline.
pub(crate) fn kiosk_art_url(url: &str, px: i32) -> String {
    if !crate::cover_artwork_qt::override_for_url(url).is_empty() {
        return url.to_string();
    }
    qbz_models::qobuz_cover_at_px(url, px.clamp(1, 1200) as u32).unwrap_or_else(|| url.to_string())
}

pub(crate) fn kiosk_grid_artwork(grid: &str, first: i32, last: i32, px: i32) {
    if !crate::kiosk_profile_qt::active() || first < 0 || last < first {
        return;
    }
    let grid = grid_from_str(grid);
    let jobs = with_grid(grid, |g| {
        g.doc
            .cards
            .iter()
            .skip(first as usize)
            .take((last - first + 1).min(128) as usize)
            .filter(|c| !c.has_custom_cover)
            .filter_map(|c| {
                c.cell_urls
                    .first()
                    .filter(|u| !u.is_empty())
                    .map(|url| (c.id.clone(), url.clone(), kiosk_art_url(url, px)))
            })
            .collect::<Vec<_>>()
    });
    crate::spawn(async move {
        crate::artwork_qt::download_missing(jobs.iter().map(|j| j.2.clone()).collect()).await;
        if !crate::kiosk_profile_qt::active() {
            return;
        }
        let changed = with_grid(grid, |g| {
            let mut changed = false;
            for (id, original, url) in jobs {
                if let Some(c) = g
                    .doc
                    .cards
                    .iter_mut()
                    .find(|c| c.id == id && c.cell_urls.first() == Some(&original))
                {
                    let path = crate::artwork_qt::cached_path(&url);
                    if !path.is_empty() && c.cell_paths.first() != Some(&path) {
                        if c.cell_paths.is_empty() {
                            c.cell_paths.push(path);
                        } else {
                            c.cell_paths[0] = path;
                        }
                        changed = true;
                    }
                }
            }
            changed
        });
        if changed {
            publish(grid);
        }
    });
}

/// ONE download pass per load, then ONE republish (spec 02 §7 T17 — never
/// republish per cover).
async fn artwork_pass(grid: Grid) {
    if crate::kiosk_profile_qt::active() {
        return;
    }
    let missing = missing_cell_urls(grid);
    if missing.is_empty() {
        return;
    }
    crate::artwork_qt::download_missing(missing).await;
    rebuild(grid);
    publish(grid);
}

/// Fire the artwork pass for whatever is visible NOW, after a toolbar rebuild.
///
/// The reference runs `refresh_covers` (its whole `artwork_jobs` dispatch) after
/// EVERY grid toolbar change — search, sort, kind-filter, reset — and not after
/// the grid/list view toggle (`main.rs:6019-6115`). Without it, a card that the
/// load's own pass never rendered (the load rebuilt through a non-empty search,
/// so the filtered-out cards' cells were never collected as missing) keeps blank
/// mosaic tiles until the next `reload_grids`. Cheap: a card whose cells are
/// already on disk resolves synchronously inside `rebuild`, so `missing` is
/// empty and this is a no-op with no republish.
fn artwork_refresh(grid: Grid) {
    crate::spawn(async move { artwork_pass(grid).await });
}

// ---------------------------------------------------------------------------
// Navigation / loading
// ---------------------------------------------------------------------------

/// Load one grid from the per-user library.db and render it, then fill the
/// mosaic covers (`myqbz.rs:594 navigate`).
///
/// The nav entry is recorded by `crate::navigate_to` (which is what routes
/// here), so this never records one itself — that also keeps `reload_grids`
/// from pushing history entries after a mutation.
///
/// The grids reload on EVERY visit, deliberately: a create / delete / rename
/// from the Add picker, the Discography Builder or the detail view changes the
/// set, so a once-per-session flag would show a stale grid. The read is one
/// local SQLite query.
pub(crate) fn load_grid(grid: Grid) {
    set_loading(grid, true);
    crate::spawn(async move {
        // Mixtapes asks for kind == mixtape; Collections loads ALL kinds and
        // drops mixtapes locally, so the kind-filter dropdown switches between
        // collection and artist_collection with no refetch (`myqbz.rs:615-633`).
        let kind_arg = match grid {
            Grid::Mixtapes => Some(CollectionKind::Mixtape),
            Grid::Collections => None,
        };
        let rows = if crate::kiosk_profile_qt::active() {
            match tokio::task::spawn_blocking(move || list_collections_kiosk(kind_arg)).await {
                Ok(Ok(rows)) => rows,
                result => {
                    log::warn!("[qbz-qt] kiosk MyQBZ grid read failed: {result:?}");
                    with_grid(grid, |g| {
                        g.doc.loading = false;
                        g.doc.error = qbz_i18n::t("Error");
                        g.doc.cards.clear();
                    });
                    publish(grid);
                    return;
                }
            }
        } else {
            tokio::task::spawn_blocking(move || list_collections(kind_arg))
                .await
                .unwrap_or_default()
        };
        let mut rows: Vec<MixtapeCollection> = match grid {
            Grid::Mixtapes => rows,
            Grid::Collections => rows
                .into_iter()
                .filter(|c| c.kind != CollectionKind::Mixtape)
                .collect(),
        };
        if crate::offline_fwd::engine().status().is_offline() {
            rows = retain_available_offline(rows).await;
        }
        apply(grid, rows);
        artwork_pass(grid).await;
    });
}

/// Re-list BOTH grids and republish.
///
/// `nav_qt::back()` publishes only `currentView` and never re-dispatches a
/// load (unlike Slint's `request_back`), so every mutation that lands — create,
/// rename, description, play-mode, convert-kind, delete, bulk-remove, cover
/// upload/remove, an add from the picker, the builder's create — calls this at
/// its tail or the user lands on a stale grid card. `convert_kind` in
/// particular MOVES a row between the two grids, which is why both republish
/// (spec 02 §7 T4).
pub(crate) fn reload_grids() {
    load_grid(Grid::Mixtapes);
    load_grid(Grid::Collections);
}

/// Re-render both grids from cache after a live language switch: the eyebrow
/// labels and the "N albums" plurals are Rust-built and do not re-translate
/// otherwise (spec 02 §10, wiring W18). No DB read, no artwork pass.
pub(crate) fn republish_all() {
    for grid in [Grid::Mixtapes, Grid::Collections] {
        rebuild(grid);
        publish(grid);
    }
}

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

/// Search (`myqbz.rs:478`): no debounce, 1:1 with the Slint.
pub(crate) fn grid_search(grid: &str, query: &str) {
    let grid = grid_from_str(grid);
    with_grid(grid, |g| g.doc.search = query.to_string());
    rebuild(grid);
    publish(grid);
    artwork_refresh(grid);
}

/// Re-picking the active sort field flips the direction; a new field resets to
/// `asc` (`myqbz.rs:512`).
pub(crate) fn grid_set_sort(grid: &str, field: &str) {
    let grid = grid_from_str(grid);
    with_grid(grid, |g| {
        let next_dir = if g.doc.sort == field {
            if g.doc.sort_dir == "asc" {
                "desc"
            } else {
                "asc"
            }
        } else {
            "asc"
        };
        g.doc.sort = field.to_string();
        g.doc.sort_dir = next_dir.to_string();
    });
    rebuild(grid);
    publish(grid);
    artwork_refresh(grid);
}

/// "grid" | "list". Session-scoped, not persisted (1:1 with Slint's global).
pub(crate) fn grid_set_view(grid: &str, view: &str) {
    let grid = grid_from_str(grid);
    let view = if view == "list" { "list" } else { "grid" };
    with_grid(grid, |g| g.doc.view = view.to_string());
    publish(grid);
}

/// Collections only: "all" | "collection" | "artist_collection". No refetch —
/// the cache already holds both kinds.
pub(crate) fn grid_set_kind_filter(kind: &str) {
    let kind = match kind {
        "collection" | "artist_collection" => kind,
        _ => "all",
    };
    with_grid(Grid::Collections, |g| g.doc.kind_filter = kind.to_string());
    rebuild(Grid::Collections);
    publish(Grid::Collections);
    artwork_refresh(Grid::Collections);
}

/// Reset the toolbar: sort `position`/`asc` and an empty search, plus
/// `kindFilter = "all"` for Collections (`myqbz.rs:537`).
pub(crate) fn grid_reset(grid: &str) {
    let grid = grid_from_str(grid);
    with_grid(grid, |g| {
        g.doc.sort = "position".to_string();
        g.doc.sort_dir = "asc".to_string();
        g.doc.search = String::new();
        if grid == Grid::Collections {
            g.doc.kind_filter = "all".to_string();
        }
    });
    rebuild(grid);
    publish(grid);
    artwork_refresh(grid);
}

// ---------------------------------------------------------------------------
// Create modal (`qbz/src/main.rs:5795-5895`)
// ---------------------------------------------------------------------------

/// "+ New Mixtape" / "+ New Collection". The kind is fixed by the grid that
/// opened the modal; the radio can flip it.
pub(crate) fn create_open(kind: &str) {
    {
        let mut doc = CREATE.lock().unwrap_or_else(|e| e.into_inner());
        doc.kind = if kind == "collection" {
            "collection".to_string()
        } else {
            "mixtape".to_string()
        };
        doc.creating = false;
        doc.open = true;
    }
    publish_create();
}

pub(crate) fn create_set_kind(kind: &str) {
    {
        let mut doc = CREATE.lock().unwrap_or_else(|e| e.into_inner());
        doc.kind = if kind == "collection" {
            "collection".to_string()
        } else {
            "mixtape".to_string()
        };
    }
    publish_create();
}

pub(crate) fn create_close() {
    {
        let mut doc = CREATE.lock().unwrap_or_else(|e| e.into_inner());
        doc.open = false;
        doc.creating = false;
    }
    publish_create();
}

/// Submit: create on a blocking worker, then close the modal and drop the user
/// straight into the new collection's detail view. A blank name or an in-flight
/// create is ignored. On failure the modal STAYS open with `creating = false`
/// and an error toast — the raw English literal the reference uses (absent from
/// all eight catalogs, kept 1:1; spec 02 §2.4.2).
pub(crate) fn create_submit(name: String) {
    let kind = {
        let mut doc = CREATE.lock().unwrap_or_else(|e| e.into_inner());
        if name.trim().is_empty() || doc.creating {
            return;
        }
        doc.creating = true;
        kind_from_str(&doc.kind)
    };
    publish_create();

    crate::spawn(async move {
        let trimmed = name.trim().to_string();
        let created = tokio::task::spawn_blocking(move || create_collection(kind, &trimmed))
            .await
            .ok()
            .flatten();
        match created {
            Some(c) => {
                {
                    let mut doc = CREATE.lock().unwrap_or_else(|e| e.into_inner());
                    doc.creating = false;
                    doc.open = false;
                }
                publish_create();
                // The new row lands at position 0 of its kind — republish both
                // grids so BACK from the detail is not stale (§7 T4).
                reload_grids();
                crate::myqbz_open_detail(c.id);
            }
            None => {
                {
                    let mut doc = CREATE.lock().unwrap_or_else(|e| e.into_inner());
                    doc.creating = false;
                }
                publish_create();
                crate::toast_qt::error("Failed to create collection".to_string());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_qobuz_url_rewrites_the_last_size_token() {
        assert_eq!(
            small_qobuz_url("https://x/img_600.jpg", 150),
            "https://x/img_150.jpg"
        );
        assert_eq!(
            small_qobuz_url("https://x/b_max.jpg", 50),
            "https://x/b_50.jpg"
        );
        // The token list is scanned IN ORDER and the first token that occurs
        // wins, so a url carrying two size tokens rewrites the one that appears
        // earliest in the list, not the last one in the string. Quirk of the
        // reference (`myqbz.rs:279-289`), reproduced.
        assert_eq!(
            small_qobuz_url("https://x/a_600.jpg/b_max.jpg", 50),
            "https://x/a_50.jpg/b_max.jpg"
        );
        // Non-Qobuz shapes pass through.
        assert_eq!(small_qobuz_url("/music/cover.png", 50), "/music/cover.png");
        assert_eq!(small_qobuz_url("", 50), String::new());
    }

    #[test]
    fn cell_target_matches_the_slint_tiers() {
        // 184px card: 2x2 -> 92px cells -> 150; 3x3 -> 61px cells -> 50.
        assert_eq!(cell_target(CARD_MOSAIC_SIZE, 2), 150);
        assert_eq!(cell_target(CARD_MOSAIC_SIZE, 3), 50);
        // 186px hero: same tiers (`myqbz_detail.rs:375`).
        assert_eq!(cell_target(186, 2), 150);
        assert_eq!(cell_target(186, 3), 50);
        // A big single cell.
        assert_eq!(cell_target(600, 1), 300);
    }

    #[test]
    fn kind_and_grid_parsing_default_safely() {
        assert_eq!(kind_from_str("collection"), CollectionKind::Collection);
        assert_eq!(
            kind_from_str("artist_collection"),
            CollectionKind::ArtistCollection
        );
        assert_eq!(kind_from_str("nonsense"), CollectionKind::Mixtape);
        assert_eq!(grid_from_str("collections"), Grid::Collections);
        assert_eq!(grid_from_str(""), Grid::Mixtapes);
    }
}
