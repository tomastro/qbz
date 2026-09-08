//! Native paged Albums collection over the derived catalog (phase F1).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::{c_char, CString};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cxx_qt_lib::QString;
use qbz_library::AudioFormat;
use qbz_local_catalog::{
    ActiveCatalog, AlbumCursor, AlbumRecord, BootstrapLayout, QueryDescriptor, SourceKey,
    SourceKind, TrackGroup, TrackSort,
};
use serde::Serialize;

use crate::local_bridge::ui;
use crate::local_rows::{
    album_favorite_source, badge_source, badge_source_raw, catalog_album_sources, detail_of,
    total_duration, AlbumMediaVariant, AlbumRow,
};

const PAGE_ENTRIES: usize = 100;
const FLAT_QUERY_ROWS: usize = 500;
const MAX_RESIDENT_ALBUMS: usize = 2_000;
const MAX_UNANCHORED_ENTRIES: usize = 500;
const MAX_COLUMNS: usize = MAX_RESIDENT_ALBUMS / PAGE_ENTRIES;

static QUERY_GENERATION: AtomicU64 = AtomicU64::new(0);
static SESSION_FAILED: AtomicBool = AtomicBool::new(false);
static SESSION: Mutex<Option<NativeSession>> = Mutex::new(None);
static LAST_QUERY: Mutex<Option<LastQuery>> = Mutex::new(None);

#[derive(Clone, PartialEq, Eq)]
struct LastQuery {
    search: String,
    sort: String,
    group: String,
    filter_json: String,
    columns: i32,
}

unsafe extern "C" {
    fn qbz_local_albums_register_qml_type();
    fn qbz_local_albums_reset(generation: i32, total_count: i64, album_total: i64);
    fn qbz_local_albums_apply_page(generation: i32, page: i32, json: *const c_char) -> bool;
    fn qbz_local_albums_set_selection(generation: i32, json: *const c_char, selected_count: i64);
}

#[derive(Clone)]
struct IndexedAlbum {
    index: usize,
    record: AlbumRecord,
}

#[derive(Clone, Default)]
struct StreamAnchor {
    entry_index: usize,
    flat_index: usize,
    cursor: Option<AlbumCursor>,
    previous_group: Option<String>,
    pending: Vec<IndexedAlbum>,
    done: bool,
}

#[derive(Clone)]
struct CachedPage {
    wire: Arc<Vec<NativeEntry>>,
    albums: usize,
}

struct NativeSession {
    generation: u64,
    catalog_generation: u64,
    query: LastQuery,
    descriptor: QueryDescriptor,
    columns: usize,
    album_total: u64,
    entry_total: u64,
    anchors: BTreeMap<usize, StreamAnchor>,
    anchors_ready: bool,
    anchor_building: bool,
    pending_pages: HashSet<usize>,
    waiting_pages: HashSet<usize>,
    cache: PageLru,
    selection: QuerySelection,
}

#[derive(Clone, Serialize)]
struct NativeEntry {
    t: u8,
    label: String,
    base: usize,
    items: Vec<NativeAlbum>,
}

#[derive(Clone, Serialize)]
struct NativeAlbum {
    #[serde(flatten)]
    row: AlbumRow,
    #[serde(rename = "nativeIndex")]
    native_index: usize,
    #[serde(rename = "artPath")]
    art_path: String,
}

#[derive(Serialize)]
struct NativeJump {
    letter: String,
    index: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct QuerySelection {
    all: bool,
    ranges: Vec<(u64, u64)>,
    anchor: Option<u64>,
}

impl QuerySelection {
    fn contains(&self, index: u64) -> bool {
        let inside = self
            .ranges
            .iter()
            .any(|&(first, last)| first <= index && index <= last);
        if self.all {
            !inside
        } else {
            inside
        }
    }

    fn count(&self, total: u64) -> u64 {
        let ranges = self
            .ranges
            .iter()
            .map(|&(first, last)| last.saturating_sub(first).saturating_add(1))
            .sum::<u64>();
        if self.all {
            total.saturating_sub(ranges)
        } else {
            ranges.min(total)
        }
    }

    fn toggle(&mut self, index: u64, shift: bool) {
        if shift {
            if let Some(anchor) = self.anchor {
                let (first, last) = if anchor <= index {
                    (anchor, index)
                } else {
                    (index, anchor)
                };
                if self.all {
                    remove_interval(&mut self.ranges, first, last);
                } else {
                    add_interval(&mut self.ranges, first, last);
                }
                return;
            }
        }
        if self.contains(index) {
            if self.all {
                add_interval(&mut self.ranges, index, index);
            } else {
                remove_interval(&mut self.ranges, index, index);
            }
        } else if self.all {
            remove_interval(&mut self.ranges, index, index);
        } else {
            add_interval(&mut self.ranges, index, index);
        }
        self.anchor = Some(index);
    }
}

#[derive(Default)]
struct PageLru {
    entries: HashMap<usize, CachedPage>,
    order: VecDeque<usize>,
    resident_albums: usize,
    evictions: u64,
}

impl PageLru {
    fn get(&mut self, page: usize) -> Option<&CachedPage> {
        if !self.entries.contains_key(&page) {
            return None;
        }
        self.touch(page);
        self.entries.get(&page)
    }

    fn insert(&mut self, page: usize, value: CachedPage) {
        if let Some(previous) = self.entries.insert(page, value) {
            self.resident_albums = self.resident_albums.saturating_sub(previous.albums);
        }
        self.resident_albums += self.entries[&page].albums;
        self.touch(page);
        while self.resident_albums > MAX_RESIDENT_ALBUMS && self.entries.len() > 1 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.resident_albums = self.resident_albums.saturating_sub(removed.albums);
                self.evictions += 1;
            }
        }
    }

    fn touch(&mut self, page: usize) {
        self.order.retain(|candidate| *candidate != page);
        self.order.push_back(page);
    }
}

struct OpenedQuery {
    generation: u64,
    catalog_generation: u64,
    query: LastQuery,
    descriptor: QueryDescriptor,
    columns: usize,
    album_total: u64,
    entry_total: u64,
    loaded: LoadedPage,
    count_time: std::time::Duration,
}

struct LoadedPage {
    generation: u64,
    page: usize,
    wire: Vec<NativeEntry>,
    anchors: Vec<StreamAnchor>,
    jumps: Vec<NativeJump>,
    query_time: std::time::Duration,
    map_time: std::time::Duration,
}

pub(crate) fn register_qml_model() {
    // SAFETY: link anchor only; registration is a QCoreApplication startup hook.
    unsafe { qbz_local_albums_register_qml_type() };
}

pub(crate) fn requested() -> bool {
    if SESSION_FAILED.load(Ordering::Acquire) {
        return false;
    }
    !std::env::var("QBZ_LOCAL_CATALOG_ALBUMS")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
}

pub(crate) fn reset(
    search: String,
    sort: String,
    group: String,
    filter_json: String,
    columns: i32,
) -> bool {
    let query = LastQuery {
        search,
        sort,
        group,
        filter_json,
        columns,
    };
    *LAST_QUERY.lock().unwrap_or_else(|error| error.into_inner()) = Some(query.clone());
    reset_inner(query, false)
}

fn reset_inner(query: LastQuery, force: bool) -> bool {
    if !requested() {
        return false;
    }
    // The derived identity currently matches the owner's default folder
    // grouping. Keep the metadata-mode reader as this surface's fallback
    // until that alternate projection can preserve its duplicate semantics.
    if crate::local_library_qt::album_mode() != "folder" {
        deactivate_for_legacy("metadata-mode");
        crate::local_bridge_ops::load_albums_legacy();
        return false;
    }
    if !force {
        let reused = SESSION
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_some_and(|session| session.query == query);
        if reused {
            ui(|mut bridge| {
                bridge.as_mut().set_local_albums_native_active(true);
                bridge.as_mut().set_local_albums_loading(false);
                bridge.as_mut().set_local_albums_error(QString::default());
                bridge
                    .as_mut()
                    .set_local_albums_native_error(QString::default());
            });
            log::info!("[local-catalog] phase=albums-reuse reason=matching-descriptor");
            return true;
        }
    }
    let generation = next_generation();
    let columns = usize::try_from(query.columns.max(1))
        .unwrap_or(1)
        .clamp(1, MAX_COLUMNS);
    let descriptor = descriptor(
        query.search.clone(),
        &query.sort,
        &query.group,
        &query.filter_json,
    );
    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = None;
    set_loading();
    if !descriptor.search().is_empty() && descriptor.search().chars().count() < 3 {
        publish_empty(generation);
        return true;
    }
    crate::spawn(async move {
        let result =
            tokio::task::spawn_blocking(move || {
                open_query(generation, query, descriptor, columns)
            })
            .await;
        match result {
            Ok(Ok(opened)) => activate_query(opened),
            Ok(Err(reason)) => fallback(generation, reason),
            Err(_) => fallback(generation, "worker-join"),
        }
    });
    true
}

/// Retry the last QML descriptor after background bootstrap/catch-up makes a
/// catalog generation available.
pub(crate) fn retry_last(force: bool) -> bool {
    let query = LAST_QUERY
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    query.is_some_and(|query| reset_inner(query, force))
}

pub(crate) fn request_page(page: i32, generation: i32) {
    if page < 0 || generation <= 0 {
        return;
    }
    let page = page as usize;
    let generation = generation as u64;
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let Some(current) = session
        .as_mut()
        .filter(|state| state.generation == generation)
    else {
        return;
    };
    if let Some(cached) = current.cache.get(page).cloned() {
        let resident = current.cache.resident_albums;
        let evictions = current.cache.evictions;
        drop(session);
        publish_page(generation, page, cached.wire, true, resident, evictions);
        return;
    }
    if !current.pending_pages.insert(page) {
        return;
    }
    let target = page.saturating_mul(PAGE_ENTRIES);
    let anchor = nearest_anchor(&current.anchors, target);
    if target.saturating_sub(anchor.entry_index) > MAX_UNANCHORED_ENTRIES && !current.anchors_ready
    {
        current.pending_pages.remove(&page);
        current.waiting_pages.insert(page);
        let start_builder = !current.anchor_building;
        drop(session);
        if start_builder {
            build_anchors(generation);
        }
        return;
    }
    let descriptor = current.descriptor.clone();
    let catalog_generation = current.catalog_generation;
    let columns = current.columns;
    drop(session);
    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            load_page(
                generation,
                catalog_generation,
                descriptor,
                columns,
                page,
                anchor,
            )
        })
        .await;
        match result {
            Ok(Ok(loaded)) => commit_page(loaded),
            Ok(Err(reason)) => fallback(generation, reason),
            Err(_) => fallback(generation, "page-worker-join"),
        }
    });
}

pub(crate) fn toggle_selection(index: i64, shift: bool) {
    if index < 0 {
        return;
    }
    update_selection(|selection, total| {
        if (index as u64) < total {
            selection.toggle(index as u64, shift);
        }
    });
}

pub(crate) fn select_all() {
    update_selection(|selection, _| {
        selection.all = true;
        selection.ranges.clear();
        selection.anchor = None;
    });
}

pub(crate) fn clear_selection() {
    update_selection(|selection, _| *selection = QuerySelection::default());
}

pub(crate) fn bulk_action(action: String) {
    let snapshot = {
        let session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
        session.as_ref().map(|current| {
            (
                current.generation,
                current.catalog_generation,
                current.descriptor.clone(),
                current.album_total,
                current.selection.clone(),
            )
        })
    };
    let Some((generation, catalog_generation, descriptor, total, selection)) = snapshot else {
        return;
    };
    if selection.count(total) == 0 {
        return;
    }
    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            selected_album_ids(
                generation,
                catalog_generation,
                &descriptor,
                total,
                &selection,
            )
        })
        .await;
        let ids = match result {
            Ok(Ok(ids)) => ids,
            _ => return,
        };
        if QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        let rows = tokio::task::spawn_blocking(move || {
            crate::local_bulk::resolve_album_ids_blocking(&ids)
        })
        .await
        .unwrap_or_default();
        if QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        if crate::local_bulk::apply(rows, &action).await {
            clear_selection_generation(generation);
        }
    });
}

fn open_query(
    generation: u64,
    query: LastQuery,
    mut descriptor: QueryDescriptor,
    columns: usize,
) -> Result<OpenedQuery, &'static str> {
    let (catalog, catalog_generation) = open_active()?;
    let sources = enabled_sources(&catalog)?;
    if sources.is_empty() && catalog.stats().map_err(|_| "source-counts")?.track_count > 0 {
        return Ok(OpenedQuery {
            generation,
            catalog_generation,
            query,
            descriptor,
            columns,
            album_total: 0,
            entry_total: 0,
            loaded: LoadedPage {
                generation,
                page: 0,
                wire: Vec::new(),
                anchors: Vec::new(),
                jumps: Vec::new(),
                query_time: std::time::Duration::ZERO,
                map_time: std::time::Duration::ZERO,
            },
            count_time: std::time::Duration::ZERO,
        });
    }
    descriptor = descriptor.with_sources(sources);
    let count_started = Instant::now();
    let album_total = catalog.count_albums(&descriptor).map_err(|_| "count")?;
    let entry_total = catalog
        .count_album_entries(&descriptor, columns)
        .map_err(|_| "entry-count")?;
    let count_time = count_started.elapsed();
    let loaded = load_page_from_catalog(
        &catalog,
        generation,
        &descriptor,
        columns,
        0,
        StreamAnchor::default(),
    )?;
    Ok(OpenedQuery {
        generation,
        catalog_generation,
        query,
        descriptor,
        columns,
        album_total,
        entry_total,
        loaded,
        count_time,
    })
}

fn activate_query(opened: OpenedQuery) {
    if QUERY_GENERATION.load(Ordering::Acquire) != opened.generation {
        return;
    }
    let first_wire = Arc::new(opened.loaded.wire.clone());
    let albums = album_count(&first_wire);
    let mut cache = PageLru::default();
    cache.insert(
        0,
        CachedPage {
            wire: Arc::clone(&first_wire),
            albums,
        },
    );
    let mut anchors = BTreeMap::new();
    anchors.insert(0, StreamAnchor::default());
    for anchor in opened.loaded.anchors {
        anchors.insert(anchor.entry_index, anchor);
    }
    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = Some(NativeSession {
        generation: opened.generation,
        catalog_generation: opened.catalog_generation,
        query: opened.query,
        descriptor: opened.descriptor.clone(),
        columns: opened.columns,
        album_total: opened.album_total,
        entry_total: opened.entry_total,
        anchors,
        anchors_ready: opened.entry_total <= MAX_UNANCHORED_ENTRIES as u64,
        anchor_building: false,
        pending_pages: HashSet::new(),
        waiting_pages: HashSet::new(),
        cache,
        selection: QuerySelection::default(),
    });
    let generation = opened.generation;
    let total = opened.entry_total;
    let albums_total = opened.album_total;
    let count_time = opened.count_time;
    let query_time = opened.loaded.query_time;
    let map_time = opened.loaded.map_time;
    let initial_jumps = jumps_json(opened.loaded.jumps);
    ui(move |mut bridge| {
        cpp_reset(generation, total, albums_total);
        let bytes = publish_page_now(generation, 0, &first_wire);
        bridge.as_mut().set_local_albums_native_active(true);
        bridge
            .as_mut()
            .set_local_albums_native_total(albums_total.min(i64::MAX as u64) as i64);
        bridge.as_mut().set_local_albums_native_selected_count(0);
        bridge
            .as_mut()
            .set_local_albums_native_jumps_json(QString::from(initial_jumps.as_str()));
        bridge
            .as_mut()
            .set_local_albums_native_error(QString::default());
        bridge.as_mut().set_local_albums_loading(false);
        log::info!(
            "[local-catalog] phase=albums-native generation={generation} albums={albums_total} entries={total} first_entries={} first_albums={albums} json_bytes={bytes} count={count_time:?} query={query_time:?} map={map_time:?}",
            first_wire.len()
        );
    });
    if opened.entry_total > MAX_UNANCHORED_ENTRIES as u64
        || opened.descriptor.group() == TrackGroup::Name
    {
        build_anchors(generation);
    }
}

fn publish_empty(generation: u64) {
    ui(move |mut bridge| {
        cpp_reset(generation, 0, 0);
        bridge.as_mut().set_local_albums_native_active(true);
        bridge.as_mut().set_local_albums_native_total(0);
        bridge.as_mut().set_local_albums_native_selected_count(0);
        bridge
            .as_mut()
            .set_local_albums_native_jumps_json(QString::from("[]"));
        bridge.as_mut().set_local_albums_loading(false);
    });
}

fn load_page(
    generation: u64,
    catalog_generation: u64,
    descriptor: QueryDescriptor,
    columns: usize,
    page: usize,
    anchor: StreamAnchor,
) -> Result<LoadedPage, &'static str> {
    let (catalog, active_generation) = open_active()?;
    if active_generation != catalog_generation {
        return Err("catalog-generation-changed");
    }
    load_page_from_catalog(&catalog, generation, &descriptor, columns, page, anchor)
}

fn load_page_from_catalog(
    catalog: &qbz_local_catalog::Catalog,
    generation: u64,
    descriptor: &QueryDescriptor,
    columns: usize,
    page: usize,
    anchor: StreamAnchor,
) -> Result<LoadedPage, &'static str> {
    let target = page.saturating_mul(PAGE_ENTRIES);
    if anchor.entry_index > target {
        return Err("anchor-after-target");
    }
    let started = Instant::now();
    let mapped = scan_stream(
        catalog,
        generation,
        descriptor,
        columns,
        anchor,
        target,
        target.saturating_add(PAGE_ENTRIES),
        true,
    )?;
    Ok(LoadedPage {
        generation,
        page,
        wire: mapped.wire,
        anchors: mapped.anchors,
        jumps: mapped.jumps,
        query_time: started.elapsed(),
        map_time: mapped.map_time,
    })
}

struct ScanResult {
    wire: Vec<NativeEntry>,
    anchors: Vec<StreamAnchor>,
    jumps: Vec<NativeJump>,
    final_state: StreamAnchor,
    map_time: std::time::Duration,
}

fn scan_stream(
    catalog: &qbz_local_catalog::Catalog,
    generation: u64,
    descriptor: &QueryDescriptor,
    columns: usize,
    mut state: StreamAnchor,
    collect_start: usize,
    stop_entry: usize,
    collect: bool,
) -> Result<ScanResult, &'static str> {
    let mut wire = Vec::new();
    let mut anchors = Vec::new();
    let mut jumps = Vec::new();
    let mut map_time = std::time::Duration::ZERO;
    while state.entry_index < stop_entry && !state.done {
        if QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return Err("superseded");
        }
        let page = catalog
            .query_albums(descriptor, state.cursor.as_ref(), FLAT_QUERY_ROWS)
            .map_err(|_| "album-query")?;
        for record in page.rows {
            let group_key = album_group_key(&record, descriptor.group());
            if descriptor.group() != TrackGroup::Off
                && state.previous_group.as_deref() != Some(group_key.as_str())
            {
                emit_pending(
                    &mut state,
                    descriptor,
                    collect_start,
                    stop_entry,
                    collect,
                    &mut wire,
                    &mut map_time,
                );
                state.previous_group = Some(group_key.clone());
                let label = album_group_label(&record, descriptor.group());
                if descriptor.group() == TrackGroup::Name {
                    jumps.push(NativeJump {
                        letter: label.clone(),
                        index: state.entry_index,
                    });
                }
                emit_header(
                    &mut state,
                    label,
                    collect_start,
                    stop_entry,
                    collect,
                    &mut wire,
                );
            }
            let index = state.flat_index;
            state.flat_index = state.flat_index.saturating_add(1);
            state.pending.push(IndexedAlbum { index, record });
            if state.pending.len() == columns {
                emit_pending(
                    &mut state,
                    descriptor,
                    collect_start,
                    stop_entry,
                    collect,
                    &mut wire,
                    &mut map_time,
                );
            }
        }
        if let Some(cursor) = page.next_cursor {
            state.cursor = Some(cursor);
            anchors.push(state.clone());
        } else {
            emit_pending(
                &mut state,
                descriptor,
                collect_start,
                stop_entry,
                collect,
                &mut wire,
                &mut map_time,
            );
            state.cursor = None;
            state.done = true;
        }
    }
    Ok(ScanResult {
        wire,
        anchors,
        jumps,
        final_state: state,
        map_time,
    })
}

fn emit_header(
    state: &mut StreamAnchor,
    label: String,
    collect_start: usize,
    collect_end: usize,
    collect: bool,
    wire: &mut Vec<NativeEntry>,
) {
    if collect && (collect_start..collect_end).contains(&state.entry_index) {
        wire.push(NativeEntry {
            t: 0,
            label,
            base: state.flat_index,
            items: Vec::new(),
        });
    }
    state.entry_index = state.entry_index.saturating_add(1);
}

fn emit_pending(
    state: &mut StreamAnchor,
    descriptor: &QueryDescriptor,
    collect_start: usize,
    collect_end: usize,
    collect: bool,
    wire: &mut Vec<NativeEntry>,
    map_time: &mut std::time::Duration,
) {
    if state.pending.is_empty() {
        return;
    }
    if collect && (collect_start..collect_end).contains(&state.entry_index) {
        let started = Instant::now();
        let items = state
            .pending
            .iter()
            .map(|album| map_album(&album.record, album.index))
            .collect();
        *map_time += started.elapsed();
        wire.push(NativeEntry {
            t: 1,
            label: String::new(),
            base: state.pending[0].index,
            items,
        });
    }
    state.pending.clear();
    state.entry_index = state.entry_index.saturating_add(1);
    let _ = descriptor;
}

fn commit_page(loaded: LoadedPage) {
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let Some(current) = session
        .as_mut()
        .filter(|state| state.generation == loaded.generation)
    else {
        return;
    };
    current.pending_pages.remove(&loaded.page);
    for anchor in loaded.anchors {
        current.anchors.insert(anchor.entry_index, anchor);
    }
    let wire = Arc::new(loaded.wire);
    let albums = album_count(&wire);
    current.cache.insert(
        loaded.page,
        CachedPage {
            wire: Arc::clone(&wire),
            albums,
        },
    );
    let resident = current.cache.resident_albums;
    let evictions = current.cache.evictions;
    let entries = wire.len();
    drop(session);
    publish_page(
        loaded.generation,
        loaded.page,
        wire,
        false,
        resident,
        evictions,
    );
    log::info!(
        "[local-catalog] phase=albums-page generation={} page={} entries={} albums={} query={:?} map={:?} resident_albums={} evictions={}",
        loaded.generation,
        loaded.page,
        entries,
        albums,
        loaded.query_time,
        loaded.map_time,
        resident,
        evictions
    );
}

fn publish_page(
    generation: u64,
    page: usize,
    wire: Arc<Vec<NativeEntry>>,
    cache_hit: bool,
    resident: usize,
    evictions: u64,
) {
    ui(move |_| {
        let bytes = publish_page_now(generation, page, &wire);
        log::info!(
            "[local-catalog] phase=albums-publish generation={generation} page={page} entries={} albums={} json_bytes={bytes} cache_hit={cache_hit} resident_albums={resident} evictions={evictions}",
            wire.len(),
            album_count(&wire)
        );
    });
}

fn publish_page_now(generation: u64, page: usize, wire: &[NativeEntry]) -> usize {
    let json = serde_json::to_string(wire).unwrap_or_else(|_| "[]".to_string());
    let bytes = json.len();
    let Ok(json) = CString::new(json) else {
        return 0;
    };
    // SAFETY: C++ copies the bounded page synchronously.
    unsafe { qbz_local_albums_apply_page(generation as i32, page as i32, json.as_ptr()) };
    bytes
}

fn build_anchors(generation: u64) {
    let (descriptor, catalog_generation, columns, entry_total) = {
        let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
        let Some(current) = session
            .as_mut()
            .filter(|state| state.generation == generation)
        else {
            return;
        };
        current.anchor_building = true;
        (
            current.descriptor.clone(),
            current.catalog_generation,
            current.columns,
            current.entry_total,
        )
    };
    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let (catalog, active_generation) = open_active()?;
            if active_generation != catalog_generation {
                return Err("catalog-generation-changed");
            }
            let started = Instant::now();
            let scan = scan_stream(
                &catalog,
                generation,
                &descriptor,
                columns,
                StreamAnchor::default(),
                usize::MAX,
                entry_total.min(usize::MAX as u64) as usize,
                false,
            )?;
            Ok((
                scan.anchors,
                scan.jumps,
                scan.final_state,
                started.elapsed(),
            ))
        })
        .await;
        match result {
            Ok(Ok((anchors, jumps, final_state, elapsed))) => {
                finish_anchors(generation, anchors, jumps, final_state, elapsed)
            }
            Ok(Err("superseded")) => {}
            Ok(Err(reason)) => fallback(generation, reason),
            Err(_) => fallback(generation, "anchor-worker-join"),
        }
    });
}

fn finish_anchors(
    generation: u64,
    anchors: Vec<StreamAnchor>,
    jumps: Vec<NativeJump>,
    final_state: StreamAnchor,
    elapsed: std::time::Duration,
) {
    let waiting = {
        let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
        let Some(current) = session
            .as_mut()
            .filter(|state| state.generation == generation)
        else {
            return;
        };
        let count = anchors.len();
        for anchor in anchors {
            current.anchors.insert(anchor.entry_index, anchor);
        }
        current.anchors.insert(final_state.entry_index, final_state);
        current.anchors_ready = true;
        current.anchor_building = false;
        log::info!(
            "[local-catalog] phase=albums-anchors generation={generation} anchors={count} elapsed={elapsed:?}"
        );
        current.waiting_pages.drain().collect::<Vec<_>>()
    };
    let jumps = jumps_json(jumps);
    ui(move |mut bridge| {
        bridge
            .as_mut()
            .set_local_albums_native_jumps_json(QString::from(jumps.as_str()));
    });
    for page in waiting {
        request_page(page as i32, generation as i32);
    }
}

fn selected_album_ids(
    generation: u64,
    catalog_generation: u64,
    descriptor: &QueryDescriptor,
    total: u64,
    selection: &QuerySelection,
) -> Result<Vec<String>, &'static str> {
    let (catalog, active_generation) = open_active()?;
    if active_generation != catalog_generation {
        return Err("catalog-generation-changed");
    }
    let mut cursor = None;
    let mut index = 0_u64;
    let mut ids = Vec::with_capacity(selection.count(total).min(16_384) as usize);
    loop {
        if QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return Err("superseded");
        }
        let page = catalog
            .query_albums(descriptor, cursor.as_ref(), FLAT_QUERY_ROWS)
            .map_err(|_| "action-query")?;
        for record in page.rows {
            if selection.contains(index) {
                ids.push(album_action_id(&record));
            }
            index = index.saturating_add(1);
        }
        let Some(next) = page.next_cursor else {
            break;
        };
        cursor = Some(next);
    }
    Ok(ids)
}

fn descriptor(search: String, sort: &str, group: &str, filter_json: &str) -> QueryDescriptor {
    let filter: serde_json::Value = serde_json::from_str(filter_json).unwrap_or_default();
    let on = |key: &str| filter.get(key).and_then(|value| value.as_bool()) == Some(true);
    let formats = ["flac", "alac", "ape", "wav", "mp3", "aac"]
        .into_iter()
        .filter(|key| on(key))
        .map(str::to_string)
        .collect();
    let qualities = ["dsd", "hires", "cd", "lossy"]
        .into_iter()
        .filter(|key| on(key))
        .map(str::to_string)
        .collect();
    let buckets = ["local", "offline", "plex", "jellyfin", "subsonic"]
        .into_iter()
        .filter(|key| on(key))
        .map(str::to_string)
        .collect();
    QueryDescriptor::albums()
        .with_search(search)
        .with_sort(parse_sort(sort))
        .with_group(parse_group(group))
        .with_formats(formats)
        .including_other_formats(on("other"))
        .with_quality_tiers(qualities)
        .with_source_buckets(buckets)
}

fn map_album(record: &AlbumRecord, index: usize) -> NativeAlbum {
    let id = album_action_id(record);
    let art_key = format!("catalog-album:{}", record.edition_id);
    if !record.artwork_token.is_empty() {
        if let Some(reference) =
            crate::local_rows::art_token(Some(&record.artwork_source), &record.artwork_token)
        {
            crate::local_state::with_art(|art| {
                art.insert(art_key.clone(), reference);
            });
        }
    }
    let format = audio_format(&record.format);
    let names = [record.artist.as_str()]
        .into_iter()
        .chain(record.all_artists.split(','))
        .collect::<Vec<_>>();
    let aliases = crate::local_artist_match::build_artist_family_aliases(&names);
    let artists = crate::local_artist_match::album_credit_names(
        &record.artist,
        &record.all_artists,
        &aliases,
    );
    let source = badge_source(Some(record.source_raw_or_kind()));
    let source_raw = badge_source_raw(Some(record.source_raw_or_kind()));
    let sources = catalog_album_sources(record);
    let variant_sources = if sources.is_empty() {
        vec![if source_raw.is_empty() {
            source.clone()
        } else {
            source_raw.clone()
        }]
    } else {
        sources.clone()
    };
    let media_variants = variant_sources
        .into_iter()
        .map(|variant_source| AlbumMediaVariant {
            quality_tier: record.quality_tier.clone(),
            format: record.format.to_ascii_uppercase(),
            source: variant_source,
        })
        .collect();
    let favoriteable = album_favorite_source(&sources).is_some();
    NativeAlbum {
        row: AlbumRow {
            is_favorite: favoriteable && crate::library_qt::is_local_favorite("album", &id),
            favoriteable,
            id,
            title: record.title.clone(),
            artist: record.artist.clone(),
            all_artists: record.all_artists.clone(),
            artists,
            year: record.year.map(|year| year.to_string()).unwrap_or_default(),
            track_count: record.track_count,
            duration: total_duration(record.total_duration_ms / 1_000),
            quality_tier: record.quality_tier.clone(),
            quality_detail: detail_of(
                &format,
                record.bit_depth,
                record.sample_rate_hz.unwrap_or(0) as f64,
            ),
            format: record.format.to_ascii_uppercase(),
            media_variants,
            genres: Vec::new(),
            art_key,
            source,
            sources,
            source_raw,
            directory_path: record.directory_path.clone(),
            folder_count: record.folder_count,
        },
        native_index: index,
        art_path: String::new(),
    }
}

trait AlbumSourceWord {
    fn source_raw_or_kind(&self) -> &str;
}

impl AlbumSourceWord for AlbumRecord {
    fn source_raw_or_kind(&self) -> &str {
        if self.source_raw.trim().is_empty() {
            self.source.as_str()
        } else {
            &self.source_raw
        }
    }
}

fn album_action_id(record: &AlbumRecord) -> String {
    let native = record.native_album_id.trim();
    match record.source {
        SourceKind::Plex if native.starts_with("plex:") => native.to_string(),
        SourceKind::Plex => format!("plex:album:{native}"),
        SourceKind::Jellyfin if native.starts_with("jellyfin:") => native.to_string(),
        SourceKind::Jellyfin => format!("jellyfin:{native}"),
        SourceKind::Subsonic if native.starts_with("subsonic:") => native.to_string(),
        SourceKind::Subsonic => format!("subsonic:{native}"),
        _ => native.to_string(),
    }
}

fn album_group_key(record: &AlbumRecord, group: TrackGroup) -> String {
    match group {
        TrackGroup::Artist => qbz_local_catalog::normalize_sort_key(&record.artist),
        TrackGroup::Name | TrackGroup::Album => {
            qbz_local_catalog::normalize_sort_key(&record.title)
                .chars()
                .next()
                .unwrap_or('#')
                .to_string()
        }
        TrackGroup::Off => String::new(),
    }
}

fn album_group_label(record: &AlbumRecord, group: TrackGroup) -> String {
    match group {
        TrackGroup::Artist => record.artist.clone(),
        TrackGroup::Name | TrackGroup::Album => album_group_key(record, group).to_uppercase(),
        TrackGroup::Off => String::new(),
    }
}

fn enabled_sources(catalog: &qbz_local_catalog::Catalog) -> Result<Vec<SourceKey>, &'static str> {
    let stats = catalog.stats().map_err(|_| "source-counts")?;
    let plex = crate::local_plex::is_configured();
    let remote = crate::media_servers_qt::configured_words();
    Ok(stats
        .source_counts
        .into_iter()
        .map(|(source, _)| source)
        .filter(|source| match source.source {
            SourceKind::Local | SourceKind::Offline => true,
            SourceKind::Plex => plex,
            SourceKind::Jellyfin => remote.iter().any(|word| *word == "jellyfin"),
            SourceKind::Subsonic => remote.iter().any(|word| *word == "subsonic"),
        })
        .collect())
}

fn open_active() -> Result<(qbz_local_catalog::Catalog, u64), &'static str> {
    let locations = crate::local_catalog_qt::locations().ok_or("missing-data-directory")?;
    match BootstrapLayout::new(locations.catalog_dir).open_active() {
        ActiveCatalog::Ready { catalog, manifest } => Ok((catalog, manifest.active_generation)),
        ActiveCatalog::Fallback(_) => Err("catalog-not-ready"),
    }
}

fn nearest_anchor(anchors: &BTreeMap<usize, StreamAnchor>, target: usize) -> StreamAnchor {
    anchors
        .range(..=target)
        .next_back()
        .map(|(_, anchor)| anchor.clone())
        .unwrap_or_default()
}

fn update_selection(update: impl FnOnce(&mut QuerySelection, u64)) {
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let Some(current) = session.as_mut() else {
        return;
    };
    update(&mut current.selection, current.album_total);
    let generation = current.generation;
    let count = current.selection.count(current.album_total);
    let json = selection_json(&current.selection);
    drop(session);
    ui(move |mut bridge| {
        bridge
            .as_mut()
            .set_local_albums_native_selected_count(count.min(i64::MAX as u64) as i64);
        cpp_set_selection(generation, &json, count);
    });
}

fn clear_selection_generation(generation: u64) {
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let Some(current) = session
        .as_mut()
        .filter(|state| state.generation == generation)
    else {
        return;
    };
    current.selection = QuerySelection::default();
    drop(session);
    ui(move |mut bridge| {
        bridge.as_mut().set_local_albums_native_selected_count(0);
        cpp_set_selection(generation, &selection_json(&QuerySelection::default()), 0);
    });
}

fn set_loading() {
    ui(|mut bridge| {
        bridge.as_mut().set_local_albums_loading(true);
        bridge.as_mut().set_local_albums_error(QString::default());
        bridge
            .as_mut()
            .set_local_albums_native_error(QString::default());
    });
}

fn fallback(generation: u64, reason: &'static str) {
    if QUERY_GENERATION.load(Ordering::Acquire) != generation {
        return;
    }
    if reason != "catalog-not-ready" {
        SESSION_FAILED.store(true, Ordering::Release);
    }
    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = None;
    log::warn!("[local-catalog] phase=albums-fallback generation={generation} reason={reason}");
    ui(move |mut bridge| {
        bridge.as_mut().set_local_albums_native_active(false);
        bridge.as_mut().set_local_albums_native_total(0);
        bridge.as_mut().set_local_albums_native_selected_count(0);
        bridge
            .as_mut()
            .set_local_albums_native_jumps_json(QString::from("[]"));
        bridge
            .as_mut()
            .set_local_albums_native_error(QString::from(reason));
        crate::local_bridge_ops::load_albums_legacy();
    });
}

fn deactivate_for_legacy(reason: &'static str) {
    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = None;
    ui(move |mut bridge| {
        bridge.as_mut().set_local_albums_native_active(false);
        bridge.as_mut().set_local_albums_native_total(0);
        bridge.as_mut().set_local_albums_native_selected_count(0);
        bridge
            .as_mut()
            .set_local_albums_native_jumps_json(QString::from("[]"));
        bridge
            .as_mut()
            .set_local_albums_native_error(QString::from(reason));
    });
}

fn parse_sort(value: &str) -> TrackSort {
    match value {
        "artist-desc" => TrackSort::ArtistDesc,
        "title-asc" => TrackSort::TitleAsc,
        "title-desc" => TrackSort::TitleDesc,
        "year-asc" => TrackSort::YearAsc,
        "year-desc" => TrackSort::YearDesc,
        _ => TrackSort::ArtistAsc,
    }
}

fn parse_group(value: &str) -> TrackGroup {
    match value {
        "alpha" => TrackGroup::Name,
        "artist" => TrackGroup::Artist,
        _ => TrackGroup::Off,
    }
}

fn audio_format(value: &str) -> AudioFormat {
    match value.to_ascii_lowercase().as_str() {
        "flac" => AudioFormat::Flac,
        "alac" | "m4a" => AudioFormat::Alac,
        "wav" | "wave" => AudioFormat::Wav,
        "aiff" | "aif" => AudioFormat::Aiff,
        "ape" => AudioFormat::Ape,
        "mp3" => AudioFormat::Mp3,
        "dsd" | "dsf" | "dff" => AudioFormat::Dsd,
        _ => AudioFormat::Unknown,
    }
}

fn album_count(wire: &[NativeEntry]) -> usize {
    wire.iter().map(|entry| entry.items.len()).sum()
}

fn jumps_json(jumps: Vec<NativeJump>) -> String {
    serde_json::to_string(&jumps).unwrap_or_else(|_| "[]".to_string())
}

fn selection_json(selection: &QuerySelection) -> String {
    serde_json::json!({ "all": selection.all, "ranges": selection.ranges }).to_string()
}

fn cpp_reset(generation: u64, total: u64, albums: u64) {
    // SAFETY: primitive values only; C++ applies synchronously on the Qt thread.
    unsafe {
        qbz_local_albums_reset(
            generation as i32,
            total.min(i64::MAX as u64) as i64,
            albums.min(i64::MAX as u64) as i64,
        )
    };
}

fn cpp_set_selection(generation: u64, json: &str, count: u64) {
    let Ok(json) = CString::new(json) else {
        return;
    };
    // SAFETY: C++ copies the bounded interval document synchronously.
    unsafe {
        qbz_local_albums_set_selection(
            generation as i32,
            json.as_ptr(),
            count.min(i64::MAX as u64) as i64,
        )
    };
}

fn next_generation() -> u64 {
    let mut generation = QUERY_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    if generation == 0 {
        QUERY_GENERATION.store(1, Ordering::Release);
        generation = 1;
    }
    generation
}

fn add_interval(ranges: &mut Vec<(u64, u64)>, first: u64, last: u64) {
    let mut merged = (first.min(last), first.max(last));
    let mut out = Vec::with_capacity(ranges.len() + 1);
    for &(start, end) in ranges.iter() {
        if end.saturating_add(1) < merged.0 {
            out.push((start, end));
        } else if merged.1.saturating_add(1) < start {
            out.push(merged);
            merged = (start, end);
        } else {
            merged.0 = merged.0.min(start);
            merged.1 = merged.1.max(end);
        }
    }
    out.push(merged);
    *ranges = out;
}

fn remove_interval(ranges: &mut Vec<(u64, u64)>, first: u64, last: u64) {
    let (first, last) = (first.min(last), first.max(last));
    let mut out = Vec::with_capacity(ranges.len() + 1);
    for &(start, end) in ranges.iter() {
        if end < first || start > last {
            out.push((start, end));
            continue;
        }
        if start < first {
            out.push((start, first - 1));
        }
        if end > last {
            out.push((last + 1, end));
        }
    }
    *ranges = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_page_cache_is_bounded_by_resident_album_rows() {
        let mut cache = PageLru::default();
        for page in 0..25 {
            cache.insert(
                page,
                CachedPage {
                    wire: Arc::new(Vec::new()),
                    albums: 100,
                },
            );
        }
        assert_eq!(cache.resident_albums, MAX_RESIDENT_ALBUMS);
        assert_eq!(cache.entries.len(), 20);
        assert_eq!(cache.evictions, 5);
        assert!(cache.get(0).is_none());
        assert!(cache.get(24).is_some());
    }

    #[test]
    fn album_selection_uses_bounded_intervals_and_descriptor_all() {
        let mut selection = QuerySelection::default();
        selection.toggle(10, false);
        selection.toggle(50_000, true);
        assert_eq!(selection.ranges, vec![(10, 50_000)]);
        assert_eq!(selection.count(1_000_000), 49_991);

        selection = QuerySelection {
            all: true,
            ..QuerySelection::default()
        };
        selection.toggle(25, false);
        selection.toggle(26, false);
        assert_eq!(selection.ranges, vec![(25, 26)]);
        assert_eq!(selection.count(1_000_000), 999_998);
    }

    #[test]
    fn superseded_album_generation_cannot_publish_as_current() {
        QUERY_GENERATION.store(70, Ordering::Release);
        let old = QUERY_GENERATION.load(Ordering::Acquire);
        let current = next_generation();
        assert_eq!(current, 71);
        assert_ne!(QUERY_GENERATION.load(Ordering::Acquire), old);
    }

    #[test]
    fn native_album_page_mapping_and_transport_are_bounded() {
        let page = (0..PAGE_ENTRIES)
            .map(|index| AlbumRecord {
                edition_id: index as i64 + 1,
                source: SourceKind::Local,
                native_album_id: format!("album-{index:05}"),
                source_raw: "local".to_string(),
                source_words: vec!["local".to_string()],
                title: format!("Album {index:05}"),
                artist: format!("Artist {:03}", index % 17),
                all_artists: format!("Artist {:03}", index % 17),
                year: Some(1980 + (index % 45) as u32),
                track_count: 10,
                total_duration_ms: 1_800_000,
                quality_tier: "hires".to_string(),
                format: "flac".to_string(),
                bit_depth: Some(24),
                sample_rate_hz: Some(96_000),
                artwork_source: String::new(),
                artwork_token: String::new(),
                directory_path: format!("/fixture/album-{index:05}"),
                folder_count: 1,
                added_at: index as i64,
            })
            .collect::<Vec<_>>();
        let map_started = Instant::now();
        let wire = page
            .iter()
            .enumerate()
            .map(|(index, row)| NativeEntry {
                t: 1,
                label: String::new(),
                base: index,
                items: vec![map_album(row, index)],
            })
            .collect::<Vec<_>>();
        let map_time = map_started.elapsed();
        let serialize_started = Instant::now();
        let bytes = serde_json::to_vec(&wire).unwrap().len();
        let serialize_time = serialize_started.elapsed();
        assert!(bytes < 256 * 1024, "bounded album page was {bytes} bytes");
        println!(
            "F1_ALBUMS_TRANSPORT first_entries={} first_albums={} json_bytes={bytes} map_ms={:.3} serialize_ms={:.3} qml_album_json_bytes=0",
            wire.len(),
            page.len(),
            map_time.as_secs_f64() * 1_000.0,
            serialize_time.as_secs_f64() * 1_000.0,
        );
    }
}
