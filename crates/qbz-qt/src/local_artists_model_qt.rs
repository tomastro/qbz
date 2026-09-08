//! Native paged Artists rail and indexed artist-album pane (phase F2).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::{c_char, CString};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cxx_qt_lib::QString;
use qbz_library::AudioFormat;
use qbz_local_catalog::{
    ActiveCatalog, AlbumCursor, AlbumRecord, ArtistCursor, ArtistRecord, BootstrapLayout,
    QueryDescriptor, SourceKey, SourceKind,
};
use serde::Serialize;

use crate::local_bridge::ui;
use crate::local_rows::{
    badge_source, badge_source_raw, catalog_album_sources, detail_of, total_duration,
    AlbumMediaVariant, AlbumRow, ArtistRow,
};

const PAGE_ENTRIES: usize = 100;
const QUERY_ROWS: usize = 500;
const MAX_RESIDENT_ARTISTS: usize = 2_000;
const MAX_RESIDENT_ALBUMS: usize = 2_000;
const MAX_UNANCHORED_ENTRIES: usize = 500;
const MAX_COLUMNS: usize = 20;

static QUERY_GENERATION: AtomicU64 = AtomicU64::new(0);
static DETAIL_GENERATION: AtomicU64 = AtomicU64::new(0);
static SESSION_FAILED: AtomicBool = AtomicBool::new(false);
static SESSION: Mutex<Option<ArtistSession>> = Mutex::new(None);
static DETAIL_SESSION: Mutex<Option<DetailSession>> = Mutex::new(None);
static LAST_SEARCH: Mutex<Option<String>> = Mutex::new(None);

unsafe extern "C" {
    fn qbz_local_artists_register_qml_type();
    fn qbz_local_artists_reset(generation: i32, total_count: i64, artist_total: i64);
    fn qbz_local_artists_apply_page(generation: i32, page: i32, json: *const c_char) -> bool;
    fn qbz_local_artist_albums_reset(generation: i32, total_count: i64, album_total: i64);
    fn qbz_local_artist_albums_apply_page(generation: i32, page: i32, json: *const c_char) -> bool;
}

#[derive(Clone, Default)]
struct ArtistAnchor {
    entry_index: usize,
    artist_index: usize,
    cursor: Option<ArtistCursor>,
    previous_group: Option<String>,
    done: bool,
}

#[derive(Clone)]
struct CachedArtistPage {
    wire: Arc<Vec<ArtistEntry>>,
    artists: usize,
}

struct ArtistSession {
    generation: u64,
    catalog_generation: u64,
    descriptor: QueryDescriptor,
    entry_total: u64,
    anchors: BTreeMap<usize, ArtistAnchor>,
    anchors_ready: bool,
    anchor_building: bool,
    pending_pages: HashSet<usize>,
    waiting_pages: HashSet<usize>,
    cache: ArtistPageLru,
}

#[derive(Clone, Serialize)]
struct ArtistEntry {
    t: u8,
    label: String,
    base: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<NativeArtist>,
}

#[derive(Clone, Serialize)]
struct NativeArtist {
    #[serde(flatten)]
    row: ArtistRow,
    #[serde(rename = "artistKey")]
    artist_key: String,
    #[serde(rename = "artPath")]
    art_path: String,
}

#[derive(Clone, Serialize)]
struct NativeJump {
    letter: String,
    index: usize,
}

#[derive(Default)]
struct ArtistPageLru {
    entries: HashMap<usize, CachedArtistPage>,
    order: VecDeque<usize>,
    resident_artists: usize,
    evictions: u64,
}

impl ArtistPageLru {
    fn get(&mut self, page: usize) -> Option<&CachedArtistPage> {
        if !self.entries.contains_key(&page) {
            return None;
        }
        self.touch(page);
        self.entries.get(&page)
    }

    fn insert(&mut self, page: usize, value: CachedArtistPage) {
        if let Some(previous) = self.entries.insert(page, value) {
            self.resident_artists = self.resident_artists.saturating_sub(previous.artists);
        }
        self.resident_artists += self.entries[&page].artists;
        self.touch(page);
        while self.resident_artists > MAX_RESIDENT_ARTISTS && self.entries.len() > 1 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.resident_artists = self.resident_artists.saturating_sub(removed.artists);
                self.evictions += 1;
            }
        }
    }

    fn touch(&mut self, page: usize) {
        self.order.retain(|candidate| *candidate != page);
        self.order.push_back(page);
    }
}

struct OpenedArtists {
    generation: u64,
    catalog_generation: u64,
    descriptor: QueryDescriptor,
    artist_total: u64,
    entry_total: u64,
    loaded: LoadedArtists,
    count_time: std::time::Duration,
}

struct LoadedArtists {
    generation: u64,
    page: usize,
    wire: Vec<ArtistEntry>,
    anchors: Vec<ArtistAnchor>,
    jumps: Vec<NativeJump>,
    query_time: std::time::Duration,
    map_time: std::time::Duration,
}

#[derive(Clone, Default)]
struct DetailAnchor {
    entry_index: usize,
    album_index: usize,
    cursor: Option<AlbumCursor>,
    pending: Vec<IndexedAlbum>,
    done: bool,
}

#[derive(Clone)]
struct IndexedAlbum {
    index: usize,
    record: AlbumRecord,
}

#[derive(Clone)]
struct CachedDetailPage {
    wire: Arc<Vec<DetailEntry>>,
    albums: usize,
}

struct DetailSession {
    generation: u64,
    catalog_generation: u64,
    artist_key: String,
    sources: Vec<SourceKey>,
    columns: usize,
    entry_total: u64,
    anchors: BTreeMap<usize, DetailAnchor>,
    anchors_ready: bool,
    anchor_building: bool,
    pending_pages: HashSet<usize>,
    waiting_pages: HashSet<usize>,
    cache: DetailPageLru,
}

#[derive(Clone, Serialize)]
struct DetailEntry {
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

#[derive(Default)]
struct DetailPageLru {
    entries: HashMap<usize, CachedDetailPage>,
    order: VecDeque<usize>,
    resident_albums: usize,
    evictions: u64,
}

impl DetailPageLru {
    fn get(&mut self, page: usize) -> Option<&CachedDetailPage> {
        if !self.entries.contains_key(&page) {
            return None;
        }
        self.touch(page);
        self.entries.get(&page)
    }

    fn insert(&mut self, page: usize, value: CachedDetailPage) {
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

struct OpenedDetail {
    generation: u64,
    catalog_generation: u64,
    artist_key: String,
    sources: Vec<SourceKey>,
    columns: usize,
    album_total: u64,
    entry_total: u64,
    loaded: LoadedDetail,
    count_time: std::time::Duration,
}

struct LoadedDetail {
    generation: u64,
    page: usize,
    wire: Vec<DetailEntry>,
    anchors: Vec<DetailAnchor>,
    query_time: std::time::Duration,
    map_time: std::time::Duration,
}

pub(crate) fn register_qml_model() {
    // SAFETY: link anchor only; registration is a QCoreApplication startup hook.
    unsafe { qbz_local_artists_register_qml_type() };
}

pub(crate) fn requested() -> bool {
    if SESSION_FAILED.load(Ordering::Acquire) {
        return false;
    }
    !std::env::var("QBZ_LOCAL_CATALOG_ARTISTS")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
}

pub(crate) fn reset(search: String, sort: String, filter_json: String) -> bool {
    if !requested() {
        return false;
    }
    if sort != "name-asc"
        || crate::local_filter::MediaFilter::from_json(&filter_json)
            != crate::local_filter::MediaFilter::default()
    {
        deactivate_for_legacy("unsupported-artist-facets");
        return false;
    }
    *LAST_SEARCH
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(search.clone());
    if crate::local_library_qt::album_mode() != "folder" {
        deactivate_for_legacy("metadata-mode");
        return false;
    }
    let generation = next_generation(&QUERY_GENERATION);
    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = None;
    let detail_generation = next_generation(&DETAIL_GENERATION);
    *DETAIL_SESSION
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    set_loading();
    ui(move |mut bridge| {
        cpp_detail_reset(detail_generation, 0, 0);
        bridge.as_mut().set_local_artist_albums_native_total(0);
        bridge.as_mut().set_local_artist_albums_loading(false);
    });
    let descriptor = QueryDescriptor::artists().with_search(search);
    crate::spawn(async move {
        let result =
            tokio::task::spawn_blocking(move || open_artists(generation, descriptor)).await;
        match result {
            Ok(Ok(opened)) => activate_artists(opened),
            Ok(Err(reason)) => fallback(generation, reason),
            Err(_) => fallback(generation, "worker-join"),
        }
    });
    true
}

pub(crate) fn retry_last() -> bool {
    let search = LAST_SEARCH
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    search.is_some_and(|search| reset(search, "name-asc".to_string(), String::new()))
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
        let resident = current.cache.resident_artists;
        let evictions = current.cache.evictions;
        drop(session);
        publish_artist_page(generation, page, cached.wire, true, resident, evictions);
        return;
    }
    if !current.pending_pages.insert(page) {
        return;
    }
    let target = page.saturating_mul(PAGE_ENTRIES);
    let anchor = nearest_artist_anchor(&current.anchors, target);
    if target.saturating_sub(anchor.entry_index) > MAX_UNANCHORED_ENTRIES && !current.anchors_ready
    {
        current.pending_pages.remove(&page);
        current.waiting_pages.insert(page);
        let build = !current.anchor_building;
        drop(session);
        if build {
            build_artist_anchors(generation);
        }
        return;
    }
    let descriptor = current.descriptor.clone();
    let catalog_generation = current.catalog_generation;
    drop(session);
    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            load_artist_page(generation, catalog_generation, descriptor, page, anchor)
        })
        .await;
        match result {
            Ok(Ok(loaded)) => commit_artist_page(loaded),
            Ok(Err("superseded")) => {}
            Ok(Err(reason)) => fallback(generation, reason),
            Err(_) => fallback(generation, "page-worker-join"),
        }
    });
}

pub(crate) fn select_artist(name: String, columns: i32) {
    // Every exit of the detail query used to be silent except success and
    // fallback; a kiosk drill-down that showed "0 albums" for an artist the
    // desktop lists left nothing in the log to reason from (2026-09-07). One
    // line per tap, and one per silent exit.
    log::info!(
        "[local-catalog] phase=artist-select name={name:?} columns={columns} requested={} album_mode={}",
        requested(),
        crate::local_library_qt::album_mode()
    );
    if !requested() || crate::local_library_qt::album_mode() != "folder" {
        log::info!("[local-catalog] phase=artist-select-skip reason=not-native-or-metadata-mode");
        return;
    }
    let generation = next_generation(&DETAIL_GENERATION);
    let columns = usize::try_from(columns.max(1))
        .unwrap_or(1)
        .clamp(1, MAX_COLUMNS);
    let artist_key = qbz_local_catalog::normalize_artist_key(&name);
    *DETAIL_SESSION
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    ui(move |mut bridge| {
        cpp_detail_reset(generation, 0, 0);
        bridge.as_mut().set_local_artist_albums_native_total(0);
        bridge.as_mut().set_local_artist_albums_loading(true);
    });
    if artist_key.is_empty() {
        log::info!("[local-catalog] phase=artist-select-skip generation={generation} reason=empty-artist-key");
        ui(move |mut bridge| {
            cpp_detail_reset(generation, 0, 0);
            bridge.as_mut().set_local_artist_albums_native_total(0);
            bridge.as_mut().set_local_artist_albums_loading(false);
        });
        return;
    }
    crate::spawn(async move {
        let result =
            tokio::task::spawn_blocking(move || open_detail(generation, artist_key, columns)).await;
        match result {
            Ok(Ok(opened)) => activate_detail(opened),
            Ok(Err("superseded")) => {
                log::info!("[local-catalog] phase=artist-select-skip generation={generation} reason=superseded");
            }
            Ok(Err(reason)) => {
                log::info!("[local-catalog] phase=artist-select-fallback generation={generation} reason={reason}");
                fallback_detail(generation, reason)
            }
            Err(_) => fallback_detail(generation, "detail-worker-join"),
        }
    });
}

pub(crate) fn request_detail_page(page: i32, generation: i32) {
    if page < 0 || generation <= 0 {
        return;
    }
    let page = page as usize;
    let generation = generation as u64;
    let mut session = DETAIL_SESSION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
        publish_detail_page(generation, page, cached.wire, true, resident, evictions);
        return;
    }
    if !current.pending_pages.insert(page) {
        return;
    }
    let target = page.saturating_mul(PAGE_ENTRIES);
    let anchor = nearest_detail_anchor(&current.anchors, target);
    if target.saturating_sub(anchor.entry_index) > MAX_UNANCHORED_ENTRIES && !current.anchors_ready
    {
        current.pending_pages.remove(&page);
        current.waiting_pages.insert(page);
        let build = !current.anchor_building;
        drop(session);
        if build {
            build_detail_anchors(generation);
        }
        return;
    }
    let catalog_generation = current.catalog_generation;
    let artist_key = current.artist_key.clone();
    let sources = current.sources.clone();
    let columns = current.columns;
    drop(session);
    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            load_detail_page(
                generation,
                catalog_generation,
                artist_key,
                sources,
                columns,
                page,
                anchor,
            )
        })
        .await;
        match result {
            Ok(Ok(loaded)) => commit_detail_page(loaded),
            Ok(Err("superseded")) => {}
            Ok(Err(reason)) => fallback_detail(generation, reason),
            Err(_) => fallback_detail(generation, "detail-page-worker-join"),
        }
    });
}

fn open_artists(
    generation: u64,
    mut descriptor: QueryDescriptor,
) -> Result<OpenedArtists, &'static str> {
    let (catalog, catalog_generation) = open_active()?;
    let (sources, none_enabled) = enabled_sources(&catalog)?;
    if none_enabled {
        return Ok(OpenedArtists {
            generation,
            catalog_generation,
            descriptor,
            artist_total: 0,
            entry_total: 0,
            loaded: LoadedArtists {
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
    let started = Instant::now();
    let artist_total = catalog
        .count_artists(&descriptor)
        .map_err(|_| "artist-count")?;
    let entry_total = catalog
        .count_artist_entries(&descriptor)
        .map_err(|_| "artist-entry-count")?;
    let count_time = started.elapsed();
    let loaded = load_artist_page_from_catalog(
        &catalog,
        generation,
        &descriptor,
        0,
        ArtistAnchor::default(),
    )?;
    Ok(OpenedArtists {
        generation,
        catalog_generation,
        descriptor,
        artist_total,
        entry_total,
        loaded,
        count_time,
    })
}

fn activate_artists(opened: OpenedArtists) {
    if QUERY_GENERATION.load(Ordering::Acquire) != opened.generation {
        return;
    }
    let wire = Arc::new(opened.loaded.wire.clone());
    let artists = artist_count(&wire);
    let unfiltered = opened.descriptor.search().is_empty();
    let mut cache = ArtistPageLru::default();
    cache.insert(
        0,
        CachedArtistPage {
            wire: Arc::clone(&wire),
            artists,
        },
    );
    let mut anchors = BTreeMap::new();
    anchors.insert(0, ArtistAnchor::default());
    for anchor in opened.loaded.anchors {
        anchors.insert(anchor.entry_index, anchor);
    }
    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = Some(ArtistSession {
        generation: opened.generation,
        catalog_generation: opened.catalog_generation,
        descriptor: opened.descriptor,
        entry_total: opened.entry_total,
        anchors,
        anchors_ready: opened.entry_total <= MAX_UNANCHORED_ENTRIES as u64,
        anchor_building: false,
        pending_pages: HashSet::new(),
        waiting_pages: HashSet::new(),
        cache,
    });
    let generation = opened.generation;
    let total = opened.entry_total;
    let artist_total = opened.artist_total;
    let jumps = jumps_json(opened.loaded.jumps);
    let count_time = opened.count_time;
    let query_time = opened.loaded.query_time;
    let map_time = opened.loaded.map_time;
    crate::local_state::state(|state| state.artists.clear());
    if unfiltered {
        crate::local_state::state(|state| {
            state.counts.artists = artist_total.min(i64::MAX as u64) as i64;
        });
        crate::local_bridge_ops::publish_counts();
    }
    ui(move |mut bridge| {
        if QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        cpp_artist_reset(generation, total, artist_total);
        let bytes = publish_artist_page_now(generation, 0, &wire);
        bridge.as_mut().set_local_artists_native_active(true);
        bridge.as_mut().set_local_artists_json(QString::from("[]"));
        bridge
            .as_mut()
            .set_local_artists_native_total(artist_total.min(i64::MAX as u64) as i64);
        bridge
            .as_mut()
            .set_local_artists_native_jumps_json(QString::from(jumps.as_str()));
        bridge
            .as_mut()
            .set_local_artists_native_error(QString::default());
        bridge.as_mut().set_local_artists_loading(false);
        log::info!(
            "[local-catalog] phase=artists-native generation={generation} artists={artist_total} entries={total} first_entries={} first_artists={artists} json_bytes={bytes} count={count_time:?} query={query_time:?} map={map_time:?}",
            wire.len()
        );
    });
    build_artist_anchors(generation);
}

fn load_artist_page(
    generation: u64,
    catalog_generation: u64,
    descriptor: QueryDescriptor,
    page: usize,
    anchor: ArtistAnchor,
) -> Result<LoadedArtists, &'static str> {
    let (catalog, active_generation) = open_active()?;
    if active_generation != catalog_generation {
        return Err("catalog-generation-changed");
    }
    load_artist_page_from_catalog(&catalog, generation, &descriptor, page, anchor)
}

fn load_artist_page_from_catalog(
    catalog: &qbz_local_catalog::Catalog,
    generation: u64,
    descriptor: &QueryDescriptor,
    page: usize,
    anchor: ArtistAnchor,
) -> Result<LoadedArtists, &'static str> {
    let target = page.saturating_mul(PAGE_ENTRIES);
    if anchor.entry_index > target {
        return Err("anchor-after-target");
    }
    let started = Instant::now();
    let scanned = scan_artists(
        catalog,
        generation,
        descriptor,
        anchor,
        target,
        target.saturating_add(PAGE_ENTRIES),
        true,
    )?;
    Ok(LoadedArtists {
        generation,
        page,
        wire: scanned.wire,
        anchors: scanned.anchors,
        jumps: scanned.jumps,
        query_time: started.elapsed(),
        map_time: scanned.map_time,
    })
}

struct ArtistScan {
    wire: Vec<ArtistEntry>,
    anchors: Vec<ArtistAnchor>,
    jumps: Vec<NativeJump>,
    final_state: ArtistAnchor,
    map_time: std::time::Duration,
}

fn scan_artists(
    catalog: &qbz_local_catalog::Catalog,
    generation: u64,
    descriptor: &QueryDescriptor,
    mut state: ArtistAnchor,
    collect_start: usize,
    stop_entry: usize,
    collect: bool,
) -> Result<ArtistScan, &'static str> {
    let mut wire = Vec::new();
    let mut anchors = Vec::new();
    let mut jumps = Vec::new();
    let mut map_time = std::time::Duration::ZERO;
    while state.entry_index < stop_entry && !state.done {
        if QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return Err("superseded");
        }
        let page = catalog
            .query_artists(descriptor, state.cursor.as_ref(), QUERY_ROWS)
            .map_err(|_| "artist-query")?;
        for record in page.rows {
            let group = artist_initial(&record.artist_key);
            if state.previous_group.as_deref() != Some(group.as_str()) {
                state.previous_group = Some(group.clone());
                jumps.push(NativeJump {
                    letter: group.clone(),
                    index: state.entry_index,
                });
                if collect && (collect_start..stop_entry).contains(&state.entry_index) {
                    wire.push(ArtistEntry {
                        t: 0,
                        label: group,
                        base: state.artist_index,
                        item: None,
                    });
                }
                state.entry_index = state.entry_index.saturating_add(1);
            }
            let index = state.artist_index;
            if collect && (collect_start..stop_entry).contains(&state.entry_index) {
                let started = Instant::now();
                let item = map_artist(&record);
                map_time += started.elapsed();
                wire.push(ArtistEntry {
                    t: 1,
                    label: String::new(),
                    base: index,
                    item: Some(item),
                });
            }
            state.artist_index = state.artist_index.saturating_add(1);
            state.entry_index = state.entry_index.saturating_add(1);
        }
        if let Some(cursor) = page.next_cursor {
            state.cursor = Some(cursor);
            anchors.push(state.clone());
        } else {
            state.cursor = None;
            state.done = true;
        }
    }
    Ok(ArtistScan {
        wire,
        anchors,
        jumps,
        final_state: state,
        map_time,
    })
}

fn commit_artist_page(loaded: LoadedArtists) {
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
    let artists = artist_count(&wire);
    current.cache.insert(
        loaded.page,
        CachedArtistPage {
            wire: Arc::clone(&wire),
            artists,
        },
    );
    let resident = current.cache.resident_artists;
    let evictions = current.cache.evictions;
    drop(session);
    publish_artist_page(
        loaded.generation,
        loaded.page,
        wire,
        false,
        resident,
        evictions,
    );
}

fn publish_artist_page(
    generation: u64,
    page: usize,
    wire: Arc<Vec<ArtistEntry>>,
    cache_hit: bool,
    resident: usize,
    evictions: u64,
) {
    ui(move |_| {
        let bytes = publish_artist_page_now(generation, page, &wire);
        log::info!(
            "[local-catalog] phase=artists-publish generation={generation} page={page} entries={} artists={} json_bytes={bytes} cache_hit={cache_hit} resident_artists={resident} evictions={evictions}",
            wire.len(),
            artist_count(&wire)
        );
    });
}

fn publish_artist_page_now(generation: u64, page: usize, wire: &[ArtistEntry]) -> usize {
    let json = serde_json::to_string(wire).unwrap_or_else(|_| "[]".to_string());
    let bytes = json.len();
    let Ok(json) = CString::new(json) else {
        return 0;
    };
    // SAFETY: C++ copies the bounded page synchronously.
    unsafe { qbz_local_artists_apply_page(generation as i32, page as i32, json.as_ptr()) };
    bytes
}

fn build_artist_anchors(generation: u64) {
    let (descriptor, catalog_generation, entry_total) = {
        let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
        let Some(current) = session
            .as_mut()
            .filter(|state| state.generation == generation)
        else {
            return;
        };
        if current.anchor_building || current.anchors_ready {
            return;
        }
        current.anchor_building = true;
        (
            current.descriptor.clone(),
            current.catalog_generation,
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
            let scan = scan_artists(
                &catalog,
                generation,
                &descriptor,
                ArtistAnchor::default(),
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
                finish_artist_anchors(generation, anchors, jumps, final_state, elapsed)
            }
            Ok(Err("superseded")) => {}
            Ok(Err(reason)) => fallback(generation, reason),
            Err(_) => fallback(generation, "anchor-worker-join"),
        }
    });
}

fn finish_artist_anchors(
    generation: u64,
    anchors: Vec<ArtistAnchor>,
    jumps: Vec<NativeJump>,
    final_state: ArtistAnchor,
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
            "[local-catalog] phase=artists-anchors generation={generation} anchors={count} elapsed={elapsed:?}"
        );
        current.waiting_pages.drain().collect::<Vec<_>>()
    };
    let jumps = jumps_json(jumps);
    ui(move |mut bridge| {
        bridge
            .as_mut()
            .set_local_artists_native_jumps_json(QString::from(jumps.as_str()));
    });
    for page in waiting {
        request_page(page as i32, generation as i32);
    }
}

fn open_detail(
    generation: u64,
    artist_key: String,
    columns: usize,
) -> Result<OpenedDetail, &'static str> {
    if DETAIL_GENERATION.load(Ordering::Acquire) != generation {
        return Err("superseded");
    }
    let (catalog, catalog_generation) = open_active()?;
    let (sources, none_enabled) = enabled_sources(&catalog)?;
    if none_enabled {
        log::info!("[local-catalog] phase=artist-select-skip generation={generation} reason=no-enabled-sources");
        return Ok(OpenedDetail {
            generation,
            catalog_generation,
            artist_key,
            sources,
            columns,
            album_total: 0,
            entry_total: 0,
            loaded: LoadedDetail {
                generation,
                page: 0,
                wire: Vec::new(),
                anchors: Vec::new(),
                query_time: std::time::Duration::ZERO,
                map_time: std::time::Duration::ZERO,
            },
            count_time: std::time::Duration::ZERO,
        });
    }
    let started = Instant::now();
    let album_total = catalog
        .count_artist_albums(&artist_key, &sources)
        .map_err(|_| "artist-album-count")?;
    let entry_total = album_total.div_ceil(columns as u64);
    let count_time = started.elapsed();
    let loaded = load_detail_page_from_catalog(
        &catalog,
        generation,
        &artist_key,
        &sources,
        columns,
        0,
        DetailAnchor::default(),
    )?;
    Ok(OpenedDetail {
        generation,
        catalog_generation,
        artist_key,
        sources,
        columns,
        album_total,
        entry_total,
        loaded,
        count_time,
    })
}

fn activate_detail(opened: OpenedDetail) {
    if DETAIL_GENERATION.load(Ordering::Acquire) != opened.generation {
        log::info!(
            "[local-catalog] phase=artist-select-skip generation={} reason=stale-before-activate",
            opened.generation
        );
        return;
    }
    let wire = Arc::new(opened.loaded.wire.clone());
    let albums = detail_album_count(&wire);
    let mut cache = DetailPageLru::default();
    cache.insert(
        0,
        CachedDetailPage {
            wire: Arc::clone(&wire),
            albums,
        },
    );
    let mut anchors = BTreeMap::new();
    anchors.insert(0, DetailAnchor::default());
    for anchor in opened.loaded.anchors {
        anchors.insert(anchor.entry_index, anchor);
    }
    *DETAIL_SESSION
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(DetailSession {
        generation: opened.generation,
        catalog_generation: opened.catalog_generation,
        artist_key: opened.artist_key,
        sources: opened.sources,
        columns: opened.columns,
        entry_total: opened.entry_total,
        anchors,
        anchors_ready: opened.entry_total <= MAX_UNANCHORED_ENTRIES as u64,
        anchor_building: false,
        pending_pages: HashSet::new(),
        waiting_pages: HashSet::new(),
        cache,
    });
    let generation = opened.generation;
    let entries = opened.entry_total;
    let total = opened.album_total;
    let count_time = opened.count_time;
    let query_time = opened.loaded.query_time;
    let map_time = opened.loaded.map_time;
    ui(move |mut bridge| {
        cpp_detail_reset(generation, entries, total);
        let bytes = publish_detail_page_now(generation, 0, &wire);
        bridge
            .as_mut()
            .set_local_artist_albums_native_total(total.min(i64::MAX as u64) as i64);
        bridge.as_mut().set_local_artist_albums_loading(false);
        log::info!(
            "[local-catalog] phase=artist-albums-native generation={generation} albums={total} entries={entries} first_entries={} first_albums={albums} json_bytes={bytes} count={count_time:?} query={query_time:?} map={map_time:?}",
            wire.len()
        );
    });
    if opened.entry_total > MAX_UNANCHORED_ENTRIES as u64 {
        build_detail_anchors(generation);
    }
}

fn load_detail_page(
    generation: u64,
    catalog_generation: u64,
    artist_key: String,
    sources: Vec<SourceKey>,
    columns: usize,
    page: usize,
    anchor: DetailAnchor,
) -> Result<LoadedDetail, &'static str> {
    let (catalog, active_generation) = open_active()?;
    if active_generation != catalog_generation {
        return Err("catalog-generation-changed");
    }
    load_detail_page_from_catalog(
        &catalog,
        generation,
        &artist_key,
        &sources,
        columns,
        page,
        anchor,
    )
}

fn load_detail_page_from_catalog(
    catalog: &qbz_local_catalog::Catalog,
    generation: u64,
    artist_key: &str,
    sources: &[SourceKey],
    columns: usize,
    page: usize,
    anchor: DetailAnchor,
) -> Result<LoadedDetail, &'static str> {
    let target = page.saturating_mul(PAGE_ENTRIES);
    if anchor.entry_index > target {
        return Err("detail-anchor-after-target");
    }
    let started = Instant::now();
    let scan = scan_detail(
        catalog,
        generation,
        artist_key,
        sources,
        columns,
        anchor,
        target,
        target.saturating_add(PAGE_ENTRIES),
        true,
    )?;
    Ok(LoadedDetail {
        generation,
        page,
        wire: scan.wire,
        anchors: scan.anchors,
        query_time: started.elapsed(),
        map_time: scan.map_time,
    })
}

struct DetailScan {
    wire: Vec<DetailEntry>,
    anchors: Vec<DetailAnchor>,
    final_state: DetailAnchor,
    map_time: std::time::Duration,
}

#[allow(clippy::too_many_arguments)]
fn scan_detail(
    catalog: &qbz_local_catalog::Catalog,
    generation: u64,
    artist_key: &str,
    sources: &[SourceKey],
    columns: usize,
    mut state: DetailAnchor,
    collect_start: usize,
    stop_entry: usize,
    collect: bool,
) -> Result<DetailScan, &'static str> {
    let mut wire = Vec::new();
    let mut anchors = Vec::new();
    let mut map_time = std::time::Duration::ZERO;
    while state.entry_index < stop_entry && !state.done {
        if DETAIL_GENERATION.load(Ordering::Acquire) != generation {
            return Err("superseded");
        }
        let page = catalog
            .query_artist_albums(artist_key, sources, state.cursor.as_ref(), QUERY_ROWS)
            .map_err(|_| "artist-album-query")?;
        for record in page.rows {
            let index = state.album_index;
            state.album_index = state.album_index.saturating_add(1);
            state.pending.push(IndexedAlbum { index, record });
            if state.pending.len() == columns {
                emit_detail_chunk(
                    &mut state,
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
            emit_detail_chunk(
                &mut state,
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
    Ok(DetailScan {
        wire,
        anchors,
        final_state: state,
        map_time,
    })
}

fn emit_detail_chunk(
    state: &mut DetailAnchor,
    collect_start: usize,
    collect_end: usize,
    collect: bool,
    wire: &mut Vec<DetailEntry>,
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
        wire.push(DetailEntry {
            t: 1,
            label: String::new(),
            base: state.pending[0].index,
            items,
        });
    }
    state.pending.clear();
    state.entry_index = state.entry_index.saturating_add(1);
}

fn commit_detail_page(loaded: LoadedDetail) {
    let mut session = DETAIL_SESSION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
    let albums = detail_album_count(&wire);
    current.cache.insert(
        loaded.page,
        CachedDetailPage {
            wire: Arc::clone(&wire),
            albums,
        },
    );
    let resident = current.cache.resident_albums;
    let evictions = current.cache.evictions;
    drop(session);
    publish_detail_page(
        loaded.generation,
        loaded.page,
        wire,
        false,
        resident,
        evictions,
    );
}

fn publish_detail_page(
    generation: u64,
    page: usize,
    wire: Arc<Vec<DetailEntry>>,
    cache_hit: bool,
    resident: usize,
    evictions: u64,
) {
    ui(move |_| {
        let bytes = publish_detail_page_now(generation, page, &wire);
        log::info!(
            "[local-catalog] phase=artist-albums-publish generation={generation} page={page} entries={} albums={} json_bytes={bytes} cache_hit={cache_hit} resident_albums={resident} evictions={evictions}",
            wire.len(),
            detail_album_count(&wire)
        );
    });
}

fn publish_detail_page_now(generation: u64, page: usize, wire: &[DetailEntry]) -> usize {
    let json = serde_json::to_string(wire).unwrap_or_else(|_| "[]".to_string());
    let bytes = json.len();
    let Ok(json) = CString::new(json) else {
        return 0;
    };
    // SAFETY: C++ copies the bounded page synchronously.
    unsafe { qbz_local_artist_albums_apply_page(generation as i32, page as i32, json.as_ptr()) };
    bytes
}

fn build_detail_anchors(generation: u64) {
    let (catalog_generation, artist_key, sources, columns, entry_total) = {
        let mut session = DETAIL_SESSION
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(current) = session
            .as_mut()
            .filter(|state| state.generation == generation)
        else {
            return;
        };
        if current.anchor_building || current.anchors_ready {
            return;
        }
        current.anchor_building = true;
        (
            current.catalog_generation,
            current.artist_key.clone(),
            current.sources.clone(),
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
            let scan = scan_detail(
                &catalog,
                generation,
                &artist_key,
                &sources,
                columns,
                DetailAnchor::default(),
                usize::MAX,
                entry_total.min(usize::MAX as u64) as usize,
                false,
            )?;
            Ok((scan.anchors, scan.final_state, started.elapsed()))
        })
        .await;
        match result {
            Ok(Ok((anchors, final_state, elapsed))) => {
                finish_detail_anchors(generation, anchors, final_state, elapsed)
            }
            Ok(Err("superseded")) => {}
            Ok(Err(reason)) => fallback_detail(generation, reason),
            Err(_) => fallback_detail(generation, "detail-anchor-worker-join"),
        }
    });
}

fn finish_detail_anchors(
    generation: u64,
    anchors: Vec<DetailAnchor>,
    final_state: DetailAnchor,
    elapsed: std::time::Duration,
) {
    let waiting = {
        let mut session = DETAIL_SESSION
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
            "[local-catalog] phase=artist-album-anchors generation={generation} anchors={count} elapsed={elapsed:?}"
        );
        current.waiting_pages.drain().collect::<Vec<_>>()
    };
    for page in waiting {
        request_detail_page(page as i32, generation as i32);
    }
}

fn map_artist(record: &ArtistRecord) -> NativeArtist {
    let art_key = format!("catalog-artist:{}", record.artist_key);
    let cached = crate::local_artist_images_qt::register(
        &art_key,
        &record.display_name,
        !record.artwork_token.is_empty(),
    );
    let reference = cached
        .as_deref()
        .and_then(|path| crate::local_rows::art_token(Some("local"), path))
        .or_else(|| {
            crate::local_rows::art_token(Some(&record.artwork_source), &record.artwork_token)
        });
    if let Some(reference) = reference {
        crate::local_state::with_art(|art| {
            art.insert(art_key.clone(), reference);
        });
    }
    NativeArtist {
        row: ArtistRow {
            name: record.display_name.clone(),
            album_count: record.album_count,
            track_count: record.track_count,
            art_key,
            source: record.source.clone(),
            sources: Vec::new(),
            formats: Vec::new(),
            quality_tiers: Vec::new(),
            years: Vec::new(),
            year: String::new(),
        },
        artist_key: record.artist_key.clone(),
        art_path: String::new(),
    }
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
    let source = badge_source(Some(source_word(record)));
    let source_raw = badge_source_raw(Some(source_word(record)));
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
    let favoriteable = crate::local_rows::album_favorite_source(&sources).is_some();
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

fn source_word(record: &AlbumRecord) -> &str {
    if record.source_raw.trim().is_empty() {
        record.source.as_str()
    } else {
        &record.source_raw
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

fn artist_initial(key: &str) -> String {
    let initial = key.chars().next().unwrap_or('#').to_ascii_uppercase();
    if initial.is_ascii_uppercase() {
        initial.to_string()
    } else {
        "#".to_string()
    }
}

/// Empty query sources means every catalog source, so return a separate
/// `none_enabled` bit to distinguish "all enabled" from "nothing enabled".
/// The common all-enabled path intentionally leaves the descriptor unscoped;
/// that keeps the materialized name/count index as the direct query plan.
fn enabled_sources(
    catalog: &qbz_local_catalog::Catalog,
) -> Result<(Vec<SourceKey>, bool), &'static str> {
    let stats = catalog.stats().map_err(|_| "source-counts")?;
    let plex = crate::local_plex::is_configured();
    let remote = crate::media_servers_qt::configured_words();
    let total_sources = stats.source_counts.len();
    let enabled = stats
        .source_counts
        .into_iter()
        .map(|(source, _)| source)
        .filter(|source| match source.source {
            SourceKind::Local | SourceKind::Offline => true,
            SourceKind::Plex => plex,
            SourceKind::Jellyfin => remote.iter().any(|word| *word == "jellyfin"),
            SourceKind::Subsonic => remote.iter().any(|word| *word == "subsonic"),
        })
        .collect::<Vec<_>>();
    let none_enabled = enabled.is_empty() && stats.track_count > 0;
    let query_sources = if enabled.len() == total_sources {
        Vec::new()
    } else {
        enabled
    };
    Ok((query_sources, none_enabled))
}

fn open_active() -> Result<(qbz_local_catalog::Catalog, u64), &'static str> {
    let locations = crate::local_catalog_qt::locations().ok_or("missing-data-directory")?;
    match BootstrapLayout::new(locations.catalog_dir).open_active() {
        ActiveCatalog::Ready { catalog, manifest } => Ok((catalog, manifest.active_generation)),
        ActiveCatalog::Fallback(_) => Err("catalog-not-ready"),
    }
}

fn nearest_artist_anchor(anchors: &BTreeMap<usize, ArtistAnchor>, target: usize) -> ArtistAnchor {
    anchors
        .range(..=target)
        .next_back()
        .map(|(_, anchor)| anchor.clone())
        .unwrap_or_default()
}

fn nearest_detail_anchor(anchors: &BTreeMap<usize, DetailAnchor>, target: usize) -> DetailAnchor {
    anchors
        .range(..=target)
        .next_back()
        .map(|(_, anchor)| anchor.clone())
        .unwrap_or_default()
}

fn set_loading() {
    ui(|mut bridge| {
        bridge.as_mut().set_local_artists_loading(true);
        bridge
            .as_mut()
            .set_local_artists_native_error(QString::default());
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
    let detail_generation = next_generation(&DETAIL_GENERATION);
    *DETAIL_SESSION
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    log::warn!("[local-catalog] phase=artists-fallback generation={generation} reason={reason}");
    ui(move |mut bridge| {
        cpp_detail_reset(detail_generation, 0, 0);
        bridge.as_mut().set_local_artists_native_active(false);
        bridge.as_mut().set_local_artists_native_total(0);
        bridge.as_mut().set_local_artist_albums_native_total(0);
        bridge.as_mut().set_local_artist_albums_loading(false);
        bridge
            .as_mut()
            .set_local_artists_native_jumps_json(QString::from("[]"));
        bridge
            .as_mut()
            .set_local_artists_native_error(QString::from(reason));
        crate::local_bridge_ops::load_artists_legacy();
        crate::local_bridge_ops::load_albums_legacy();
    });
}

fn fallback_detail(generation: u64, reason: &'static str) {
    if DETAIL_GENERATION.load(Ordering::Acquire) != generation {
        return;
    }
    let rail_generation = QUERY_GENERATION.load(Ordering::Acquire);
    fallback(rail_generation, reason);
}

fn deactivate_for_legacy(reason: &'static str) {
    // A filtered compatibility view must not be replaced by an older native
    // response or an unfiltered retry when the catalog publishes an update.
    next_generation(&QUERY_GENERATION);
    *LAST_SEARCH
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = None;
    let detail_generation = next_generation(&DETAIL_GENERATION);
    *DETAIL_SESSION
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    ui(move |mut bridge| {
        cpp_detail_reset(detail_generation, 0, 0);
        bridge.as_mut().set_local_artists_native_active(false);
        bridge.as_mut().set_local_artists_native_total(0);
        bridge.as_mut().set_local_artist_albums_native_total(0);
        bridge.as_mut().set_local_artist_albums_loading(false);
        bridge
            .as_mut()
            .set_local_artists_native_jumps_json(QString::from("[]"));
        bridge
            .as_mut()
            .set_local_artists_native_error(QString::from(reason));
    });
}

fn artist_count(wire: &[ArtistEntry]) -> usize {
    wire.iter().filter(|entry| entry.t == 1).count()
}

fn detail_album_count(wire: &[DetailEntry]) -> usize {
    wire.iter().map(|entry| entry.items.len()).sum()
}

fn jumps_json(jumps: Vec<NativeJump>) -> String {
    serde_json::to_string(&jumps).unwrap_or_else(|_| "[]".to_string())
}

fn cpp_artist_reset(generation: u64, total: u64, artists: u64) {
    // SAFETY: primitive values only; C++ applies synchronously on the Qt thread.
    unsafe {
        qbz_local_artists_reset(
            generation as i32,
            total.min(i64::MAX as u64) as i64,
            artists.min(i64::MAX as u64) as i64,
        )
    };
}

fn cpp_detail_reset(generation: u64, total: u64, albums: u64) {
    // SAFETY: primitive values only; C++ applies synchronously on the Qt thread.
    unsafe {
        qbz_local_artist_albums_reset(
            generation as i32,
            total.min(i64::MAX as u64) as i64,
            albums.min(i64::MAX as u64) as i64,
        )
    };
}

fn next_generation(counter: &AtomicU64) -> u64 {
    let mut generation = counter.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    if generation == 0 {
        counter.store(1, Ordering::Release);
        generation = 1;
    }
    generation
}

#[cfg(test)]
mod tests {
    use super::*;
    static QUERY_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn artist_entry(index: usize) -> ArtistEntry {
        ArtistEntry {
            t: 1,
            label: String::new(),
            base: index,
            item: Some(NativeArtist {
                row: ArtistRow {
                    name: format!("Artist {index}"),
                    album_count: 1,
                    track_count: 1,
                    art_key: format!("artist:{index}"),
                    source: "local".to_string(),
                    sources: vec!["local".to_string()],
                    formats: Vec::new(),
                    quality_tiers: Vec::new(),
                    years: Vec::new(),
                    year: String::new(),
                },
                artist_key: format!("artist {index}"),
                art_path: String::new(),
            }),
        }
    }

    #[test]
    fn artist_page_cache_is_bounded() {
        let mut cache = ArtistPageLru::default();
        for page in 0..25 {
            let wire = Arc::new((0..100).map(|row| artist_entry(page * 100 + row)).collect());
            cache.insert(page, CachedArtistPage { wire, artists: 100 });
        }
        assert!(cache.resident_artists <= MAX_RESIDENT_ARTISTS);
        assert!(cache.evictions > 0);
    }

    #[test]
    fn stale_artist_generation_is_not_current() {
        let _guard = QUERY_TEST_LOCK.lock().unwrap();
        let old = next_generation(&QUERY_GENERATION);
        let new = next_generation(&QUERY_GENERATION);
        assert_ne!(old, new);
        assert_ne!(QUERY_GENERATION.load(Ordering::Acquire), old);
        assert_eq!(QUERY_GENERATION.load(Ordering::Acquire), new);
    }

    #[test]
    fn filtered_artist_query_cancels_native_results_and_unfiltered_retry() {
        let _guard = QUERY_TEST_LOCK.lock().unwrap();
        *LAST_SEARCH.lock().unwrap() = Some("Led Zeppelin".into());
        let generation = QUERY_GENERATION.load(Ordering::Acquire);
        assert!(!reset(
            "Led Zeppelin".into(),
            "name-asc".into(),
            r#"{"local":true}"#.into()
        ));
        assert!(QUERY_GENERATION.load(Ordering::Acquire) > generation);
        assert!(LAST_SEARCH.lock().unwrap().is_none());
        assert!(!retry_last());
    }

    #[test]
    fn artist_alpha_bucket_matches_the_existing_rail() {
        assert_eq!(artist_initial("air"), "A");
        assert_eq!(artist_initial("123 go"), "#");
        assert_eq!(artist_initial("émilie"), "#");
    }

    #[test]
    fn artist_qml_restarts_timer_ids_without_treating_them_as_root_properties() {
        let qml = include_str!("../qml/views/local/LocalArtistsTab.qml");
        assert!(qml.contains("nativeQueryCoalescer.restart()"));
        assert!(qml.contains("detailQueryCoalescer.restart()"));
        assert!(!qml.contains("root.nativeQueryCoalescer"));
        assert!(!qml.contains("root.detailQueryCoalescer"));
    }

    #[test]
    fn artist_transport_is_bounded_to_one_visual_page() {
        const TOTAL: usize = 20_000;
        let legacy = (0..TOTAL)
            .map(|index| ArtistRow {
                name: format!("Artist {index:05}"),
                album_count: 1,
                track_count: 1,
                art_key: format!("artist:Artist {index:05}"),
                source: match index % 4 {
                    0 => "local",
                    1 => "plex",
                    2 => "jellyfin",
                    _ => "subsonic",
                }
                .to_string(),
                sources: Vec::new(),
                formats: Vec::new(),
                quality_tiers: Vec::new(),
                years: Vec::new(),
                year: String::new(),
            })
            .collect::<Vec<_>>();
        let legacy_bytes = serde_json::to_vec(&legacy).unwrap().len();
        let map_started = Instant::now();
        let mut page = vec![ArtistEntry {
            t: 0,
            label: "A".to_string(),
            base: 0,
            item: None,
        }];
        page.extend(
            legacy
                .iter()
                .take(99)
                .enumerate()
                .map(|(index, row)| ArtistEntry {
                    t: 1,
                    label: String::new(),
                    base: index,
                    item: Some(NativeArtist {
                        row: row.clone(),
                        artist_key: qbz_local_catalog::normalize_artist_key(&row.name),
                        art_path: String::new(),
                    }),
                }),
        );
        let map_time = map_started.elapsed();
        let serialize_started = Instant::now();
        let native_bytes = serde_json::to_vec(&page).unwrap().len();
        let serialize_time = serialize_started.elapsed();

        assert_eq!(page.len(), PAGE_ENTRIES);
        assert_eq!(artist_count(&page), 99);
        assert!(native_bytes < legacy_bytes / 100);
        println!(
            "F2_ARTISTS_TRANSPORT total={TOTAL} first_entries={} first_artists={} json_bytes={native_bytes} legacy_json_bytes={legacy_bytes} map_ms={:.3} serialize_ms={:.3} qml_artist_json_bytes=0",
            page.len(),
            artist_count(&page),
            map_time.as_secs_f64() * 1_000.0,
            serialize_time.as_secs_f64() * 1_000.0,
        );
    }
}
