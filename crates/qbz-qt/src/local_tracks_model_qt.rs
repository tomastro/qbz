//! Native, bounded Tracks model controller (Local Library backend phase E).
//!
//! QML sees the hand-written `QAbstractListModel` in
//! `cxx/local_tracks_model.*`. This module owns immutable query descriptors,
//! catalog keysets, sparse anchors, async page work, the mirrored eight-page
//! LRU and descriptor/range selection. No database handle crosses an await and
//! the C++ model's `data()` only reads resident rows.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::{c_char, CString};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cxx_qt_lib::QString;
use qbz_library::{AudioFormat, LocalTrack};
use qbz_local_catalog::{
    ActiveCatalog, BootstrapLayout, QueryDescriptor, SourceKey, SourceKind, TrackCursor,
    TrackGroup, TrackRecord, TrackRef, TrackSort,
};
use qbz_source::SourceId;
use serde::Serialize;

use crate::local_bridge::ui;
use crate::local_rows::{badge_source, badge_source_raw, detail_of, mmss, tier_of, TrackRow};

const PAGE_ROWS: usize = 250;
const MAX_RESIDENT_PAGES: usize = 8;
const ANCHOR_STRIDE_ROWS: usize = 1_000;

static QUERY_GENERATION: AtomicU64 = AtomicU64::new(0);
static SESSION_FAILED: AtomicBool = AtomicBool::new(false);
static SESSION: Mutex<Option<NativeSession>> = Mutex::new(None);

unsafe extern "C" {
    fn qbz_local_tracks_register_qml_type();
    fn qbz_local_tracks_reset(generation: i32, total_count: i64, group: *const c_char);
    fn qbz_local_tracks_apply_page(generation: i32, page: i32, json: *const c_char) -> bool;
    fn qbz_local_tracks_set_selection(generation: i32, json: *const c_char, selected_count: i64);
}

#[derive(Clone)]
struct CachedPage {
    wire: Arc<Vec<NativeEntry>>,
    records: Arc<Vec<TrackRecord>>,
}

struct NativeSession {
    generation: u64,
    catalog_generation: u64,
    descriptor: QueryDescriptor,
    total: u64,
    anchors: BTreeMap<usize, TrackCursor>,
    anchors_ready: bool,
    anchor_building: bool,
    pending_pages: HashSet<usize>,
    waiting_pages: HashSet<usize>,
    cache: PageLru<CachedPage>,
    selection: QuerySelection,
}

#[derive(Clone, Serialize)]
struct NativeEntry {
    #[serde(rename = "groupStart")]
    group_start: bool,
    #[serde(rename = "groupLabel")]
    group_label: String,
    row: TrackRow,
}

#[derive(Serialize)]
struct NativeJump {
    letter: String,
    index: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct QuerySelection {
    /// `false`: ranges are inclusions. `true`: ranges are exclusions from the
    /// immutable query descriptor.
    all: bool,
    ranges: Vec<(u64, u64)>,
    anchor: Option<u64>,
}

impl QuerySelection {
    fn contains(&self, index: u64) -> bool {
        let in_ranges = self
            .ranges
            .iter()
            .any(|&(first, last)| first <= index && index <= last);
        if self.all {
            !in_ranges
        } else {
            in_ranges
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

    fn select_all(&mut self) {
        self.all = true;
        self.ranges.clear();
        self.anchor = None;
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug)]
struct PageLru<T> {
    capacity: usize,
    entries: HashMap<usize, T>,
    order: VecDeque<usize>,
    evictions: u64,
}

impl<T> PageLru<T> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
            evictions: 0,
        }
    }

    fn get(&mut self, page: usize) -> Option<&T> {
        if !self.entries.contains_key(&page) {
            return None;
        }
        self.touch(page);
        self.entries.get(&page)
    }

    fn insert(&mut self, page: usize, value: T) {
        self.entries.insert(page, value);
        self.touch(page);
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if self.entries.remove(&oldest).is_some() {
                self.evictions += 1;
            }
        }
    }

    fn touch(&mut self, page: usize) {
        self.order.retain(|candidate| *candidate != page);
        self.order.push_back(page);
    }

    fn resident_rows(&self) -> usize {
        self.entries.len() * PAGE_ROWS
    }
}

pub(crate) fn register_qml_model() {
    // SAFETY: link/registration anchor; no arguments and no retained Rust
    // pointer. Actual registration runs through Q_COREAPP_STARTUP_FUNCTION.
    unsafe { qbz_local_tracks_register_qml_type() };
}

pub(crate) fn requested() -> bool {
    if SESSION_FAILED.load(Ordering::Acquire) {
        return false;
    }
    // The rollout switch used to be opt-in while the native model was being
    // proved. Keeping that default after the model shipped silently routes a
    // normal launch through the legacy accumulated-page document: the A-Z
    // rail can then only know about letters in the current 500-row prefix.
    // Native is the production path now. An explicit false value remains a
    // diagnostic escape hatch, and SESSION_FAILED still provides the
    // per-session automatic rollback after a real model/catalog failure.
    !std::env::var("QBZ_LOCAL_CATALOG_TRACKS")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
}

/// Start a new immutable native query. Returns `false` when the runtime flag
/// is off or this surface already fell back for the session.
pub(crate) fn reset() -> bool {
    if !requested() {
        return false;
    }
    let generation = next_generation();
    let query = crate::local_state::tracks_query();
    let sort = parse_sort(&crate::local_state::tracks_sort());
    let group = parse_group(&crate::local_state::tracks_group());
    let filter = crate::local_filter::MediaFilter::from_json(&crate::local_state::tracks_filter());
    let descriptor = QueryDescriptor::tracks()
        .with_search(query.clone())
        .with_sort(sort)
        .with_group(group)
        .with_formats(filter.formats)
        .including_other_formats(filter.other_formats)
        .with_quality_tiers(filter.qualities)
        .with_source_buckets(filter.sources);

    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = None;
    if !query.is_empty() && query.chars().count() < 3 {
        publish_empty(generation, group);
        return true;
    }

    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || open_query(generation, descriptor)).await;
        match result {
            Ok(Ok(opened)) => activate_query(opened),
            Ok(Err(reason)) => fallback(generation, reason),
            Err(_) => fallback(generation, "worker-join"),
        }
    });
    true
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
        log::debug!("[local-catalog] phase=page-discard generation={generation} reason=stale-miss");
        return;
    };
    if let Some(cached) = current.cache.get(page).cloned() {
        let rows = current.cache.resident_rows();
        let evictions = current.cache.evictions;
        drop(session);
        publish_page(generation, page, &cached.wire, true, rows, evictions);
        return;
    }
    if !current.pending_pages.insert(page) {
        return;
    }

    let start = page.saturating_mul(PAGE_ROWS);
    let nearest = nearest_anchor(&current.anchors, start);
    if start.saturating_sub(nearest.0) > ANCHOR_STRIDE_ROWS && !current.anchors_ready {
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
    let anchor_row = nearest.0;
    let anchor_cursor = nearest.1.cloned();
    drop(session);

    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            load_page(
                generation,
                catalog_generation,
                descriptor,
                page,
                anchor_row,
                anchor_cursor,
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
        if index as u64 >= total {
            return;
        }
        selection.toggle(index as u64, shift);
    });
}

pub(crate) fn select_all() {
    update_selection(|selection, _| selection.select_all());
}

pub(crate) fn clear_selection() {
    update_selection(|selection, _| selection.clear());
}

/// Play the resident keyset window containing `index`. The clicked page and
/// any already-prefetched consecutive pages become the queue; resolving the
/// stable catalog refs happens off the Qt thread against the authoritative
/// caches.
pub(crate) fn play(index: i64) {
    let Some((generation, records, start)) = resident_play_window(index) else {
        return;
    };
    crate::spawn(async move {
        let rows = tokio::task::spawn_blocking(move || resolve_records_blocking(&records))
            .await
            .unwrap_or_default();
        if rows.is_empty() || QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        crate::local_playback::play_rows(&crate::app(), rows, start, false).await;
    });
}

/// Apply a one-row queue action without making the QML row carry a legacy
/// integer id. The index resolves to the resident TrackRef snapshot.
pub(crate) fn enqueue(index: i64, mode: String) {
    let Some((generation, record)) = resident_record(index) else {
        return;
    };
    crate::spawn(async move {
        let rows = tokio::task::spawn_blocking(move || resolve_records_blocking(&[record]))
            .await
            .unwrap_or_default();
        if rows.is_empty() || QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        let action = match mode.as_str() {
            "next" => "play-next",
            "later" => "play-later",
            _ => "queue",
        };
        crate::local_bulk::apply(rows, action).await;
    });
}

pub(crate) fn row_action(index: i64, action: String) {
    let track_info = action == "track-info";
    if track_info {
        crate::local_media_info_qt::begin();
    }
    let Some(lookup) = action_record_lookup(index) else {
        if track_info {
            crate::local_media_info_qt::open_empty("track");
        }
        return;
    };
    crate::spawn(async move {
        let generation = lookup.generation;
        let record = tokio::task::spawn_blocking(move || load_action_record(lookup))
            .await
            .ok()
            .flatten();
        let rows = record
            .map(|record| resolve_records_blocking(&[record]))
            .unwrap_or_default();
        // Physical media info belongs to the row the user clicked, not to the
        // lifetime of the surrounding page. A tab/navigation reset may race
        // this off-thread lookup; discarding it used to leave the modal's
        // `loading` flag open until Local Library was mounted again.
        if !track_info && QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        crate::local_bulk::apply(rows, &action).await;
    });
}

struct ActionRecordLookup {
    generation: u64,
    catalog_generation: u64,
    descriptor: QueryDescriptor,
    page: usize,
    offset: usize,
    anchor_row: usize,
    anchor_cursor: Option<TrackCursor>,
    resident: Option<TrackRecord>,
}

fn action_record_lookup(index: i64) -> Option<ActionRecordLookup> {
    let index = usize::try_from(index).ok()?;
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let current = session.as_mut()?;
    if index as u64 >= current.total {
        return None;
    }
    let page = index / PAGE_ROWS;
    let offset = index % PAGE_ROWS;
    let resident = current
        .cache
        .get(page)
        .and_then(|cached| cached.records.get(offset))
        .cloned();
    let target_row = page.saturating_mul(PAGE_ROWS);
    let (anchor_row, anchor_cursor) = nearest_anchor(&current.anchors, target_row);
    Some(ActionRecordLookup {
        generation: current.generation,
        catalog_generation: current.catalog_generation,
        descriptor: current.descriptor.clone(),
        page,
        offset,
        anchor_row,
        anchor_cursor: anchor_cursor.cloned(),
        resident,
    })
}

fn load_action_record(lookup: ActionRecordLookup) -> Option<TrackRecord> {
    if let Some(record) = lookup.resident {
        return Some(record);
    }
    let (catalog, active_generation) = open_active().ok()?;
    if active_generation != lookup.catalog_generation {
        return None;
    }
    let target_row = lookup.page.saturating_mul(PAGE_ROWS);
    let mut row = lookup.anchor_row;
    let mut cursor = lookup.anchor_cursor;
    while row < target_row {
        let page = catalog
            .query_tracks(&lookup.descriptor, cursor.as_ref(), PAGE_ROWS)
            .ok()?;
        let count = page.rows.len();
        let next = page.next_cursor?;
        row = row.saturating_add(count);
        cursor = Some(next);
    }
    if row != target_row {
        return None;
    }
    catalog
        .query_tracks(&lookup.descriptor, cursor.as_ref(), PAGE_ROWS)
        .ok()?
        .rows
        .get(lookup.offset)
        .cloned()
}

/// Resolve the descriptor/range selection only when an explicit action needs
/// concrete authoritative rows. Select-all itself remains O(1) memory.
pub(crate) fn bulk_action(action: String) {
    let snapshot = {
        let session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
        session.as_ref().map(|current| {
            (
                current.generation,
                current.catalog_generation,
                current.descriptor.clone(),
                current.total,
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
        let resolved = tokio::task::spawn_blocking(move || {
            let records = selected_records_blocking(
                generation,
                catalog_generation,
                &descriptor,
                total,
                &selection,
            )?;
            Ok::<_, &'static str>(resolve_records_blocking(&records))
        })
        .await;
        let rows = match resolved {
            Ok(Ok(rows)) => rows,
            Ok(Err("superseded")) => return,
            Ok(Err(reason)) => {
                log::warn!(
                    "[local-catalog] phase=tracks-action generation={generation} action={action} reason={reason}"
                );
                return;
            }
            Err(_) => return,
        };
        if QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        log::info!(
            "[local-catalog] phase=tracks-action generation={generation} action={action} resolved_rows={}",
            rows.len()
        );
        if crate::local_bulk::apply(rows, &action).await {
            clear_selection_generation(generation);
        }
    });
}

fn resident_record(index: i64) -> Option<(u64, TrackRecord)> {
    let index = usize::try_from(index).ok()?;
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let current = session.as_mut()?;
    if index as u64 >= current.total {
        return None;
    }
    let page = index / PAGE_ROWS;
    let offset = index % PAGE_ROWS;
    let record = current.cache.get(page)?.records.get(offset)?.clone();
    Some((current.generation, record))
}

fn resident_play_window(index: i64) -> Option<(u64, Vec<TrackRecord>, usize)> {
    let index = usize::try_from(index).ok()?;
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let current = session.as_mut()?;
    if index as u64 >= current.total {
        return None;
    }
    let first_page = index / PAGE_ROWS;
    let start = index % PAGE_ROWS;
    let mut records = Vec::new();
    for page in first_page..first_page.saturating_add(MAX_RESIDENT_PAGES) {
        let Some(cached) = current.cache.get(page) else {
            break;
        };
        records.extend(cached.records.iter().cloned());
    }
    (start < records.len()).then_some((current.generation, records, start))
}

fn selected_records_blocking(
    generation: u64,
    catalog_generation: u64,
    descriptor: &QueryDescriptor,
    total: u64,
    selection: &QuerySelection,
) -> Result<Vec<TrackRecord>, &'static str> {
    let (catalog, active_generation) = open_active()?;
    if active_generation != catalog_generation {
        return Err("catalog-generation-changed");
    }
    let mut cursor = None;
    let mut index = 0_u64;
    let capacity = selection.count(total).min(16_384) as usize;
    let mut selected = Vec::with_capacity(capacity);
    loop {
        if QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return Err("superseded");
        }
        let page = catalog
            .query_tracks(descriptor, cursor.as_ref(), 500)
            .map_err(|_| "action-query")?;
        for record in page.rows {
            if selection.contains(index) {
                selected.push(record);
            }
            index = index.saturating_add(1);
        }
        let Some(next) = page.next_cursor else {
            break;
        };
        cursor = Some(next);
    }
    Ok(selected)
}

fn resolve_records_blocking(records: &[TrackRecord]) -> Vec<LocalTrack> {
    let local_ids = records
        .iter()
        .filter_map(|record| record.local_track_id)
        .collect::<Vec<_>>();
    let local_rows = crate::local_state::with_db(|db| {
        let mut rows = HashMap::new();
        for id in &local_ids {
            if let Some(track) = db.get_track(*id)? {
                rows.insert(*id, track);
            }
        }
        Ok(rows)
    })
    .unwrap_or_default();

    let plex_ids = records
        .iter()
        .filter(|record| record.track_ref.source == SourceKind::Plex)
        .map(|record| record.track_ref.native_id.clone())
        .collect::<Vec<_>>();
    let mut plex_rows = HashMap::new();
    for chunk in plex_ids.chunks(500) {
        if let Ok(rows) = qbz_plex::plex_cache_get_cached_tracks_by_keys(chunk) {
            for row in rows {
                let mapped = crate::local_plex::map_cached_to_local_track(row);
                plex_rows.insert(mapped.file_path.clone(), mapped);
            }
        }
    }

    let jellyfin = resolve_remote_records(records, SourceKind::Jellyfin);
    let subsonic = resolve_remote_records(records, SourceKind::Subsonic);
    let mut missing = 0usize;
    let rows = records
        .iter()
        .filter_map(|record| {
            let resolved = match record.track_ref.source {
                SourceKind::Local | SourceKind::Offline => record
                    .local_track_id
                    .and_then(|id| local_rows.get(&id).cloned()),
                SourceKind::Plex => plex_rows.get(&record.track_ref.native_id).cloned(),
                SourceKind::Jellyfin => jellyfin.get(&record.track_ref.native_id).cloned(),
                SourceKind::Subsonic => subsonic.get(&record.track_ref.native_id).cloned(),
            };
            if resolved.is_none() {
                missing += 1;
            }
            resolved
        })
        .collect::<Vec<_>>();
    if missing > 0 {
        log::warn!(
            "[local-catalog] phase=tracks-resolve requested={} missing={missing}",
            records.len()
        );
    }
    rows
}

fn resolve_remote_records(
    records: &[TrackRecord],
    source_kind: SourceKind,
) -> HashMap<String, LocalTrack> {
    let source = match source_kind {
        SourceKind::Jellyfin => qbz_media_cache::RemoteSource::Jellyfin,
        SourceKind::Subsonic => qbz_media_cache::RemoteSource::Subsonic,
        _ => return HashMap::new(),
    };
    let ids = records
        .iter()
        .filter(|record| record.track_ref.source == source_kind)
        .map(|record| record.track_ref.native_id.as_str())
        .collect::<Vec<_>>();
    if source_kind == SourceKind::Jellyfin && !ids.is_empty() {
        crate::media_sync_qt::prioritize_jellyfin_quality(
            ids.iter().map(|item_id| (*item_id).to_string()).collect(),
            true,
        );
    }
    let read = |conn: &rusqlite::Connection| {
        ids.iter()
            .filter_map(|id| {
                qbz_media_cache::track_by_item_id(conn, source, id)
                    .ok()
                    .flatten()
                    .map(crate::media_servers_qt::cached_to_local_track)
                    .map(|track| ((*id).to_string(), track))
            })
            .collect::<HashMap<_, _>>()
    };
    match source {
        qbz_media_cache::RemoteSource::Jellyfin => qbz_source::registry()
            .jellyfin()
            .cache()
            .with(read)
            .unwrap_or_default(),
        qbz_media_cache::RemoteSource::Subsonic => qbz_source::registry()
            .subsonic()
            .cache()
            .with(read)
            .unwrap_or_default(),
    }
}

fn clear_selection_generation(generation: u64) {
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let Some(current) = session
        .as_mut()
        .filter(|state| state.generation == generation)
    else {
        return;
    };
    current.selection.clear();
    drop(session);
    ui(move |mut bridge| {
        bridge.as_mut().set_local_tracks_native_selected_count(0);
        cpp_set_selection(generation, &selection_json(&QuerySelection::default()), 0);
    });
}

fn update_selection(update: impl FnOnce(&mut QuerySelection, u64)) {
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let Some(current) = session.as_mut() else {
        return;
    };
    update(&mut current.selection, current.total);
    let generation = current.generation;
    let count = current.selection.count(current.total);
    let json = selection_json(&current.selection);
    drop(session);
    ui(move |mut bridge| {
        bridge
            .as_mut()
            .set_local_tracks_native_selected_count(count.min(i64::MAX as u64) as i64);
        cpp_set_selection(generation, &json, count);
    });
}

struct OpenedQuery {
    generation: u64,
    catalog_generation: u64,
    descriptor: QueryDescriptor,
    total: u64,
    page: LoadedPage,
    count_time: std::time::Duration,
}

struct LoadedPage {
    generation: u64,
    page: usize,
    wire: Vec<NativeEntry>,
    records: Vec<TrackRecord>,
    anchors: Vec<(usize, TrackCursor)>,
    sql_time: std::time::Duration,
    map_time: std::time::Duration,
}

fn open_query(
    generation: u64,
    mut descriptor: QueryDescriptor,
) -> Result<OpenedQuery, &'static str> {
    let (catalog, catalog_generation) = open_active()?;
    let sources = enabled_sources(&catalog)?;
    if sources.is_empty() && catalog.stats().map_err(|_| "source-counts")?.track_count > 0 {
        return Ok(OpenedQuery {
            generation,
            catalog_generation,
            descriptor,
            total: 0,
            page: LoadedPage {
                generation,
                page: 0,
                wire: Vec::new(),
                records: Vec::new(),
                anchors: Vec::new(),
                sql_time: std::time::Duration::ZERO,
                map_time: std::time::Duration::ZERO,
            },
            count_time: std::time::Duration::ZERO,
        });
    }
    descriptor = descriptor.with_sources(sources);
    let count_started = Instant::now();
    let total = catalog.count_tracks(&descriptor).map_err(|_| "count")?;
    let count_time = count_started.elapsed();
    let page = load_page_from_catalog(&catalog, generation, &descriptor, 0, 0, None)?;
    Ok(OpenedQuery {
        generation,
        catalog_generation,
        descriptor,
        total,
        page,
        count_time,
    })
}

fn activate_query(opened: OpenedQuery) {
    if QUERY_GENERATION.load(Ordering::Acquire) != opened.generation {
        log::info!(
            "[local-catalog] phase=query-discard generation={} reason=superseded-open",
            opened.generation
        );
        return;
    }
    let track_group = opened.descriptor.group();
    let grouped = track_group != TrackGroup::Off;
    let group = group_word(track_group).to_string();
    let initial_jumps = jumps_json(if grouped {
        group_jumps(&opened.page.records, 0, track_group)
    } else {
        Vec::new()
    });
    let first_wire = Arc::new(opened.page.wire.clone());
    let first_records = Arc::new(opened.page.records.clone());
    let mut cache = PageLru::new(MAX_RESIDENT_PAGES);
    cache.insert(
        0,
        CachedPage {
            wire: Arc::clone(&first_wire),
            records: first_records,
        },
    );
    let mut anchors = BTreeMap::new();
    for (row, cursor) in opened.page.anchors {
        anchors.insert(row, cursor);
    }
    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = Some(NativeSession {
        generation: opened.generation,
        catalog_generation: opened.catalog_generation,
        descriptor: opened.descriptor,
        total: opened.total,
        anchors,
        anchors_ready: opened.total <= ANCHOR_STRIDE_ROWS as u64,
        anchor_building: false,
        pending_pages: HashSet::new(),
        waiting_pages: HashSet::new(),
        cache,
        selection: QuerySelection::default(),
    });

    let generation = opened.generation;
    let total = opened.total;
    let count_time = opened.count_time;
    let sql_time = opened.page.sql_time;
    let map_time = opened.page.map_time;
    let wire = Arc::clone(&first_wire);
    ui(move |mut bridge| {
        cpp_reset(generation, total, &group);
        let bytes = publish_page_now(generation, 0, &wire);
        bridge.as_mut().set_local_tracks_native_active(true);
        bridge
            .as_mut()
            .set_local_tracks_native_total(total.min(i64::MAX as u64) as i64);
        bridge.as_mut().set_local_tracks_native_selected_count(0);
        bridge
            .as_mut()
            .set_local_tracks_native_jumps_json(QString::from(initial_jumps.as_str()));
        bridge
            .as_mut()
            .set_local_tracks_native_error(QString::default());
        bridge.as_mut().set_local_tracks_loading(false);
        bridge.as_mut().set_local_tracks_loading_more(false);
        log::info!(
            "[local-catalog] phase=tracks-native generation={generation} total={total} page=0 rows={} json_bytes={bytes} count={count_time:?} sql={sql_time:?} map={map_time:?} resident_rows={} cache_pages=1",
            wire.len(),
            wire.len()
        );
    });
    // Group navigation is metadata of the immutable query, not of the pages
    // the ListView happened to make resident. Build it in the same bounded
    // keyset walk as the sparse anchors whenever page zero is not the whole
    // result. This is what keeps the complete # A-Z rail independent from
    // delegate creation and scrolling.
    if opened.total > ANCHOR_STRIDE_ROWS as u64 || (grouped && opened.total > PAGE_ROWS as u64) {
        build_anchors(generation);
    }
}

fn publish_empty(generation: u64, group: TrackGroup) {
    let group = group_word(group).to_string();
    ui(move |mut bridge| {
        cpp_reset(generation, 0, &group);
        bridge.as_mut().set_local_tracks_native_active(true);
        bridge.as_mut().set_local_tracks_native_total(0);
        bridge.as_mut().set_local_tracks_native_selected_count(0);
        bridge
            .as_mut()
            .set_local_tracks_native_jumps_json(QString::from("[]"));
        bridge.as_mut().set_local_tracks_loading(false);
        bridge.as_mut().set_local_tracks_loading_more(false);
    });
}

fn load_page(
    generation: u64,
    catalog_generation: u64,
    descriptor: QueryDescriptor,
    page: usize,
    anchor_row: usize,
    anchor_cursor: Option<TrackCursor>,
) -> Result<LoadedPage, &'static str> {
    let (catalog, active_generation) = open_active()?;
    if active_generation != catalog_generation {
        return Err("catalog-generation-changed");
    }
    load_page_from_catalog(
        &catalog,
        generation,
        &descriptor,
        page,
        anchor_row,
        anchor_cursor,
    )
}

fn load_page_from_catalog(
    catalog: &qbz_local_catalog::Catalog,
    generation: u64,
    descriptor: &QueryDescriptor,
    target_page: usize,
    mut row: usize,
    mut cursor: Option<TrackCursor>,
) -> Result<LoadedPage, &'static str> {
    let target_row = target_page.saturating_mul(PAGE_ROWS);
    let mut anchors = Vec::new();
    while row < target_row {
        if QUERY_GENERATION.load(Ordering::Acquire) != generation {
            return Err("superseded");
        }
        let (page, _) = catalog
            .query_tracks_timed(descriptor, cursor.as_ref(), PAGE_ROWS)
            .map_err(|_| "seek-query")?;
        let count = page.rows.len();
        let Some(next) = page.next_cursor else {
            return Err("seek-past-end");
        };
        row = row.saturating_add(count);
        cursor = Some(next.clone());
        anchors.push((row, next));
    }
    if row != target_row {
        return Err("anchor-misaligned");
    }

    let previous_group = cursor
        .as_ref()
        .map(|value| cursor_group_key(value, descriptor.group()));
    let query_started = Instant::now();
    let (page, metrics) = catalog
        .query_tracks_timed(descriptor, cursor.as_ref(), PAGE_ROWS)
        .map_err(|_| "page-query")?;
    let sql_time = query_started.elapsed().max(metrics.sql_time);
    if let Some(next) = page.next_cursor.clone() {
        anchors.push((target_row.saturating_add(page.rows.len()), next));
    }
    let records = page.rows;
    let map_started = Instant::now();
    let wire = map_entries(&records, descriptor.group(), previous_group.as_deref());
    let map_time = map_started.elapsed();
    Ok(LoadedPage {
        generation,
        page: target_page,
        wire,
        records,
        anchors,
        sql_time,
        map_time,
    })
}

fn commit_page(loaded: LoadedPage) {
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let Some(current) = session
        .as_mut()
        .filter(|state| state.generation == loaded.generation)
    else {
        log::info!(
            "[local-catalog] phase=query-discard generation={} page={} reason=superseded-page",
            loaded.generation,
            loaded.page
        );
        return;
    };
    let jellyfin_ids = loaded
        .records
        .iter()
        .filter(|record| record.track_ref.source == SourceKind::Jellyfin)
        .map(|record| record.track_ref.native_id.clone())
        .collect::<Vec<_>>();
    if !jellyfin_ids.is_empty() {
        crate::media_sync_qt::prioritize_jellyfin_quality(jellyfin_ids, false);
    }
    current.pending_pages.remove(&loaded.page);
    for (row, cursor) in &loaded.anchors {
        current.anchors.insert(*row, cursor.clone());
    }
    let cached = CachedPage {
        wire: Arc::new(loaded.wire),
        records: Arc::new(loaded.records),
    };
    let wire = Arc::clone(&cached.wire);
    current.cache.insert(loaded.page, cached);
    let resident = current.cache.resident_rows();
    let evictions = current.cache.evictions;
    drop(session);
    publish_page(
        loaded.generation,
        loaded.page,
        &wire,
        false,
        resident,
        evictions,
    );
    log::info!(
        "[local-catalog] phase=tracks-page generation={} page={} rows={} sql={:?} map={:?} resident_rows={} evictions={}",
        loaded.generation,
        loaded.page,
        wire.len(),
        loaded.sql_time,
        loaded.map_time,
        resident,
        evictions
    );
}

fn publish_page(
    generation: u64,
    page: usize,
    wire: &Arc<Vec<NativeEntry>>,
    cache_hit: bool,
    resident: usize,
    evictions: u64,
) {
    let wire = Arc::clone(wire);
    ui(move |_| {
        let bytes = publish_page_now(generation, page, &wire);
        log::info!(
            "[local-catalog] phase=tracks-publish generation={generation} page={page} rows={} json_bytes={bytes} cache_hit={cache_hit} resident_rows={resident} evictions={evictions}",
            wire.len()
        );
    });
}

fn publish_page_now(generation: u64, page: usize, wire: &[NativeEntry]) -> usize {
    let json = serde_json::to_string(wire).unwrap_or_else(|_| "[]".to_string());
    let bytes = json.len();
    let Ok(json) = CString::new(json) else {
        return 0;
    };
    // SAFETY: the C++ call consumes the bounded JSON synchronously and never
    // retains the pointer.
    unsafe {
        qbz_local_tracks_apply_page(generation as i32, page as i32, json.as_ptr());
    }
    bytes
}

fn cpp_reset(generation: u64, total: u64, group: &str) {
    let group = CString::new(group).expect("group words contain no NUL");
    // SAFETY: synchronous copy into QString; pointer is not retained.
    unsafe {
        qbz_local_tracks_reset(
            generation as i32,
            total.min(i64::MAX as u64) as i64,
            group.as_ptr(),
        )
    };
}

fn cpp_set_selection(generation: u64, json: &str, count: u64) {
    let Ok(json) = CString::new(json) else {
        return;
    };
    // SAFETY: synchronous QByteArray copy; pointer is not retained.
    unsafe {
        qbz_local_tracks_set_selection(
            generation as i32,
            json.as_ptr(),
            count.min(i64::MAX as u64) as i64,
        )
    };
}

fn build_anchors(generation: u64) {
    let (descriptor, catalog_generation) = {
        let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
        let Some(current) = session
            .as_mut()
            .filter(|state| state.generation == generation)
        else {
            return;
        };
        current.anchor_building = true;
        (current.descriptor.clone(), current.catalog_generation)
    };
    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let (catalog, active_generation) = open_active()?;
            if active_generation != catalog_generation {
                return Err("catalog-generation-changed");
            }
            let started = Instant::now();
            let mut cursor = None;
            let mut row = 0usize;
            let mut anchors = Vec::new();
            let mut jumps = Vec::new();
            let mut seen_letters = HashSet::new();
            loop {
                if QUERY_GENERATION.load(Ordering::Acquire) != generation {
                    return Err("superseded");
                }
                let page = catalog
                    .query_tracks(&descriptor, cursor.as_ref(), 500)
                    .map_err(|_| "anchor-query")?;
                if descriptor.group() != TrackGroup::Off {
                    for jump in group_jumps(&page.rows, row, descriptor.group()) {
                        if seen_letters.insert(jump.letter.clone()) {
                            jumps.push(jump);
                        }
                    }
                }
                row = row.saturating_add(page.rows.len());
                let Some(next) = page.next_cursor else {
                    break;
                };
                if row % ANCHOR_STRIDE_ROWS == 0 {
                    anchors.push((row, next.clone()));
                }
                cursor = Some(next);
            }
            Ok((anchors, jumps, started.elapsed()))
        })
        .await;
        match result {
            Ok(Ok((anchors, jumps, elapsed))) => {
                finish_anchors(generation, anchors, jumps, elapsed)
            }
            Ok(Err("superseded")) => {
                log::info!(
                    "[local-catalog] phase=query-discard generation={generation} reason=superseded-anchors"
                );
            }
            Ok(Err(reason)) => fallback(generation, reason),
            Err(_) => fallback(generation, "anchor-worker-join"),
        }
    });
}

fn finish_anchors(
    generation: u64,
    anchors: Vec<(usize, TrackCursor)>,
    jumps: Vec<NativeJump>,
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
        current.anchors.extend(anchors);
        current.anchors_ready = true;
        current.anchor_building = false;
        log::info!(
            "[local-catalog] phase=tracks-anchors generation={generation} anchors={count} stride_rows={ANCHOR_STRIDE_ROWS} elapsed={elapsed:?}"
        );
        current.waiting_pages.drain().collect::<Vec<_>>()
    };
    let jumps = jumps_json(jumps);
    ui(move |mut bridge| {
        bridge
            .as_mut()
            .set_local_tracks_native_jumps_json(QString::from(jumps.as_str()));
    });
    for page in waiting {
        request_page(page as i32, generation as i32);
    }
}

fn open_active() -> Result<(qbz_local_catalog::Catalog, u64), &'static str> {
    let locations = crate::local_catalog_qt::locations().ok_or("missing-data-directory")?;
    match BootstrapLayout::new(locations.catalog_dir).open_active() {
        ActiveCatalog::Ready { catalog, manifest } => Ok((catalog, manifest.active_generation)),
        ActiveCatalog::Fallback(_) => Err("catalog-not-ready"),
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

fn map_entries(
    records: &[TrackRecord],
    group: TrackGroup,
    previous: Option<&str>,
) -> Vec<NativeEntry> {
    crate::local_state::with_art(|art| {
        let mut previous = previous.map(str::to_string);
        records
            .iter()
            .map(|record| {
                let key = record_group_key(record, group);
                let group_start =
                    group != TrackGroup::Off && previous.as_deref() != Some(key.as_str());
                previous = Some(key);
                NativeEntry {
                    group_start,
                    group_label: record_group_label(record, group),
                    row: map_record(record, art),
                }
            })
            .collect()
    })
}

fn map_record(record: &TrackRecord, art: &mut HashMap<String, (SourceId, String)>) -> TrackRow {
    let id = encode_ref(&record.track_ref);
    let art_key = format!("catalog-track:{id}");
    let raw = if record.source_raw.trim().is_empty() {
        record.track_ref.source.as_str()
    } else {
        record.source_raw.as_str()
    };
    if let Some(token) = record
        .artwork_token
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        if let Some(reference) = crate::local_rows::art_token(Some(raw), token) {
            art.insert(art_key.clone(), reference);
        }
    }
    let format = audio_format(&record.format);
    TrackRow {
        id,
        title: record.title.clone(),
        artist: record.artist.clone(),
        album: record.album.clone(),
        album_id: album_id(record),
        artist_id: String::new(),
        number: record.track_number.unwrap_or(0),
        disc: record.disc_number.unwrap_or(1),
        duration: mmss(record.duration_ms / 1_000),
        quality_tier: tier_of(
            &format,
            record.bit_depth,
            record.sample_rate_hz.unwrap_or(0) as f64,
        )
        .to_string(),
        quality_detail: detail_of(
            &format,
            record.bit_depth,
            record.sample_rate_hz.unwrap_or(0) as f64,
        ),
        format: record.format.to_ascii_uppercase(),
        genres: Vec::new(),
        year: record.year.map(|year| year.to_string()).unwrap_or_default(),
        art_key,
        art_path: String::new(),
        source: badge_source(Some(raw)),
        source_raw: badge_source_raw(Some(raw)),
        explicit: false,
        is_favorite: false,
    }
}

fn album_id(record: &TrackRecord) -> String {
    let native = record.native_album_id.as_deref().unwrap_or("");
    match record.track_ref.source {
        SourceKind::Plex if native.starts_with("plex:") => native.to_string(),
        SourceKind::Plex if !native.is_empty() => format!("plex:album:{native}"),
        SourceKind::Jellyfin if !native.is_empty() => format!("jellyfin:{native}"),
        SourceKind::Subsonic if !native.is_empty() => format!("subsonic:{native}"),
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

fn encode_ref(track_ref: &TrackRef) -> String {
    serde_json::to_string(track_ref).unwrap_or_default()
}

fn record_group_key(record: &TrackRecord, group: TrackGroup) -> String {
    match group {
        TrackGroup::Off => String::new(),
        TrackGroup::Album => qbz_local_catalog::normalize_sort_key(&record.album),
        TrackGroup::Artist => qbz_local_catalog::normalize_sort_key(&record.artist),
        TrackGroup::Name => qbz_local_catalog::normalize_sort_key(&record.title)
            .chars()
            .next()
            .unwrap_or('#')
            .to_uppercase()
            .collect(),
    }
}

fn record_group_label(record: &TrackRecord, group: TrackGroup) -> String {
    match group {
        TrackGroup::Off => String::new(),
        TrackGroup::Album => record.album.clone(),
        TrackGroup::Artist => record.artist.clone(),
        TrackGroup::Name => record_group_key(record, TrackGroup::Name),
    }
}

fn alpha_bucket(value: &str) -> String {
    let initial = qbz_local_catalog::normalize_sort_key(value)
        .chars()
        .next()
        .unwrap_or('#')
        .to_ascii_uppercase();
    if initial.is_ascii_alphabetic() {
        initial.to_string()
    } else {
        "#".to_string()
    }
}

fn group_jumps(records: &[TrackRecord], base: usize, group: TrackGroup) -> Vec<NativeJump> {
    let mut previous = String::new();
    let mut jumps = Vec::new();
    for (offset, record) in records.iter().enumerate() {
        let value = match group {
            TrackGroup::Album => record.album.as_str(),
            TrackGroup::Artist => record.artist.as_str(),
            TrackGroup::Name => record.title.as_str(),
            TrackGroup::Off => continue,
        };
        let letter = alpha_bucket(value);
        if letter != previous {
            previous = letter.clone();
            jumps.push(NativeJump {
                letter,
                index: base.saturating_add(offset),
            });
        }
    }
    jumps
}

fn jumps_json(jumps: Vec<NativeJump>) -> String {
    serde_json::to_string(&jumps).unwrap_or_else(|_| "[]".to_string())
}

fn cursor_group_key(cursor: &TrackCursor, group: TrackGroup) -> String {
    cursor.group_key(group)
}

fn nearest_anchor(
    anchors: &BTreeMap<usize, TrackCursor>,
    target: usize,
) -> (usize, Option<&TrackCursor>) {
    anchors
        .range(..=target)
        .next_back()
        .map(|(row, cursor)| (*row, Some(cursor)))
        .unwrap_or((0, None))
}

fn parse_sort(value: &str) -> TrackSort {
    match value {
        "title-asc" => TrackSort::TitleAsc,
        "title-desc" => TrackSort::TitleDesc,
        "artist-asc" => TrackSort::ArtistAsc,
        "artist-desc" => TrackSort::ArtistDesc,
        "year-asc" => TrackSort::YearAsc,
        "year-desc" => TrackSort::YearDesc,
        "added-desc" => TrackSort::AddedDesc,
        _ => TrackSort::Default,
    }
}

fn parse_group(value: &str) -> TrackGroup {
    match value {
        "album" => TrackGroup::Album,
        "artist" => TrackGroup::Artist,
        "name" => TrackGroup::Name,
        _ => TrackGroup::Off,
    }
}

fn group_word(group: TrackGroup) -> &'static str {
    match group {
        TrackGroup::Off => "off",
        TrackGroup::Album => "album",
        TrackGroup::Artist => "artist",
        TrackGroup::Name => "name",
    }
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

fn fallback(generation: u64, reason: &'static str) {
    if QUERY_GENERATION.load(Ordering::Acquire) != generation {
        return;
    }
    // A first paint can legitimately beat the background bootstrap. Serve the
    // legacy reader now but allow the ready notification to retry. Query,
    // schema and model failures remain session-sticky as required.
    if reason != "catalog-not-ready" {
        SESSION_FAILED.store(true, Ordering::Release);
    }
    *SESSION.lock().unwrap_or_else(|error| error.into_inner()) = None;
    log::warn!("[local-catalog] phase=tracks-fallback generation={generation} reason={reason}");
    ui(move |mut bridge| {
        bridge.as_mut().set_local_tracks_native_active(false);
        bridge.as_mut().set_local_tracks_native_total(0);
        bridge.as_mut().set_local_tracks_native_selected_count(0);
        bridge
            .as_mut()
            .set_local_tracks_native_jumps_json(QString::from("[]"));
        bridge
            .as_mut()
            .set_local_tracks_native_error(QString::from(reason));
        crate::local_bridge_ops::load_tracks_legacy(true);
    });
}

fn selection_json(selection: &QuerySelection) -> String {
    serde_json::json!({
        "all": selection.all,
        "ranges": selection.ranges,
    })
    .to_string()
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
    use qbz_local_catalog::{Catalog, ProjectedTrack};

    #[test]
    fn page_cache_is_a_bounded_lru_and_reports_hits_and_evictions() {
        let mut cache = PageLru::new(8);
        for page in 0..8 {
            cache.insert(page, page);
        }
        assert_eq!(cache.resident_rows(), 2_000);
        assert_eq!(cache.get(0), Some(&0));
        cache.insert(8, 8);
        assert!(cache.get(1).is_none());
        assert_eq!(cache.get(0), Some(&0));
        assert_eq!(cache.evictions, 1);
        assert_eq!(cache.resident_rows(), 2_000);
    }

    #[test]
    fn page_miss_does_not_change_residency_before_async_delivery() {
        let mut cache = PageLru::new(8);
        cache.insert(0, 10);
        assert!(cache.get(7).is_none());
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.resident_rows(), PAGE_ROWS);
        assert_eq!(cache.evictions, 0);
    }

    #[test]
    fn old_query_generation_cannot_become_current() {
        QUERY_GENERATION.store(41, Ordering::Release);
        let old = QUERY_GENERATION.load(Ordering::Acquire);
        let current = next_generation();
        assert_eq!(old, 41);
        assert_eq!(current, 42);
        assert_ne!(QUERY_GENERATION.load(Ordering::Acquire), old);
    }

    #[test]
    fn selection_all_is_descriptor_plus_bounded_exclusion_ranges() {
        let mut selection = QuerySelection::default();
        selection.select_all();
        selection.toggle(25, false);
        selection.toggle(26, false);
        selection.toggle(100, false);
        assert!(selection.all);
        assert_eq!(selection.ranges, vec![(25, 26), (100, 100)]);
        assert_eq!(selection.count(1_000_000), 999_997);
        selection.anchor = Some(20);
        selection.toggle(30, true);
        assert!(selection.ranges.iter().all(|&(a, b)| b < 20 || a > 30));
        assert_eq!(selection.count(1_000_000), 999_999);
    }

    #[test]
    fn shift_selection_uses_intervals_not_one_id_per_result() {
        let mut selection = QuerySelection::default();
        selection.toggle(10, false);
        selection.toggle(50_000, true);
        assert_eq!(selection.ranges, vec![(10, 50_000)]);
        assert_eq!(selection.count(1_000_000), 49_991);
    }

    #[test]
    fn grouped_jump_metadata_is_bounded_and_uses_global_row_indices() {
        let records = ["Alpha", "Another", "Beta", "Zulu"]
            .into_iter()
            .enumerate()
            .map(|(index, title)| metric_track(index as u64, SourceKind::Local, title))
            .collect::<Vec<_>>();
        let mut catalog = Catalog::open_in_memory(1).unwrap();
        catalog.upsert_tracks(&records).unwrap();
        let page = catalog
            .query_tracks(
                &QueryDescriptor::tracks().with_group(TrackGroup::Name),
                None,
                10,
            )
            .unwrap();
        let jumps = group_jumps(&page.rows, 10_000, TrackGroup::Name);
        assert_eq!(jumps.len(), 3);
        assert_eq!(jumps[0].letter, "A");
        assert_eq!(jumps[0].index, 10_000);
        assert_eq!(jumps[2].letter, "Z");
        assert_eq!(jumps[2].index, 10_003);
    }

    #[test]
    fn grouped_jumps_use_the_selected_field_and_fold_numbers_and_symbols() {
        let mut records = vec![
            metric_track(1, SourceKind::Local, "Zulu"),
            metric_track(2, SourceKind::Local, "Alpha"),
            metric_track(3, SourceKind::Local, "Beta"),
        ];
        records[0].album = "1999".to_string();
        records[1].album = "Abbey Road".to_string();
        records[2].album = "Blue".to_string();
        let mut catalog = Catalog::open_in_memory(1).unwrap();
        catalog.upsert_tracks(&records).unwrap();
        let page = catalog
            .query_tracks(
                &QueryDescriptor::tracks().with_group(TrackGroup::Album),
                None,
                10,
            )
            .unwrap();
        let jumps = group_jumps(&page.rows, 50, TrackGroup::Album);
        assert_eq!(
            jumps
                .iter()
                .map(|jump| jump.letter.as_str())
                .collect::<Vec<_>>(),
            vec!["#", "A", "B"]
        );
        assert_eq!(
            jumps.iter().map(|jump| jump.index).collect::<Vec<_>>(),
            vec![50, 51, 52]
        );
    }

    #[test]
    fn artist_group_headers_follow_track_artist_across_pages() {
        let mut catalog = Catalog::open_in_memory(1).unwrap();
        let mut zed = metric_track(1, SourceKind::Local, "Track Z");
        zed.artist = "Zed Performer".to_string();
        zed.album_artist = "Alpha Album Artist".to_string();
        let mut alpha = metric_track(2, SourceKind::Plex, "Track A");
        alpha.artist = "Alpha Performer".to_string();
        alpha.album_artist = "Zed Album Artist".to_string();
        catalog.upsert_tracks(&[zed, alpha]).unwrap();

        let descriptor = QueryDescriptor::tracks().with_group(TrackGroup::Artist);
        let first = catalog.query_tracks(&descriptor, None, 1).unwrap();
        let first_wire = map_entries(&first.rows, TrackGroup::Artist, None);
        assert!(first_wire[0].group_start);
        assert_eq!(first_wire[0].group_label, "Alpha Performer");

        let cursor = first.next_cursor.expect("second artist group page");
        let previous = cursor.group_key(TrackGroup::Artist);
        let second = catalog.query_tracks(&descriptor, Some(&cursor), 1).unwrap();
        let second_wire = map_entries(&second.rows, TrackGroup::Artist, Some(&previous));
        assert!(second_wire[0].group_start);
        assert_eq!(second_wire[0].group_label, "Zed Performer");
    }

    #[test]
    fn native_first_page_transport_and_broad_search_are_bounded() {
        const TOTAL: u64 = 20_000;
        let mut catalog = Catalog::open_in_memory(9).unwrap();
        let source_kinds = [
            SourceKind::Local,
            SourceKind::Plex,
            SourceKind::Jellyfin,
            SourceKind::Subsonic,
        ];
        let projected = (0..TOTAL)
            .map(|index| {
                metric_track(
                    index,
                    source_kinds[index as usize % source_kinds.len()],
                    &format!("Track {index:06}"),
                )
            })
            .collect::<Vec<_>>();
        catalog.upsert_tracks(&projected).unwrap();

        let descriptor = QueryDescriptor::tracks();
        let count_started = Instant::now();
        let count = catalog.count_tracks(&descriptor).unwrap();
        let count_time = count_started.elapsed();
        let (page, query_metrics) = catalog
            .query_tracks_timed(&descriptor, None, PAGE_ROWS)
            .unwrap();
        let map_started = Instant::now();
        let wire = map_entries(&page.rows, TrackGroup::Off, None);
        let map_time = map_started.elapsed();
        let serialize_started = Instant::now();
        let bytes = serde_json::to_vec(&wire).unwrap().len();
        let serialize_time = serialize_started.elapsed();

        let broad = descriptor.with_search("Track");
        let broad_started = Instant::now();
        let broad_count = catalog.count_tracks(&broad).unwrap();
        let broad_count_time = broad_started.elapsed();
        let (broad_page, broad_metrics) =
            catalog.query_tracks_timed(&broad, None, PAGE_ROWS).unwrap();
        let counts = catalog
            .stats()
            .unwrap()
            .source_counts
            .into_iter()
            .map(|(source, rows)| format!("{}={rows}", source.source.as_str()))
            .collect::<Vec<_>>()
            .join(",");

        assert_eq!(count, TOTAL);
        assert_eq!(page.rows.len(), PAGE_ROWS);
        assert!(page.has_more);
        assert_eq!(broad_count, TOTAL);
        assert_eq!(broad_page.rows.len(), PAGE_ROWS);
        assert!(bytes < 256 * 1024, "bounded page JSON was {bytes} bytes");
        println!(
            "E_NATIVE_METRIC total={TOTAL} first_rows={} json_bytes={bytes} count_ms={:.3} query_ms={:.3} map_ms={:.3} serialize_ms={:.3} broad_count_ms={:.3} broad_query_ms={:.3} source_counts={counts} qml_track_json_bytes=0",
            page.rows.len(),
            count_time.as_secs_f64() * 1_000.0,
            query_metrics.sql_time.as_secs_f64() * 1_000.0,
            map_time.as_secs_f64() * 1_000.0,
            serialize_time.as_secs_f64() * 1_000.0,
            broad_count_time.as_secs_f64() * 1_000.0,
            broad_metrics.sql_time.as_secs_f64() * 1_000.0,
        );
    }

    fn metric_track(index: u64, source: SourceKind, title: &str) -> ProjectedTrack {
        ProjectedTrack {
            track_ref: TrackRef {
                source,
                source_instance: format!("{}-fixture", source.as_str()),
                native_id: index.to_string(),
            },
            source_raw: source.as_str().to_string(),
            local_track_id: (source == SourceKind::Local).then_some(index as i64 + 1),
            local_path: (source == SourceKind::Local).then(|| format!("/fixture/{index}.flac")),
            native_album_id: Some(format!("album-{}", index / 10)),
            source_copy_id: None,
            title: title.to_string(),
            artist: format!("Artist {}", index % 401),
            album_artist: format!("Artist {}", index % 401),
            album: format!("Album {}", index % 997),
            duration_ms: 180_000,
            year: Some(1980 + (index % 45) as u32),
            disc_number: Some(1),
            track_number: Some((index % 20 + 1) as u32),
            format: "flac".to_string(),
            bit_depth: Some(24),
            sample_rate_hz: Some(96_000),
            artwork_token: None,
            isrc: None,
            musicbrainz_recording_id: None,
            added_at: index as i64,
            available: true,
            observed_generation: 1,
            credits: Vec::new(),
        }
    }
}
