use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OpenFlags, Row};
use serde::Deserialize;

use crate::{
    ActiveCatalog, ArtistCredit, BootstrapBatch, BootstrapLayout, BootstrapOutcome,
    BootstrapProgress, CatalogError, CreditRole, ProjectedTrack, Result, SourceKey, SourceKind,
    SourceProbe, TrackRef, BOOTSTRAP_BATCH_ROWS,
};
use crate::{ProjectionOutcome, ProjectionProgress, ReconciliationBatch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyTable {
    Local,
    Plex,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySourceSpec {
    pub source: SourceKey,
    pub database_path: PathBuf,
    table: LegacyTable,
}

/// Physical locations of the authoritative caches for one user. Qt keeps the
/// local and remote mirrors in the user's directory while the existing Plex
/// cache is installation-wide, so assuming co-location would silently build
/// an empty catalog for real profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyLocations {
    pub catalog_dir: PathBuf,
    pub local_database: PathBuf,
    pub plex_database: PathBuf,
    pub remote_database: PathBuf,
    /// Sources authorized for this user right now. Disabled or incomplete
    /// integrations are neither opened nor retained in the derived catalog.
    pub enabled_sources: BTreeSet<SourceKind>,
}

impl LegacyLocations {
    pub fn co_located(data_dir: impl Into<PathBuf>) -> Self {
        let catalog_dir = data_dir.into();
        Self {
            local_database: catalog_dir.join("library.db"),
            plex_database: catalog_dir.join("plex_cache.db"),
            remote_database: catalog_dir.join("remote_cache.db"),
            enabled_sources: SourceKind::ALL.into_iter().collect(),
            catalog_dir,
        }
    }
}

pub fn discover_legacy_sources(data_dir: &Path) -> Result<Vec<LegacySourceSpec>> {
    discover_legacy_sources_at(&LegacyLocations::co_located(data_dir))
}

pub fn discover_legacy_sources_at(locations: &LegacyLocations) -> Result<Vec<LegacySourceSpec>> {
    let mut specs = Vec::new();
    if locations.enabled_sources.contains(&SourceKind::Local)
        || locations.enabled_sources.contains(&SourceKind::Offline)
    {
        discover_local(&locations.local_database, &mut specs)?;
    }
    if locations.enabled_sources.contains(&SourceKind::Plex) {
        discover_plex(&locations.plex_database, &mut specs)?;
    }
    if locations.enabled_sources.contains(&SourceKind::Jellyfin)
        || locations.enabled_sources.contains(&SourceKind::Subsonic)
    {
        discover_remote(&locations.remote_database, &mut specs)?;
        specs.retain(|spec| locations.enabled_sources.contains(&spec.source.source));
    }
    specs.sort_by(|left, right| left.source.cmp(&right.source));
    specs.dedup_by(|left, right| left.source == right.source);
    Ok(specs)
}

pub fn bootstrap_legacy_caches(
    data_dir: &Path,
    cancelled: &AtomicBool,
) -> Result<BootstrapOutcome> {
    bootstrap_legacy_caches_with_progress(data_dir, cancelled, |_| {})
}

pub fn bootstrap_legacy_caches_with_progress(
    data_dir: &Path,
    cancelled: &AtomicBool,
    publish: impl FnMut(&BootstrapProgress),
) -> Result<BootstrapOutcome> {
    bootstrap_legacy_caches_at_with_progress(
        &LegacyLocations::co_located(data_dir),
        cancelled,
        publish,
    )
}

pub fn bootstrap_legacy_caches_at_with_progress(
    locations: &LegacyLocations,
    cancelled: &AtomicBool,
    mut publish: impl FnMut(&BootstrapProgress),
) -> Result<BootstrapOutcome> {
    let layout = BootstrapLayout::new(&locations.catalog_dir);
    if let ActiveCatalog::Ready { catalog, manifest } = layout.open_active() {
        // Also performs the one-time migration for profiles created by builds
        // that published generations but never removed obsolete ones.
        layout.prune_obsolete_generations(&manifest);
        return Ok(BootstrapOutcome::Activated {
            generation: manifest.active_generation,
            track_count: catalog.stats()?.track_count,
            resumed_rows: 0,
        });
    }

    let specs = discover_legacy_sources_at(locations)?;
    let sidecars = locations.catalog_dir.join("metadata_sidecars.db");
    let mut readers = specs
        .into_iter()
        .map(|spec| LegacyReader::open(spec, &sidecars))
        .collect::<Result<Vec<_>>>()?;
    let probes = readers
        .iter()
        .map(|reader| reader.probe.clone())
        .collect::<Vec<_>>();
    let (mut session, _) = layout.prepare(&probes, None)?;
    let mut resumed_rows = 0_u64;
    let mut committed_rows = 0_u64;
    let overall_rows_total = probes.iter().map(|probe| probe.row_count).sum::<u64>();
    let source_count = readers.len();
    let mut overall_rows_before_source = 0_u64;

    for (source_offset, reader) in readers.iter_mut().enumerate() {
        let mut checkpoint = session.checkpoint(&reader.spec.source)?;
        if let Some(saved) = &checkpoint {
            if saved.checkpoint_version != reader.probe.snapshot_version
                || saved.checkpoint_rows > reader.probe.row_count
            {
                session
                    .restart_changed_source(&reader.spec.source, &reader.probe.snapshot_version)?;
                checkpoint = None;
            } else {
                resumed_rows = resumed_rows.saturating_add(saved.checkpoint_rows);
            }
        }
        if checkpoint
            .as_ref()
            .is_some_and(|saved| saved.complete && saved.checkpoint_rows == reader.probe.row_count)
        {
            overall_rows_before_source =
                overall_rows_before_source.saturating_add(reader.probe.row_count);
            continue;
        }

        let mut cursor = checkpoint
            .map(|saved| saved.checkpoint_cursor)
            .unwrap_or_default();
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Ok(BootstrapOutcome::Paused {
                    generation: session.generation(),
                    source: Some(reader.spec.source.clone()),
                    committed_rows,
                });
            }
            let batch = reader.read_batch(&cursor)?;
            let saved = session.apply_batch(&batch)?;
            committed_rows = committed_rows.saturating_add(batch.tracks.len() as u64);
            cursor = saved.checkpoint_cursor;
            publish(&BootstrapProgress {
                generation: session.generation(),
                source: reader.spec.source.clone(),
                committed_rows: saved.checkpoint_rows,
                source_rows_total: reader.probe.row_count,
                overall_rows_written: overall_rows_before_source
                    .saturating_add(saved.checkpoint_rows.min(reader.probe.row_count)),
                overall_rows_total,
                source_index: source_offset + 1,
                source_count,
                checkpoint_cursor: cursor.clone(),
                source_complete: saved.complete,
            });
            if saved.complete {
                break;
            }
            std::thread::yield_now();
            std::thread::sleep(Duration::from_millis(1));
        }
        overall_rows_before_source =
            overall_rows_before_source.saturating_add(reader.probe.row_count);
    }

    if cancelled.load(Ordering::Acquire) {
        return Ok(BootstrapOutcome::Paused {
            generation: session.generation(),
            source: None,
            committed_rows,
        });
    }
    let stats = session.stats()?;
    let manifest = session.activate(&probes)?;
    Ok(BootstrapOutcome::Activated {
        generation: manifest.active_generation,
        track_count: stats.track_count,
        resumed_rows,
    })
}

pub fn reconcile_legacy_caches(
    data_dir: &Path,
    cancelled: &AtomicBool,
) -> Result<ProjectionOutcome> {
    reconcile_legacy_caches_with_progress(data_dir, cancelled, |_| {})
}

pub fn reconcile_legacy_caches_with_progress(
    data_dir: &Path,
    cancelled: &AtomicBool,
    publish: impl FnMut(&ProjectionProgress),
) -> Result<ProjectionOutcome> {
    reconcile_legacy_caches_at_with_progress(
        &LegacyLocations::co_located(data_dir),
        cancelled,
        publish,
    )
}

pub fn reconcile_legacy_caches_at_with_progress(
    locations: &LegacyLocations,
    cancelled: &AtomicBool,
    mut publish: impl FnMut(&ProjectionProgress),
) -> Result<ProjectionOutcome> {
    let layout = BootstrapLayout::new(&locations.catalog_dir);
    let (active, active_generation) = match layout.open_active() {
        ActiveCatalog::Ready { catalog, manifest } => (catalog, manifest.active_generation),
        ActiveCatalog::Fallback(reason) => {
            return Err(CatalogError::ActivationNotReady(format!(
                "reconciliation fallback: {reason:?}"
            )))
        }
    };
    let active_sources = active.stats()?.source_counts;
    let specs = discover_legacy_sources_at(locations)?;
    let sidecars = locations.catalog_dir.join("metadata_sidecars.db");
    let mut readers = specs
        .into_iter()
        .map(|spec| LegacyReader::open(spec, &sidecars))
        .collect::<Result<Vec<_>>>()?;
    let mut probes = readers
        .iter()
        .map(|reader| reader.probe.clone())
        .collect::<Vec<_>>();
    let disabled_probes = active_sources
        .into_iter()
        .filter(|(source, rows)| {
            *rows > 0 && !locations.enabled_sources.contains(&source.source)
        })
        .map(|(source, _)| SourceProbe {
            source: source.clone(),
            source_path: match source.source {
                SourceKind::Local | SourceKind::Offline => locations.local_database.clone(),
                SourceKind::Plex => locations.plex_database.clone(),
                SourceKind::Jellyfin | SourceKind::Subsonic => locations.remote_database.clone(),
            },
            snapshot_version: "disabled:v1".to_string(),
            row_count: 0,
            page_bytes: 0,
            integrity_ok: true,
        })
        .collect::<Vec<_>>();
    probes.extend(disabled_probes.iter().cloned());
    let mut changed_keys = readers
        .iter()
        .filter_map(|reader| {
            let checkpoint = active.source_checkpoint(&reader.spec.source).ok().flatten();
            (!checkpoint.is_some_and(|checkpoint| {
                checkpoint.complete_generation > 0
                    && checkpoint.checkpoint_version == reader.probe.snapshot_version
                    && checkpoint.checkpoint_rows == reader.probe.row_count
            }))
            .then(|| reader.spec.source.clone())
        })
        .collect::<HashSet<_>>();
    changed_keys.extend(disabled_probes.iter().map(|probe| probe.source.clone()));
    if changed_keys.is_empty() {
        return Ok(ProjectionOutcome::UpToDate {
            generation: active_generation,
            track_count: active.stats()?.track_count,
        });
    }
    drop(active);

    let changed_probes = probes
        .iter()
        .filter(|probe| changed_keys.contains(&probe.source))
        .cloned()
        .collect::<Vec<_>>();
    let (mut session, _) = layout.prepare_projection(&probes, None)?;
    let mut resumed_rows = 0_u64;
    let mut committed_rows = 0_u64;
    let overall_rows_total = changed_probes
        .iter()
        .map(|probe| probe.row_count)
        .sum::<u64>();
    let source_count = changed_probes.len();
    let mut overall_rows_before_source = 0_u64;
    let changed_reader_count = readers
        .iter()
        .filter(|reader| changed_keys.contains(&reader.spec.source))
        .count();
    for (source_offset, reader) in readers
        .iter_mut()
        .filter(|reader| changed_keys.contains(&reader.spec.source))
        .enumerate()
    {
        let checkpoint = session.checkpoint(&reader.spec.source)?;
        let mut cursor = if let Some(checkpoint) = checkpoint {
            if checkpoint.complete
                && checkpoint.checkpoint_version == reader.probe.snapshot_version
                && checkpoint.checkpoint_rows == reader.probe.row_count
            {
                overall_rows_before_source =
                    overall_rows_before_source.saturating_add(reader.probe.row_count);
                continue;
            } else if checkpoint.checkpoint_version == reader.probe.snapshot_version
                && !checkpoint.complete
                && checkpoint.checkpoint_rows <= reader.probe.row_count
            {
                resumed_rows = resumed_rows.saturating_add(checkpoint.checkpoint_rows);
                checkpoint.checkpoint_cursor
            } else {
                session.begin_source(&reader.spec.source, &reader.probe.snapshot_version)?;
                String::new()
            }
        } else {
            session.begin_source(&reader.spec.source, &reader.probe.snapshot_version)?;
            String::new()
        };
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Ok(ProjectionOutcome::Paused {
                    generation: session.generation(),
                    source: reader.spec.source.clone(),
                    committed_rows,
                });
            }
            let batch = reader.read_batch(&cursor)?;
            let projection_batch = ReconciliationBatch {
                source: batch.source,
                snapshot_version: batch.snapshot_version,
                expected_cursor: batch.expected_cursor,
                next_cursor: batch.next_cursor,
                tracks: batch.tracks,
                complete: batch.complete,
            };
            let saved = session.apply_batch(&projection_batch)?;
            committed_rows = committed_rows.saturating_add(projection_batch.tracks.len() as u64);
            cursor = saved.checkpoint_cursor;
            publish(&ProjectionProgress {
                generation: session.generation(),
                source: reader.spec.source.clone(),
                rows_written: saved.checkpoint_rows,
                source_rows_total: reader.probe.row_count,
                overall_rows_written: overall_rows_before_source
                    .saturating_add(saved.checkpoint_rows.min(reader.probe.row_count)),
                overall_rows_total,
                source_index: source_offset + 1,
                source_count,
                checkpoint_cursor: cursor.clone(),
                source_complete: saved.complete,
                prune_authorized: saved.complete,
            });
            if saved.complete {
                break;
            }
            std::thread::yield_now();
            std::thread::sleep(Duration::from_millis(1));
        }
        overall_rows_before_source =
            overall_rows_before_source.saturating_add(reader.probe.row_count);
    }
    for (disabled_offset, probe) in disabled_probes.iter().enumerate() {
        if cancelled.load(Ordering::Acquire) {
            return Ok(ProjectionOutcome::Paused {
                generation: session.generation(),
                source: probe.source.clone(),
                committed_rows,
            });
        }
        session.begin_source(&probe.source, &probe.snapshot_version)?;
        let saved = session.apply_batch(&ReconciliationBatch {
            source: probe.source.clone(),
            snapshot_version: probe.snapshot_version.clone(),
            expected_cursor: String::new(),
            next_cursor: String::new(),
            tracks: Vec::new(),
            complete: true,
        })?;
        publish(&ProjectionProgress {
            generation: session.generation(),
            source: probe.source.clone(),
            rows_written: 0,
            source_rows_total: 0,
            overall_rows_written: overall_rows_before_source,
            overall_rows_total,
            source_index: changed_reader_count + disabled_offset + 1,
            source_count,
            checkpoint_cursor: saved.checkpoint_cursor,
            source_complete: true,
            prune_authorized: true,
        });
    }
    let stats = session.stats()?;
    let manifest = session.activate(&changed_probes)?;
    Ok(ProjectionOutcome::Activated {
        generation: manifest.active_generation,
        track_count: stats.track_count,
        changed_sources: changed_probes.len(),
        resumed_rows,
    })
}

fn discover_local(path: &Path, specs: &mut Vec<LegacySourceSpec>) -> Result<()> {
    let Some(conn) = open_if_present(path)? else {
        return Ok(());
    };
    if table_exists(&conn, "local_tracks")? {
        specs.push(LegacySourceSpec {
            source: SourceKey {
                source: SourceKind::Local,
                source_instance: "library".to_string(),
            },
            database_path: path.to_path_buf(),
            table: LegacyTable::Local,
        });
    }
    Ok(())
}

fn discover_plex(path: &Path, specs: &mut Vec<LegacySourceSpec>) -> Result<()> {
    let Some(conn) = open_if_present(path)? else {
        return Ok(());
    };
    if !table_exists(&conn, "plex_cache_tracks")? {
        return Ok(());
    }
    let mut instances = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT COALESCE(NULLIF(server_id,''),'default')
           FROM plex_cache_tracks",
    )?;
    for value in stmt.query_map([], |row| row.get::<_, String>(0))? {
        instances.insert(value?);
    }
    if table_exists(&conn, "plex_cache_sections")? {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT COALESCE(NULLIF(server_id,''),'default')
               FROM plex_cache_sections",
        )?;
        for value in stmt.query_map([], |row| row.get::<_, String>(0))? {
            instances.insert(value?);
        }
    }
    if instances.is_empty() {
        instances.insert("default".to_string());
    }
    for source_instance in instances {
        specs.push(LegacySourceSpec {
            source: SourceKey {
                source: SourceKind::Plex,
                source_instance,
            },
            database_path: path.to_path_buf(),
            table: LegacyTable::Plex,
        });
    }
    Ok(())
}

fn discover_remote(path: &Path, specs: &mut Vec<LegacySourceSpec>) -> Result<()> {
    let Some(conn) = open_if_present(path)? else {
        return Ok(());
    };
    if !table_exists(&conn, "remote_cache_tracks")? {
        return Ok(());
    }
    let mut keys = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT source, COALESCE(NULLIF(server_id,''),'default')
           FROM remote_cache_tracks",
    )?;
    for value in stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        keys.insert(value?);
    }
    if table_exists(&conn, "remote_cache_libraries")? {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT source, COALESCE(NULLIF(server_id,''),'default')
               FROM remote_cache_libraries",
        )?;
        for value in stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            keys.insert(value?);
        }
    }
    for (word, source_instance) in keys {
        let Some(source) = SourceKind::from_str(&word) else {
            continue;
        };
        if !matches!(source, SourceKind::Jellyfin | SourceKind::Subsonic) {
            continue;
        }
        specs.push(LegacySourceSpec {
            source: SourceKey {
                source,
                source_instance,
            },
            database_path: path.to_path_buf(),
            table: LegacyTable::Remote,
        });
    }
    Ok(())
}

struct LegacyReader {
    spec: LegacySourceSpec,
    conn: Connection,
    probe: SourceProbe,
    overrides: RemoteOverrides,
}

impl LegacyReader {
    fn open(spec: LegacySourceSpec, sidecar_path: &Path) -> Result<Self> {
        let conn = open_read_only(&spec.database_path)?;
        conn.execute_batch("PRAGMA query_only=ON; BEGIN;")?;
        let quick_check: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let page_count: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let schema_version: i64 = conn.query_row("PRAGMA schema_version", [], |row| row.get(0))?;
        let (row_count, maximum_id, maximum_updated) = source_summary(&conn, &spec)?;
        let file_epoch = sqlite_file_epoch(&spec.database_path)?;
        let overrides = RemoteOverrides::load(sidecar_path, &spec.source)?;
        let snapshot_version = format!(
            "v2:{schema_version}:{page_count}:{row_count}:{maximum_id}:{maximum_updated}:{file_epoch}:{}",
            overrides.revision
        );
        let probe = SourceProbe {
            source: spec.source.clone(),
            source_path: spec.database_path.clone(),
            snapshot_version,
            row_count,
            page_bytes: (page_size.max(0) as u64).saturating_mul(page_count.max(0) as u64),
            integrity_ok: quick_check == "ok",
        };
        Ok(Self {
            spec,
            conn,
            probe,
            overrides,
        })
    }

    fn read_batch(&self, cursor: &str) -> Result<BootstrapBatch> {
        let after = if cursor.is_empty() {
            0
        } else {
            cursor.parse::<i64>().map_err(|_| {
                CatalogError::InvalidInput(format!("invalid bootstrap cursor {cursor:?}"))
            })?
        };
        let mut tracks = match self.spec.table {
            LegacyTable::Local => read_local(&self.conn, &self.spec, after)?,
            LegacyTable::Plex => read_plex(&self.conn, &self.spec, after)?,
            LegacyTable::Remote => read_remote(&self.conn, &self.spec, after)?,
        };
        for (_, track) in &mut tracks {
            self.overrides.apply(track);
        }
        let complete = tracks.len() <= BOOTSTRAP_BATCH_ROWS;
        tracks.truncate(BOOTSTRAP_BATCH_ROWS);
        let next_cursor = tracks
            .last()
            .map(|track| legacy_cursor(track).to_string())
            .unwrap_or_else(|| cursor.to_string());
        Ok(BootstrapBatch {
            source: self.spec.source.clone(),
            snapshot_version: self.probe.snapshot_version.clone(),
            expected_cursor: cursor.to_string(),
            next_cursor,
            tracks: tracks.into_iter().map(|(_, track)| track).collect(),
            complete,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarAlbum {
    album_title: Option<String>,
    album_artist: Option<String>,
    year: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarTrack {
    file_path: String,
    title: Option<String>,
    disc_number: Option<u32>,
    track_number: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarExtendedAlbum {
    artwork_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarExtendedTrack {
    file_path: String,
    artist_credit: String,
    musicbrainz_recording_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarDocument {
    #[serde(default)]
    album: SidecarAlbum,
    #[serde(default)]
    tracks: Vec<SidecarTrack>,
    extended_album: Option<SidecarExtendedAlbum>,
    #[serde(default)]
    extended_tracks: Vec<SidecarExtendedTrack>,
}

#[derive(Default)]
struct RemoteOverrides {
    by_album: HashMap<String, SidecarDocument>,
    revision: String,
}

impl RemoteOverrides {
    fn load(path: &Path, source: &SourceKey) -> Result<Self> {
        if matches!(source.source, SourceKind::Local | SourceKind::Offline) || !path.is_file() {
            return Ok(Self::default());
        }
        let conn = open_read_only(path)?;
        if !table_exists(&conn, "remote_album_tag_sidecars")? {
            return Ok(Self::default());
        }
        let mut stmt = conn.prepare(
            "SELECT album_id,payload_json,updated_at
               FROM remote_album_tag_sidecars
              WHERE source=?1 AND source_instance=?2",
        )?;
        let rows = stmt.query_map(
            params![source.source.as_str(), source.source_instance],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )?;
        let mut by_album = HashMap::new();
        let mut count = 0_u64;
        let mut maximum_updated = 0_i64;
        for row in rows {
            let (album_id, json, updated_at) = row?;
            count = count.saturating_add(1);
            maximum_updated = maximum_updated.max(updated_at);
            if let Ok(document) = serde_json::from_str(&json) {
                by_album.insert(album_id, document);
            }
        }
        let file_epoch = sqlite_file_epoch(path)?;
        Ok(Self {
            by_album,
            revision: format!("{count}:{maximum_updated}:{file_epoch}"),
        })
    }

    fn apply(&self, track: &mut ProjectedTrack) {
        let Some(album_id) = track.native_album_id.as_deref() else {
            return;
        };
        let Some(sidecar) = self.by_album.get(album_id) else {
            return;
        };
        if let Some(title) = sidecar
            .album
            .album_title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            track.album = title.to_string();
        }
        if let Some(artist) = sidecar.album.album_artist.as_deref() {
            track.album_artist = artist.trim().to_string();
        }
        if let Some(year) = sidecar.album.year {
            track.year = (year != 0).then_some(year);
        }
        if let Some(path) = sidecar
            .extended_album
            .as_ref()
            .and_then(|album| album.artwork_path.as_deref())
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            track.artwork_token = Some(format!("{}{path}", crate::SIDECAR_LOCAL_ART_PREFIX));
        }
        if let Some(row) = sidecar
            .tracks
            .iter()
            .find(|row| row.file_path == track.track_ref.native_id)
        {
            if let Some(title) = row
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
            {
                track.title = title.to_string();
            }
            if let Some(number) = row.disc_number {
                track.disc_number = (number != 0).then_some(number);
            }
            if let Some(number) = row.track_number {
                track.track_number = (number != 0).then_some(number);
            }
        }
        if let Some(row) = sidecar
            .extended_tracks
            .iter()
            .find(|row| row.file_path == track.track_ref.native_id)
        {
            if !row.artist_credit.trim().is_empty() {
                track.artist = row.artist_credit.trim().to_string();
            }
            track.musicbrainz_recording_id = row.musicbrainz_recording_id.clone();
        }
        track.credits = credits(&track.artist, &track.album_artist);
    }
}

type LegacyRow = (i64, ProjectedTrack);

/// An identity column is either an id or nothing — an empty string must not
/// become a key that joins every untagged row to every other one.
fn nonempty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn legacy_cursor(row: &LegacyRow) -> i64 {
    row.0
}

fn read_local(conn: &Connection, spec: &LegacySourceSpec, after: i64) -> Result<Vec<LegacyRow>> {
    let columns = table_columns(conn, "local_tracks")?;
    let album_id = optional_column(&columns, "album_group_key", "NULL");
    let source_raw = optional_column(&columns, "source", "''");
    // Identity columns post-date the library schema; NULL on a db that has
    // not migrated yet (the upsert already binds both).
    let isrc = optional_column(&columns, "isrc", "NULL");
    let mbid = optional_column(&columns, "musicbrainz_recording_id", "NULL");
    let sql = format!(
        "SELECT id, file_path, title, artist, COALESCE(album_artist,''), album,
                      duration_secs, year, disc_number, track_number, format, bit_depth,
                      CAST(sample_rate AS INTEGER), artwork_path, indexed_at, {album_id},
                      COALESCE({source_raw},''), {isrc}, {mbid}
                 FROM local_tracks
                WHERE id > ?1
                ORDER BY id
                LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![after, BOOTSTRAP_BATCH_ROWS as i64 + 1], |row| {
        let id: i64 = row.get(0)?;
        let artist: String = row.get(3)?;
        let album_artist: String = row.get(4)?;
        let credits = credits(&artist, &album_artist);
        Ok((
            id,
            ProjectedTrack {
                track_ref: TrackRef {
                    source: SourceKind::Local,
                    source_instance: spec.source.source_instance.clone(),
                    native_id: id.to_string(),
                },
                source_raw: row.get(16)?,
                local_track_id: Some(id),
                local_path: row.get(1)?,
                native_album_id: row.get(15)?,
                source_copy_id: None,
                title: row.get(2)?,
                artist,
                album_artist,
                album: row.get(5)?,
                duration_ms: nonnegative(row.get::<_, Option<i64>>(6)?.unwrap_or(0))
                    .saturating_mul(1_000),
                year: optional_u32(row, 7)?,
                disc_number: optional_u32(row, 8)?,
                track_number: optional_u32(row, 9)?,
                format: row.get::<_, String>(10)?.to_ascii_lowercase(),
                bit_depth: optional_u32(row, 11)?,
                sample_rate_hz: optional_u32(row, 12)?,
                artwork_token: row.get(13)?,
                isrc: nonempty(row.get(17)?),
                musicbrainz_recording_id: nonempty(row.get(18)?),
                added_at: row.get::<_, Option<i64>>(14)?.unwrap_or(0),
                available: true,
                observed_generation: 0,
                credits,
            },
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn read_plex(conn: &Connection, spec: &LegacySourceSpec, after: i64) -> Result<Vec<LegacyRow>> {
    let columns = table_columns(conn, "plex_cache_tracks")?;
    require_columns(
        &columns,
        &["rating_key", "title", "artist", "album", "duration_ms"],
        "plex_cache_tracks",
    )?;
    let item_art = optional_column(&columns, "artwork_path", "NULL");
    let collection_art = optional_column(&columns, "collection_artwork_path", "NULL");
    let metadata_art = if columns.contains("parent_rating_key")
        && table_exists(conn, "plex_cache_album_artwork")?
    {
        "(SELECT NULLIF(album_art.artwork_path,'')
            FROM plex_cache_album_artwork album_art
           WHERE album_art.server_id =
                 COALESCE(NULLIF(plex_cache_tracks.server_id,''),'default')
             AND album_art.parent_rating_key = plex_cache_tracks.parent_rating_key)"
            .to_string()
    } else {
        "NULL".to_string()
    };
    // Track art keeps its existing precedence on track surfaces. Parent art
    // and the album-metadata repair are fallback-only, so filling a missing
    // card cannot replace a legitimate disc/track-specific image.
    let effective_art =
        format!("COALESCE(NULLIF({item_art},''),NULLIF({collection_art},''),{metadata_art})");
    let sql = format!(
        "SELECT rowid, rating_key, title, COALESCE(artist,''), COALESCE(album,''),
                COALESCE(duration_ms,0), {year}, {disc}, {track}, {format}, {depth},
                {rate}, {art}, {updated}, {album_id}, {mbid}
           FROM plex_cache_tracks
          WHERE rowid > ?1
            AND COALESCE(NULLIF(server_id,''),'default') = ?2
          ORDER BY rowid
          LIMIT ?3",
        year = optional_column(&columns, "year", "NULL"),
        disc = optional_column(&columns, "disc_number", "NULL"),
        track = optional_column(&columns, "track_number", "NULL"),
        format = format_expression(&columns),
        depth = optional_column(&columns, "bit_depth", "NULL"),
        rate = optional_column(&columns, "sampling_rate_hz", "NULL"),
        art = effective_art,
        updated = optional_column(&columns, "updated_at", "0"),
        album_id = if columns.contains("parent_rating_key") {
            "parent_rating_key".to_string()
        } else {
            optional_column(&columns, "album_key", "NULL")
        },
        mbid = optional_column(&columns, "recording_mbid", "NULL"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            after,
            spec.source.source_instance,
            BOOTSTRAP_BATCH_ROWS as i64 + 1
        ],
        |row| {
            let mut mapped = map_remote_like(row, spec, SourceKind::Plex)?;
            mapped.1.native_album_id = row.get(14)?;
            mapped.1.musicbrainz_recording_id = nonempty(row.get(15)?);
            Ok(mapped)
        },
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn read_remote(conn: &Connection, spec: &LegacySourceSpec, after: i64) -> Result<Vec<LegacyRow>> {
    let columns = table_columns(conn, "remote_cache_tracks")?;
    require_columns(
        &columns,
        &[
            "id",
            "item_id",
            "title",
            "artist",
            "album_artist",
            "album",
            "duration_ms",
        ],
        "remote_cache_tracks",
    )?;
    let sql = format!(
        "SELECT id, item_id, title, artist, album, duration_ms, {year}, {disc}, {track},
                {format}, {depth}, {rate}, {art}, {updated}, album_artist, {album_id},
                {isrc}, {mbid}
           FROM remote_cache_tracks
          WHERE id > ?1 AND source = ?2
            AND COALESCE(NULLIF(server_id,''),'default') = ?3
          ORDER BY id
          LIMIT ?4",
        year = optional_column(&columns, "year", "NULL"),
        disc = optional_column(&columns, "disc_number", "NULL"),
        track = optional_column(&columns, "track_number", "NULL"),
        format = format_expression(&columns),
        depth = optional_column(&columns, "bit_depth", "NULL"),
        rate = optional_column(&columns, "sample_rate_hz", "NULL"),
        art = optional_column(&columns, "artwork_token", "NULL"),
        updated = optional_column(&columns, "updated_at", "0"),
        album_id = optional_column(&columns, "album_id", "NULL"),
        isrc = optional_column(&columns, "isrc", "NULL"),
        mbid = optional_column(&columns, "recording_mbid", "NULL"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            after,
            spec.source.source.as_str(),
            spec.source.source_instance,
            BOOTSTRAP_BATCH_ROWS as i64 + 1
        ],
        |row| {
            let mut mapped = map_remote_like(row, spec, spec.source.source)?;
            mapped.1.album_artist = row.get(14)?;
            mapped.1.native_album_id = row.get(15)?;
            mapped.1.isrc = nonempty(row.get(16)?);
            mapped.1.musicbrainz_recording_id = nonempty(row.get(17)?);
            mapped.1.credits = credits(&mapped.1.artist, &mapped.1.album_artist);
            Ok(mapped)
        },
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn map_remote_like(
    row: &Row<'_>,
    spec: &LegacySourceSpec,
    source: SourceKind,
) -> rusqlite::Result<LegacyRow> {
    let cursor: i64 = row.get(0)?;
    let artist: String = row.get(3)?;
    Ok((
        cursor,
        ProjectedTrack {
            track_ref: TrackRef {
                source,
                source_instance: spec.source.source_instance.clone(),
                native_id: value_as_string(row, 1)?,
            },
            source_raw: source.as_str().to_string(),
            local_track_id: None,
            local_path: None,
            native_album_id: None,
            source_copy_id: None,
            title: row.get(2)?,
            artist: artist.clone(),
            album_artist: artist.clone(),
            album: row.get(4)?,
            duration_ms: nonnegative(row.get::<_, Option<i64>>(5)?.unwrap_or(0)),
            year: optional_u32(row, 6)?,
            disc_number: optional_u32(row, 7)?,
            track_number: optional_u32(row, 8)?,
            format: row
                .get::<_, Option<String>>(9)?
                .unwrap_or_default()
                .to_ascii_lowercase(),
            bit_depth: optional_u32(row, 10)?,
            sample_rate_hz: optional_u32(row, 11)?,
            artwork_token: row.get(12)?,
            isrc: None,
            musicbrainz_recording_id: None,
            added_at: row.get::<_, Option<i64>>(13)?.unwrap_or(0),
            available: true,
            observed_generation: 0,
            credits: credits(&artist, &artist),
        },
    ))
}

fn source_summary(conn: &Connection, spec: &LegacySourceSpec) -> Result<(u64, i64, i64)> {
    let plex_artwork_updated =
        if spec.table == LegacyTable::Plex && table_exists(conn, "plex_cache_album_artwork")? {
            "(SELECT COALESCE(MAX(checked_at),0)
            FROM plex_cache_album_artwork
           WHERE server_id=?1)"
        } else {
            "0"
        };
    let (sql, params) = match spec.table {
        LegacyTable::Local => (
            "SELECT COUNT(*), COALESCE(MAX(id),0), COALESCE(MAX(indexed_at),0)
               FROM local_tracks"
                .to_string(),
            Vec::new(),
        ),
        LegacyTable::Plex => (
            format!(
                "SELECT COUNT(*), COALESCE(MAX(rowid),0),
                        MAX(COALESCE(MAX(updated_at),0),{plex_artwork_updated})
               FROM plex_cache_tracks
              WHERE COALESCE(NULLIF(server_id,''),'default') = ?1"
            ),
            vec![spec.source.source_instance.clone()],
        ),
        LegacyTable::Remote => (
            "SELECT COUNT(*), COALESCE(MAX(id),0), COALESCE(MAX(updated_at),0)
               FROM remote_cache_tracks
              WHERE source = ?1
                AND COALESCE(NULLIF(server_id,''),'default') = ?2"
                .to_string(),
            vec![
                spec.source.source.as_str().to_string(),
                spec.source.source_instance.clone(),
            ],
        ),
    };
    let mut stmt = conn.prepare(&sql)?;
    let values = params.iter().map(String::as_str).collect::<Vec<_>>();
    let (count, maximum_id, maximum_updated): (i64, i64, i64) = stmt
        .query_row(rusqlite::params_from_iter(values), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    Ok((count.max(0) as u64, maximum_id, maximum_updated))
}

fn credits(artist: &str, album_artist: &str) -> Vec<ArtistCredit> {
    let mut values = Vec::new();
    if !artist.trim().is_empty() {
        values.push(ArtistCredit {
            display_name: artist.trim().to_string(),
            role: CreditRole::TrackArtist,
            ordinal: 0,
        });
    }
    if !album_artist.trim().is_empty() && !album_artist.eq_ignore_ascii_case(artist) {
        values.push(ArtistCredit {
            display_name: album_artist.trim().to_string(),
            role: CreditRole::AlbumArtist,
            ordinal: 0,
        });
    }
    values
}

fn nonnegative(value: i64) -> u64 {
    value.max(0) as u64
}

fn optional_u32(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<u32>> {
    Ok(row
        .get::<_, Option<i64>>(index)?
        .and_then(|value| u32::try_from(value).ok()))
}

fn value_as_string(row: &Row<'_>, index: usize) -> rusqlite::Result<String> {
    match row.get_ref(index)? {
        ValueRef::Text(value) => Ok(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Integer(value) => Ok(value.to_string()),
        _ => row.get(index),
    }
}

fn open_if_present(path: &Path) -> Result<Option<Connection>> {
    if !path.is_file() {
        return Ok(None);
    }
    open_read_only(path).map(Some)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(2_500))?;
    conn.execute_batch("PRAGMA query_only=ON;")?;
    Ok(conn)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )?)
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
}

fn require_columns(columns: &HashSet<String>, required: &[&str], table: &str) -> Result<()> {
    if let Some(column) = required.iter().find(|column| !columns.contains(**column)) {
        return Err(CatalogError::InvalidSource(format!(
            "{table} is missing required column {column}"
        )));
    }
    Ok(())
}

fn optional_column(columns: &HashSet<String>, column: &str, fallback: &str) -> String {
    if columns.contains(column) {
        column.to_string()
    } else {
        fallback.to_string()
    }
}

fn format_expression(columns: &HashSet<String>) -> String {
    match (columns.contains("codec"), columns.contains("container")) {
        (true, true) => "COALESCE(NULLIF(codec,''),NULLIF(container,''),'')".to_string(),
        (true, false) => "COALESCE(codec,'')".to_string(),
        (false, true) => "COALESCE(container,'')".to_string(),
        (false, false) => "''".to_string(),
    }
}

fn sqlite_file_epoch(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
    ] {
        match std::fs::metadata(candidate) {
            Ok(metadata) => {
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos())
                    .unwrap_or(0);
                parts.push(format!("{}:{modified}", metadata.len()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                parts.push("0:0".to_string())
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(parts.join(":"))
}
