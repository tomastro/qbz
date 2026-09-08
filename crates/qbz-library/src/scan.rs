//! Incremental, root-scoped local-library scanner.
//!
//! Enumeration is streaming and deterministic. Each root owns a persistent
//! generation/checkpoint, cheap fingerprints skip unchanged metadata, changed
//! files are extracted by a bounded worker pool, and SQLite writes are grouped
//! into bounded transactions. A root only prunes rows after both of its passes
//! finish without traversal errors. Cancellation, an inaccessible mount, or a
//! WalkDir error therefore means "not observed", never "deleted".

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, OptionalExtension};

use crate::{
    cue_to_tracks, AlbumTagSidecar, CueParser, CueSheet, LibraryDatabase, LibraryError,
    LibraryFolder, LibraryScanner, LocalTrack, MetadataExtractor, ScanError, ScanFileKind,
    ScanStatus,
};

const SCAN_BATCH_FILES: usize = 100;
const MAX_EXTRACT_WORKERS: usize = 8;
const PASS_CACHE_ENTRIES: usize = 2_048;

/// One step of a scan, pushed to the caller. Per-file events are intentionally
/// compatible with the existing Qt progress surface; root events carry the
/// incremental/prune telemetry without exposing full paths in logs.
pub enum ScanEvent {
    Started,
    TotalsAdded {
        total: u32,
    },
    FileStarted {
        path: String,
    },
    FileDone {
        processed: u32,
        total: u32,
    },
    RootStarted {
        root_id: i64,
        generation: u64,
        resumed: bool,
        is_network: bool,
    },
    RootFinished {
        root_id: i64,
        generation: u64,
        discovered: u64,
        extracted: u64,
        reused: u64,
        pruned: u64,
        prune_authorized: bool,
        elapsed: Duration,
    },
    Cleanup,
    Finished {
        status: ScanStatus,
        errors: Vec<ScanError>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanPhase {
    Cues,
    Audio,
}

impl ScanPhase {
    fn kind(self) -> ScanFileKind {
        match self {
            Self::Cues => ScanFileKind::Cue,
            Self::Audio => ScanFileKind::Audio,
        }
    }
}

#[derive(Debug, Clone)]
struct RootState {
    generation: u64,
    phase: ScanPhase,
    checkpoint_path: String,
    discovered: u64,
    processed: u64,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFingerprint {
    file_id: String,
    size_bytes: i64,
    mtime_ns: i64,
}

#[derive(Default)]
struct RootMetrics {
    extracted: u64,
    reused: u64,
    pruned: u64,
}

#[derive(Debug, Clone)]
enum ExtractTask {
    Audio { path: PathBuf, root: PathBuf },
    Cue { cue: CueSheet, audio_path: PathBuf },
}

trait ExtractionBackend: Sync {
    fn extract(&self, task: &ExtractTask) -> Result<Vec<LocalTrack>, String>;
}

struct RealExtraction;

impl ExtractionBackend for RealExtraction {
    fn extract(&self, task: &ExtractTask) -> Result<Vec<LocalTrack>, String> {
        match task {
            ExtractTask::Audio { path, root } => {
                MetadataExtractor::extract_with_roots(path, std::slice::from_ref(root))
                    .map(|track| vec![track])
                    .map_err(|error| error.to_string())
            }
            ExtractTask::Cue { cue, audio_path } => {
                let properties = MetadataExtractor::extract_properties(audio_path)
                    .map_err(|error| error.to_string())?;
                let format = MetadataExtractor::detect_format(audio_path);
                let tracks = cue_to_tracks(cue, properties.duration_secs, format, &properties);
                if tracks.is_empty() {
                    Err("CUE produced no tracks".to_string())
                } else {
                    Ok(tracks)
                }
            }
        }
    }
}

#[derive(Debug)]
enum PreparedAction {
    Reuse,
    Extract(ExtractTask),
    Preserve(String),
    /// A valid CUE with several FILE directives describes already-split
    /// audio. Keep the CUE scan record but publish no virtual tracks and
    /// retire rows produced by older single-file-only parsing.
    IgnoreMultiFileCue,
    SkipCueAudio,
}

#[derive(Debug)]
struct PreparedFile {
    traversal_path: String,
    canonical_path: String,
    kind: ScanFileKind,
    fingerprint: Option<FileFingerprint>,
    dependency: String,
    current_cue_audio_path: Option<String>,
    previous_cue_audio_path: Option<String>,
    action: PreparedAction,
}

struct ResolvedFile {
    prepared: PreparedFile,
    tracks: Option<Result<Vec<LocalTrack>, String>>,
}

struct BoundedCache<K, V> {
    values: HashMap<K, V>,
    order: VecDeque<K>,
    limit: usize,
}

impl<K, V> BoundedCache<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    fn new(limit: usize) -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
            limit,
        }
    }

    fn get_or_insert_with(&mut self, key: K, make: impl FnOnce() -> V) -> V {
        if let Some(value) = self.values.get(&key).cloned() {
            return value;
        }
        let value = make();
        self.values.insert(key.clone(), value.clone());
        self.order.push_back(key);
        while self.values.len() > self.limit {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.values.remove(&oldest);
        }
        value
    }
}

struct PassCaches {
    sidecars: BoundedCache<String, Option<AlbumTagSidecar>>,
    artwork: BoundedCache<String, Option<String>>,
    dependencies: BoundedCache<PathBuf, String>,
}

impl Default for PassCaches {
    fn default() -> Self {
        Self {
            sidecars: BoundedCache::new(PASS_CACHE_ENTRIES),
            artwork: BoundedCache::new(PASS_CACHE_ENTRIES),
            dependencies: BoundedCache::new(PASS_CACHE_ENTRIES),
        }
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn fingerprint(path: &Path) -> Result<FileFingerprint, LibraryError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(path)?;
    let nanos =
        i128::from(metadata.mtime()) * 1_000_000_000_i128 + i128::from(metadata.mtime_nsec());
    Ok(FileFingerprint {
        file_id: format!("{}:{}", metadata.dev(), metadata.ino()),
        size_bytes: metadata.len().min(i64::MAX as u64) as i64,
        mtime_ns: nanos.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
    })
}

#[cfg(not(unix))]
fn fingerprint(path: &Path) -> Result<FileFingerprint, LibraryError> {
    let metadata = std::fs::metadata(path)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    Ok(FileFingerprint {
        file_id: String::new(),
        size_bytes: metadata.len().min(i64::MAX as u64) as i64,
        mtime_ns: modified,
    })
}

fn fingerprint_word(value: &FileFingerprint) -> String {
    format!("{}:{}:{}", value.file_id, value.size_bytes, value.mtime_ns)
}

fn is_artwork_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "bmp" | "tif" | "tiff")
    )
}

/// Fingerprint the inexpensive non-audio dependencies that can change a
/// stored row: album sidecars and candidate folder artwork. The result is
/// cached per directory and bounded; it never retains the whole root.
fn directory_dependency(audio_path: &Path, caches: &mut PassCaches) -> String {
    let parent = audio_path.parent().unwrap_or(audio_path).to_path_buf();
    caches.dependencies.get_or_insert_with(parent.clone(), || {
        let album_root = PathBuf::from(MetadataExtractor::album_group_info(audio_path, None).0);
        let mut directories = vec![album_root];
        if directories[0] != parent {
            directories.push(parent);
        }
        let mut parts = Vec::new();
        for directory in directories {
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let sidecar = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == ".qbz.json");
                if !sidecar && !is_artwork_file(&path) {
                    continue;
                }
                if let Ok(value) = fingerprint(&path) {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    parts.push(format!("{name}:{}", fingerprint_word(&value)));
                }
            }
        }
        parts.sort();
        parts.join("|")
    })
}

fn apply_sidecar_override(
    track: &mut LocalTrack,
    cache: &mut BoundedCache<String, Option<AlbumTagSidecar>>,
) {
    let group_key = track.album_group_key.trim();
    if group_key.is_empty() {
        return;
    }
    let album_dir = PathBuf::from(group_key);
    let mut directories = vec![album_dir.clone()];
    if let Some(own_dir) = Path::new(&track.file_path).parent() {
        if own_dir != album_dir {
            directories.push(own_dir.to_path_buf());
        }
    }
    for directory in directories {
        let key = directory.to_string_lossy().into_owned();
        let sidecar = cache.get_or_insert_with(key, || {
            crate::tag_sidecar::read_album_sidecar(&directory).unwrap_or(None)
        });
        if let Some(sidecar) = sidecar.as_ref() {
            crate::tag_sidecar::apply_sidecar_to_track(track, sidecar);
            return;
        }
    }
}

fn decorate_tracks(tracks: &mut [LocalTrack], artwork_cache: &Path, caches: &mut PassCaches) {
    for track in tracks {
        apply_sidecar_override(track, &mut caches.sidecars);
        let audio_path = PathBuf::from(&track.file_path);
        let own_dir = audio_path.parent().unwrap_or(audio_path.as_path());
        let collection_key = if track.album_group_key.trim().is_empty() {
            own_dir.to_string_lossy().into_owned()
        } else {
            track.album_group_key.clone()
        };
        let album_hint = if track.album_group_title.trim().is_empty() {
            track.album.as_str()
        } else {
            track.album_group_title.as_str()
        };

        // Artwork is track/disc metadata, not merely an album-card property.
        // Resolve from the narrowest scope outwards so a box set cannot stamp
        // Disc 01's cover onto every queue row and media-control surface:
        // embedded tag -> this disc's folder -> collection root -> none.
        let embedded_key = format!("embedded:{}", audio_path.to_string_lossy());
        let embedded = caches.artwork.get_or_insert_with(embedded_key, || {
            MetadataExtractor::extract_artwork(&audio_path, artwork_cache)
        });
        let disc_key = format!("disc-folder:{}", own_dir.to_string_lossy());
        let disc_file = caches.artwork.get_or_insert_with(disc_key, || {
            MetadataExtractor::folder_artwork_in_dir(own_dir).and_then(|path| {
                MetadataExtractor::cache_artwork_file(Path::new(&path), artwork_cache)
            })
        });
        let collection_art_key = format!("collection:{collection_key}");
        let collection = caches
            .artwork
            .get_or_insert_with(collection_art_key, || {
                MetadataExtractor::find_folder_artwork(&audio_path, Some(album_hint)).and_then(
                    |folder_art| {
                        MetadataExtractor::cache_artwork_file(Path::new(&folder_art), artwork_cache)
                    },
                )
            });
        let artwork = embedded.or(disc_file).or(collection);
        track.artwork_path = artwork;
    }
}

fn scan_file_matches(
    db: &LibraryDatabase,
    root_id: i64,
    path: &str,
    kind: ScanFileKind,
    value: &FileFingerprint,
    dependency: &str,
) -> Result<bool, LibraryError> {
    db.with_connection(|connection| {
        connection
            .query_row(
                "SELECT 1 FROM local_scan_files
                  WHERE root_id=?1 AND file_path=?2 AND file_kind=?3
                    AND file_id=?4 AND size_bytes=?5 AND mtime_ns=?6
                    AND dependency_fingerprint=?7 AND extraction_ok=1",
                params![
                    root_id,
                    path,
                    kind_word(kind),
                    value.file_id,
                    value.size_bytes,
                    value.mtime_ns,
                    dependency,
                ],
                |_| Ok(()),
            )
            .optional()
            .map(|value| value.is_some())
            .map_err(|error| LibraryError::Database(error.to_string()))
    })
}

fn known_cue_audio_path(
    db: &LibraryDatabase,
    root_id: i64,
    cue_path: &str,
) -> Result<Option<String>, LibraryError> {
    db.with_connection(|connection| {
        connection
            .query_row(
                "SELECT cue_audio_path FROM local_scan_files
                  WHERE root_id=?1 AND file_path=?2 AND file_kind='cue'",
                params![root_id, cue_path],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(|error| LibraryError::Database(error.to_string()))
    })
}

fn kind_word(kind: ScanFileKind) -> &'static str {
    match kind {
        ScanFileKind::Audio => "audio",
        ScanFileKind::Cue => "cue",
    }
}

fn cue_audio_is_referenced(
    db: &LibraryDatabase,
    root_id: i64,
    generation: u64,
    path: &str,
) -> Result<bool, LibraryError> {
    db.with_connection(|connection| {
        connection
            .query_row(
                "SELECT 1 FROM local_scan_cue_refs
                  WHERE root_id=?1 AND generation=?2 AND audio_path=?3",
                params![root_id, generation.min(i64::MAX as u64) as i64, path],
                |_| Ok(()),
            )
            .optional()
            .map(|value| value.is_some())
            .map_err(|error| LibraryError::Database(error.to_string()))
    })
}

fn load_root_state(db: &LibraryDatabase, root_id: i64) -> Result<Option<RootState>, LibraryError> {
    db.with_connection(|connection| {
        connection
            .query_row(
                "SELECT generation,phase,checkpoint_path,discovered,processed,status
                   FROM local_scan_roots WHERE root_id=?1",
                params![root_id],
                |row| {
                    let phase: String = row.get(1)?;
                    Ok(RootState {
                        generation: row.get::<_, i64>(0)?.max(0) as u64,
                        phase: if phase == "audio" {
                            ScanPhase::Audio
                        } else {
                            ScanPhase::Cues
                        },
                        checkpoint_path: row.get(2)?,
                        discovered: row.get::<_, i64>(3)?.max(0) as u64,
                        processed: row.get::<_, i64>(4)?.max(0) as u64,
                        status: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(|error| LibraryError::Database(error.to_string()))
    })
}

fn prepare_root(
    db: &LibraryDatabase,
    folder: &LibraryFolder,
) -> Result<(RootState, bool), LibraryError> {
    let previous = load_root_state(db, folder.id)?;
    let resumable = previous.as_ref().is_some_and(|state| {
        matches!(
            state.status.as_str(),
            "running" | "cancelled" | "unavailable"
        ) && (state.checkpoint_path.is_empty() || Path::new(&state.checkpoint_path).exists())
    });
    if resumable {
        let mut state = previous.expect("checked above");
        state.status = "running".to_string();
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE local_scan_roots
                    SET status='running',prune_authorized=0,updated_at=?2
                  WHERE root_id=?1",
                params![folder.id, now_secs()],
            )
        })
        .map_err(|error| LibraryError::Database(error.to_string()))?;
        return Ok((state, true));
    }

    let generation = previous
        .map(|state| state.generation)
        .unwrap_or(0)
        .wrapping_add(1)
        .max(1);
    db.with_connection(|connection| {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        transaction
            .execute(
                "INSERT INTO local_scan_roots(
                     root_id,generation,phase,checkpoint_path,discovered,processed,
                     status,prune_authorized,updated_at
                 ) VALUES (?1,?2,'cues','',0,0,'running',0,?3)
                 ON CONFLICT(root_id) DO UPDATE SET
                     generation=excluded.generation,phase='cues',checkpoint_path='',
                     discovered=0,processed=0,status='running',prune_authorized=0,
                     updated_at=excluded.updated_at",
                params![
                    folder.id,
                    generation.min(i64::MAX as u64) as i64,
                    now_secs()
                ],
            )
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        transaction
            .execute(
                "DELETE FROM local_scan_cue_refs WHERE root_id=?1",
                params![folder.id],
            )
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        transaction
            .commit()
            .map_err(|error| LibraryError::Database(error.to_string()))
    })?;
    Ok((
        RootState {
            generation,
            phase: ScanPhase::Cues,
            checkpoint_path: String::new(),
            discovered: 0,
            processed: 0,
            status: "running".to_string(),
        },
        false,
    ))
}

fn mark_root_status(db: &LibraryDatabase, root_id: i64, status: &str) -> Result<(), LibraryError> {
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE local_scan_roots
                SET status=?2,prune_authorized=0,updated_at=?3 WHERE root_id=?1",
            params![root_id, status, now_secs()],
        )
    })
    .map(|_| ())
    .map_err(|error| LibraryError::Database(error.to_string()))
}

fn transition_to_audio(
    db: &LibraryDatabase,
    state: &mut RootState,
    root_id: i64,
) -> Result<(), LibraryError> {
    state.phase = ScanPhase::Audio;
    state.checkpoint_path.clear();
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE local_scan_roots
                SET phase='audio',checkpoint_path='',status='running',updated_at=?2
              WHERE root_id=?1",
            params![root_id, now_secs()],
        )
    })
    .map(|_| ())
    .map_err(|error| LibraryError::Database(error.to_string()))
}

fn resolve_batch(batch: Vec<PreparedFile>, backend: &dyn ExtractionBackend) -> Vec<ResolvedFile> {
    let mut tasks = VecDeque::new();
    let mut passthrough = HashMap::new();
    for (index, prepared) in batch.into_iter().enumerate() {
        match prepared.action {
            PreparedAction::Extract(ref task) => tasks.push_back((index, task.clone(), prepared)),
            _ => {
                passthrough.insert(
                    index,
                    ResolvedFile {
                        prepared,
                        tracks: None,
                    },
                );
            }
        }
    }
    let work = Arc::new(Mutex::new(tasks));
    let results = Arc::new(Mutex::new(Vec::new()));
    let workers = work.lock().map(|queue| queue.len()).unwrap_or(0).min(
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2)
            .clamp(1, MAX_EXTRACT_WORKERS),
    );
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let work = Arc::clone(&work);
            let results = Arc::clone(&results);
            scope.spawn(move || loop {
                let next = work.lock().ok().and_then(|mut queue| queue.pop_front());
                let Some((index, task, prepared)) = next else {
                    break;
                };
                let tracks = backend.extract(&task).and_then(|tracks| {
                    if tracks.is_empty() {
                        Err("metadata extraction produced no tracks".to_string())
                    } else {
                        Ok(tracks)
                    }
                });
                if let Ok(mut output) = results.lock() {
                    output.push((
                        index,
                        ResolvedFile {
                            prepared,
                            tracks: Some(tracks),
                        },
                    ));
                }
            });
        }
    });
    let extracted = Arc::try_unwrap(results)
        .ok()
        .and_then(|mutex| mutex.into_inner().ok())
        .unwrap_or_default();
    for (index, result) in extracted {
        passthrough.insert(index, result);
    }
    let mut ordered = passthrough.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(index, _)| *index);
    ordered.into_iter().map(|(_, result)| result).collect()
}

fn remove_obsolete_cue_tracks(
    db: &LibraryDatabase,
    cue_path: &str,
    current: &[LocalTrack],
) -> Result<(), LibraryError> {
    let keep = current
        .iter()
        .map(|track| {
            (
                track.file_path.clone(),
                track.cue_start_secs.map(f64::to_bits),
            )
        })
        .collect::<HashSet<_>>();
    let stale = db.with_connection(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT id,file_path,cue_start_secs
                   FROM local_tracks WHERE cue_file_path=?1",
            )
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        let rows = statement
            .query_map(params![cue_path], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<f64>>(2)?.map(f64::to_bits),
                ))
            })
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        let mut ids = Vec::new();
        for row in rows {
            let (id, file_path, start) =
                row.map_err(|error| LibraryError::Database(error.to_string()))?;
            if !keep.contains(&(file_path, start)) {
                ids.push(id);
            }
        }
        Ok::<_, LibraryError>(ids)
    })?;
    for ids in stale.chunks(500) {
        db.delete_tracks_by_ids(ids)?;
    }
    Ok(())
}

fn commit_batch(
    db: &LibraryDatabase,
    folder: &LibraryFolder,
    state: &RootState,
    batch: &mut [ResolvedFile],
    artwork_cache: &Path,
    caches: &mut PassCaches,
    metrics: &mut RootMetrics,
    errors: &mut Vec<ScanError>,
) -> Result<(), LibraryError> {
    for file in batch.iter_mut() {
        if let Some(Ok(tracks)) = file.tracks.as_mut() {
            decorate_tracks(tracks, artwork_cache, caches);
        }
    }

    db.with_connection(|connection| connection.execute_batch("BEGIN IMMEDIATE;"))
        .map_err(|error| LibraryError::Database(error.to_string()))?;
    let result = (|| {
        for file in batch.iter() {
            match &file.prepared.action {
                PreparedAction::Reuse => {
                    metrics.reused = metrics.reused.saturating_add(1);
                }
                PreparedAction::Extract(_) => match file.tracks.as_ref() {
                    Some(Ok(tracks)) => {
                        for track in tracks {
                            db.insert_scanned_track(track, folder.is_network)?;
                        }
                        if file.prepared.kind == ScanFileKind::Cue {
                            remove_obsolete_cue_tracks(db, &file.prepared.canonical_path, tracks)?;
                        }
                        metrics.extracted = metrics.extracted.saturating_add(1);
                    }
                    Some(Err(error)) => errors.push(ScanError {
                        file_path: file.prepared.canonical_path.clone(),
                        error: error.clone(),
                    }),
                    None => {}
                },
                PreparedAction::Preserve(error) => errors.push(ScanError {
                    file_path: file.prepared.canonical_path.clone(),
                    error: error.clone(),
                }),
                PreparedAction::IgnoreMultiFileCue => {
                    remove_obsolete_cue_tracks(db, &file.prepared.canonical_path, &[])?;
                    metrics.extracted = metrics.extracted.saturating_add(1);
                }
                PreparedAction::SkipCueAudio => {}
            }

            let successful = matches!(file.prepared.action, PreparedAction::Reuse)
                || matches!(file.prepared.action, PreparedAction::IgnoreMultiFileCue)
                || matches!(file.tracks.as_ref(), Some(Ok(_)));
            if let Some(value) = file.prepared.fingerprint.as_ref() {
                // A present but unreadable file receives a negative scan row.
                // That row protects pre-G data from prune while extraction_ok
                // forces another metadata attempt on the next generation.
                let stored_cue_audio_path = if successful {
                    file.prepared.current_cue_audio_path.as_ref()
                } else {
                    file.prepared
                        .previous_cue_audio_path
                        .as_ref()
                        .or(file.prepared.current_cue_audio_path.as_ref())
                };
                db.with_connection(|connection| {
                    connection.execute(
                        "INSERT INTO local_scan_files(
                             root_id,file_path,file_kind,file_id,size_bytes,mtime_ns,
                             dependency_fingerprint,cue_audio_path,extraction_ok,
                             observed_generation
                         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                         ON CONFLICT(root_id,file_path,file_kind) DO UPDATE SET
                             file_id=excluded.file_id,size_bytes=excluded.size_bytes,
                             mtime_ns=excluded.mtime_ns,
                             dependency_fingerprint=excluded.dependency_fingerprint,
                             cue_audio_path=excluded.cue_audio_path,
                             extraction_ok=excluded.extraction_ok,
                             observed_generation=excluded.observed_generation",
                        params![
                            folder.id,
                            file.prepared.canonical_path,
                            kind_word(file.prepared.kind),
                            value.file_id,
                            value.size_bytes,
                            value.mtime_ns,
                            file.prepared.dependency,
                            stored_cue_audio_path,
                            successful as i64,
                            state.generation.min(i64::MAX as u64) as i64,
                        ],
                    )
                })
                .map_err(|error| LibraryError::Database(error.to_string()))?;
            }
            let mut cue_references = Vec::new();
            if let Some(audio_path) = file.prepared.current_cue_audio_path.as_ref() {
                cue_references.push(audio_path);
            }
            if !successful {
                if let Some(audio_path) = file.prepared.previous_cue_audio_path.as_ref() {
                    if !cue_references.contains(&audio_path) {
                        cue_references.push(audio_path);
                    }
                }
            }
            for audio_path in cue_references {
                db.with_connection(|connection| {
                    connection.execute(
                        "INSERT OR IGNORE INTO local_scan_cue_refs(root_id,generation,audio_path)
                         VALUES (?1,?2,?3)",
                        params![
                            folder.id,
                            state.generation.min(i64::MAX as u64) as i64,
                            audio_path,
                        ],
                    )
                })
                .map_err(|error| LibraryError::Database(error.to_string()))?;
            }
        }
        let checkpoint = batch
            .last()
            .map(|file| file.prepared.traversal_path.as_str())
            .unwrap_or(state.checkpoint_path.as_str());
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE local_scan_roots
                    SET checkpoint_path=?2,discovered=?3,processed=?4,
                        status='running',prune_authorized=0,updated_at=?5
                  WHERE root_id=?1",
                params![
                    folder.id,
                    checkpoint,
                    state.discovered.min(i64::MAX as u64) as i64,
                    state.processed.min(i64::MAX as u64) as i64,
                    now_secs(),
                ],
            )
        })
        .map_err(|error| LibraryError::Database(error.to_string()))?;
        Ok::<(), LibraryError>(())
    })();
    match result {
        Ok(()) => db
            .with_connection(|connection| connection.execute_batch("COMMIT;"))
            .map_err(|error| LibraryError::Database(error.to_string())),
        Err(error) => {
            let _ = db.with_connection(|connection| connection.execute_batch("ROLLBACK;"));
            Err(error)
        }
    }
}

fn prepare_file(
    db: &LibraryDatabase,
    folder: &LibraryFolder,
    state: &RootState,
    phase: ScanPhase,
    traversal_path: PathBuf,
    caches: &mut PassCaches,
) -> Result<PreparedFile, LibraryError> {
    let canonical = std::fs::canonicalize(&traversal_path)?;
    let canonical_path = canonical.to_string_lossy().into_owned();
    if phase == ScanPhase::Audio
        && cue_audio_is_referenced(db, folder.id, state.generation, &canonical_path)?
    {
        return Ok(PreparedFile {
            traversal_path: traversal_path.to_string_lossy().into_owned(),
            canonical_path,
            kind: ScanFileKind::Audio,
            fingerprint: None,
            dependency: String::new(),
            current_cue_audio_path: None,
            previous_cue_audio_path: None,
            action: PreparedAction::SkipCueAudio,
        });
    }

    let value = fingerprint(&canonical)?;
    match phase {
        ScanPhase::Audio => {
            let dependency = directory_dependency(&canonical, caches);
            let action = if scan_file_matches(
                db,
                folder.id,
                &canonical_path,
                ScanFileKind::Audio,
                &value,
                &dependency,
            )? {
                PreparedAction::Reuse
            } else {
                PreparedAction::Extract(ExtractTask::Audio {
                    path: canonical,
                    root: normalize_path(Path::new(&folder.path)),
                })
            };
            Ok(PreparedFile {
                traversal_path: traversal_path.to_string_lossy().into_owned(),
                canonical_path,
                kind: ScanFileKind::Audio,
                fingerprint: Some(value),
                dependency,
                current_cue_audio_path: None,
                previous_cue_audio_path: None,
                action,
            })
        }
        ScanPhase::Cues => {
            let previous_cue_audio_path = known_cue_audio_path(db, folder.id, &canonical_path)?;
            let parsed = CueParser::parse(&canonical);
            let (action, dependency, current_cue_audio_path) = match parsed {
                Ok(mut cue) => {
                    cue.file_path = canonical_path.clone();
                    if !cue.is_single_file_image() {
                        // Multi-file sheets are useful metadata sidecars but
                        // the referenced split audio files are the playable,
                        // independently tagged sources. Marking this as a
                        // successful observation also clears any virtual CUE
                        // rows left by the old parser while allowing the audio
                        // phase to index every real file normally.
                        let dependency =
                            format!("multi-file-sidecar-v1|files={}", cue.audio_file_count());
                        let action = if previous_cue_audio_path.is_none()
                            && scan_file_matches(
                                db,
                                folder.id,
                                &canonical_path,
                                ScanFileKind::Cue,
                                &value,
                                &dependency,
                            )? {
                            PreparedAction::Reuse
                        } else {
                            PreparedAction::IgnoreMultiFileCue
                        };
                        return Ok(PreparedFile {
                            traversal_path: traversal_path.to_string_lossy().into_owned(),
                            canonical_path,
                            kind: ScanFileKind::Cue,
                            fingerprint: Some(value),
                            dependency,
                            current_cue_audio_path: None,
                            previous_cue_audio_path,
                            action,
                        });
                    }
                    let audio = normalize_path(Path::new(&cue.audio_file));
                    cue.audio_file = audio.to_string_lossy().into_owned();
                    let audio_path = cue.audio_file.clone();
                    match fingerprint(&audio) {
                        Ok(audio_fingerprint) => {
                            let dependency = format!(
                                "{}|{}",
                                fingerprint_word(&audio_fingerprint),
                                directory_dependency(&audio, caches)
                            );
                            let action = if scan_file_matches(
                                db,
                                folder.id,
                                &canonical_path,
                                ScanFileKind::Cue,
                                &value,
                                &dependency,
                            )? {
                                PreparedAction::Reuse
                            } else {
                                PreparedAction::Extract(ExtractTask::Cue {
                                    cue,
                                    audio_path: audio,
                                })
                            };
                            (action, dependency, Some(audio_path))
                        }
                        Err(error) => (
                            PreparedAction::Preserve(error.to_string()),
                            String::new(),
                            Some(audio_path),
                        ),
                    }
                }
                Err(error) => (
                    PreparedAction::Preserve(error.to_string()),
                    String::new(),
                    None,
                ),
            };
            Ok(PreparedFile {
                traversal_path: traversal_path.to_string_lossy().into_owned(),
                canonical_path,
                kind: ScanFileKind::Cue,
                fingerprint: Some(value),
                dependency,
                current_cue_audio_path,
                previous_cue_audio_path,
                action,
            })
        }
    }
}

enum PhaseResult {
    Complete,
    Cancelled,
    TraversalIncomplete,
}

#[allow(clippy::too_many_arguments)]
fn run_phase(
    db: &LibraryDatabase,
    folder: &LibraryFolder,
    state: &mut RootState,
    phase: ScanPhase,
    progress_base: u64,
    artwork_cache: &Path,
    cancel: &AtomicBool,
    on_event: &(dyn Fn(ScanEvent) + Send + Sync),
    backend: &dyn ExtractionBackend,
    caches: &mut PassCaches,
    metrics: &mut RootMetrics,
    errors: &mut Vec<ScanError>,
) -> Result<PhaseResult, LibraryError> {
    if cancel.load(Ordering::Acquire) {
        return Ok(PhaseResult::Cancelled);
    }
    let scanner = LibraryScanner::new();
    let stream = match scanner.stream_directory(Path::new(&folder.path)) {
        Ok(stream) => stream,
        Err(error) => {
            errors.push(ScanError {
                file_path: folder.path.clone(),
                error: error.to_string(),
            });
            return Ok(PhaseResult::TraversalIncomplete);
        }
    };
    let mut resume_pending = !state.checkpoint_path.is_empty();
    let mut traversal_incomplete = false;
    let mut batch = Vec::with_capacity(SCAN_BATCH_FILES);

    for entry in stream {
        if cancel.load(Ordering::Acquire) {
            if !batch.is_empty() {
                let mut resolved = resolve_batch(std::mem::take(&mut batch), backend);
                let processed_before = state.processed;
                state.processed = state.processed.saturating_add(resolved.len() as u64);
                commit_batch(
                    db,
                    folder,
                    state,
                    &mut resolved,
                    artwork_cache,
                    caches,
                    metrics,
                    errors,
                )?;
                emit_file_done(
                    on_event,
                    progress_base,
                    processed_before,
                    resolved.len(),
                    state.discovered,
                );
            }
            return Ok(PhaseResult::Cancelled);
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                traversal_incomplete = true;
                errors.push(ScanError {
                    file_path: error
                        .path
                        .unwrap_or(error.subtree)
                        .to_string_lossy()
                        .into_owned(),
                    error: error.message,
                });
                continue;
            }
        };
        if entry.kind != phase.kind() {
            continue;
        }
        let traversal_word = entry.path.to_string_lossy().into_owned();
        if resume_pending {
            if traversal_word == state.checkpoint_path {
                resume_pending = false;
            }
            continue;
        }
        on_event(ScanEvent::FileStarted {
            path: traversal_word.clone(),
        });
        state.discovered = state.discovered.saturating_add(1);
        on_event(ScanEvent::TotalsAdded {
            total: progress_base
                .saturating_add(state.discovered)
                .min(u32::MAX as u64) as u32,
        });
        match prepare_file(db, folder, state, phase, entry.path.clone(), caches) {
            Ok(prepared) => batch.push(prepared),
            Err(error) => {
                traversal_incomplete = true;
                errors.push(ScanError {
                    file_path: traversal_word,
                    error: error.to_string(),
                });
            }
        }
        if batch.len() < SCAN_BATCH_FILES {
            continue;
        }
        let mut resolved = resolve_batch(std::mem::take(&mut batch), backend);
        let processed_before = state.processed;
        state.processed = state.processed.saturating_add(resolved.len() as u64);
        commit_batch(
            db,
            folder,
            state,
            &mut resolved,
            artwork_cache,
            caches,
            metrics,
            errors,
        )?;
        emit_file_done(
            on_event,
            progress_base,
            processed_before,
            resolved.len(),
            state.discovered,
        );
        state.checkpoint_path = resolved
            .last()
            .map(|file| file.prepared.traversal_path.clone())
            .unwrap_or_default();
        if cancel.load(Ordering::Acquire) {
            return Ok(PhaseResult::Cancelled);
        }
    }
    if resume_pending {
        traversal_incomplete = true;
        errors.push(ScanError {
            file_path: state.checkpoint_path.clone(),
            error: "scan checkpoint disappeared before resume".to_string(),
        });
    }
    if !batch.is_empty() {
        let mut resolved = resolve_batch(batch, backend);
        let processed_before = state.processed;
        state.processed = state.processed.saturating_add(resolved.len() as u64);
        commit_batch(
            db,
            folder,
            state,
            &mut resolved,
            artwork_cache,
            caches,
            metrics,
            errors,
        )?;
        emit_file_done(
            on_event,
            progress_base,
            processed_before,
            resolved.len(),
            state.discovered,
        );
        state.checkpoint_path = resolved
            .last()
            .map(|file| file.prepared.traversal_path.clone())
            .unwrap_or_default();
        if cancel.load(Ordering::Acquire) {
            return Ok(PhaseResult::Cancelled);
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Ok(PhaseResult::Cancelled);
    }
    Ok(if traversal_incomplete {
        PhaseResult::TraversalIncomplete
    } else {
        PhaseResult::Complete
    })
}

fn emit_file_done(
    on_event: &(dyn Fn(ScanEvent) + Send + Sync),
    progress_base: u64,
    processed_before: u64,
    batch_len: usize,
    discovered: u64,
) {
    let total = progress_base
        .saturating_add(discovered)
        .min(u32::MAX as u64) as u32;
    for completed in 1..=batch_len as u64 {
        on_event(ScanEvent::FileDone {
            processed: progress_base
                .saturating_add(processed_before)
                .saturating_add(completed)
                .min(u32::MAX as u64) as u32,
            total,
        });
    }
}

fn finish_root(
    db: &LibraryDatabase,
    folder: &LibraryFolder,
    state: &RootState,
) -> Result<u64, LibraryError> {
    let generation = state.generation.min(i64::MAX as u64) as i64;
    let root_prefix = format!("{}/", folder.path.trim_end_matches('/'));
    db.with_connection(|connection| {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        let removed = transaction
            .execute(
                "DELETE FROM local_tracks AS lt
                  WHERE (lt.source IS NULL OR lt.source='user')
                    AND (
                      EXISTS (
                        SELECT 1 FROM local_scan_files stale
                         WHERE stale.root_id=?1 AND stale.observed_generation<>?2
                           AND ((stale.file_kind='audio' AND lt.cue_file_path IS NULL
                                 AND lt.file_path=stale.file_path)
                             OR (stale.file_kind='cue' AND lt.cue_file_path=stale.file_path))
                           AND NOT EXISTS (
                               SELECT 1 FROM local_scan_files keep
                                WHERE keep.root_id<>?1
                                  AND keep.file_path=stale.file_path
                                  AND keep.file_kind=stale.file_kind
                           )
                      )
                      OR (lt.cue_file_path IS NULL
                          AND substr(lt.file_path,1,length(?3))=?3
                          AND NOT EXISTS (
                              SELECT 1 FROM local_scan_files seen
                               WHERE seen.file_kind='audio' AND seen.file_path=lt.file_path
                          ))
                      OR (lt.cue_file_path IS NOT NULL
                          AND substr(lt.cue_file_path,1,length(?3))=?3
                          AND NOT EXISTS (
                              SELECT 1 FROM local_scan_files seen
                               WHERE seen.file_kind='cue' AND seen.file_path=lt.cue_file_path
                          ))
                    )",
                params![folder.id, generation, root_prefix],
            )
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        transaction
            .execute(
                "DELETE FROM local_scan_files
                  WHERE root_id=?1 AND observed_generation<>?2",
                params![folder.id, generation],
            )
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        transaction
            .execute(
                "DELETE FROM local_scan_cue_refs
                  WHERE root_id=?1 AND generation<>?2",
                params![folder.id, generation],
            )
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        transaction
            .execute(
                "UPDATE local_scan_roots
                    SET phase='idle',checkpoint_path='',status='complete',
                        prune_authorized=1,updated_at=?2
                  WHERE root_id=?1",
                params![folder.id, now_secs()],
            )
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        transaction
            .execute(
                "UPDATE library_folders SET last_scan=?2 WHERE id=?1",
                params![folder.id, now_secs()],
            )
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        transaction
            .commit()
            .map_err(|error| LibraryError::Database(error.to_string()))?;
        Ok(removed as u64)
    })
}

enum RootResult {
    Complete(u64),
    Cancelled,
    Incomplete(u64),
}

fn report_root_without_prune(
    folder: &LibraryFolder,
    state: &RootState,
    metrics: &RootMetrics,
    status: &str,
    started: Instant,
    on_event: &(dyn Fn(ScanEvent) + Send + Sync),
) {
    log::warn!(
        "[local-scan] root_id={} generation={} network={} status={} discovered={} extracted={} reused={} pruned=0 prune_authorized=false elapsed={:?}",
        folder.id,
        state.generation,
        folder.is_network,
        status,
        state.discovered,
        metrics.extracted,
        metrics.reused,
        started.elapsed(),
    );
    on_event(ScanEvent::RootFinished {
        root_id: folder.id,
        generation: state.generation,
        discovered: state.discovered,
        extracted: metrics.extracted,
        reused: metrics.reused,
        pruned: 0,
        prune_authorized: false,
        elapsed: started.elapsed(),
    });
}

#[allow(clippy::too_many_arguments)]
fn scan_root(
    db: &LibraryDatabase,
    folder: &mut LibraryFolder,
    progress_base: u64,
    artwork_cache: &Path,
    cancel: &AtomicBool,
    on_event: &(dyn Fn(ScanEvent) + Send + Sync),
    backend: &dyn ExtractionBackend,
    errors: &mut Vec<ScanError>,
) -> Result<RootResult, LibraryError> {
    let started = Instant::now();
    if crate::reachability::probe_default(Path::new(&folder.path))
        != crate::reachability::Reach::Present
    {
        let previous = load_root_state(db, folder.id)?;
        if previous.as_ref().is_some_and(|state| {
            matches!(
                state.status.as_str(),
                "running" | "cancelled" | "unavailable"
            )
        }) {
            // Preserve a resumable in-flight generation. A previously
            // completed generation stays completed; reusing its generation
            // after remount would make deleted files look observed forever.
            mark_root_status(db, folder.id, "unavailable")?;
        }
        errors.push(ScanError {
            file_path: folder.path.clone(),
            error: "root unavailable; previous rows preserved".to_string(),
        });
        log::warn!(
            "[local-scan] root_id={} generation={} status=unavailable discovered=0 extracted=0 reused=0 pruned=0 prune_authorized=false elapsed={:?}",
            folder.id,
            previous.as_ref().map(|state| state.generation).unwrap_or(0),
            started.elapsed(),
        );
        on_event(ScanEvent::RootFinished {
            root_id: folder.id,
            generation: previous.map(|state| state.generation).unwrap_or(0),
            discovered: 0,
            extracted: 0,
            reused: 0,
            pruned: 0,
            prune_authorized: false,
            elapsed: started.elapsed(),
        });
        return Ok(RootResult::Incomplete(0));
    }

    if !folder.user_override_network {
        let is_network = crate::mount_info::is_network_path(Path::new(&folder.path));
        if is_network != folder.is_network {
            let label = is_network
                .then(|| crate::mount_info::network_fs_label(Path::new(&folder.path)))
                .flatten();
            db.update_folder_settings(
                folder.id,
                folder.alias.as_deref(),
                folder.enabled,
                is_network,
                label.as_deref(),
                false,
            )?;
            folder.is_network = is_network;
        }
    }

    let (mut state, resumed) = prepare_root(db, folder)?;
    on_event(ScanEvent::RootStarted {
        root_id: folder.id,
        generation: state.generation,
        resumed,
        is_network: folder.is_network,
    });
    if state.discovered > 0 {
        on_event(ScanEvent::TotalsAdded {
            total: progress_base
                .saturating_add(state.discovered)
                .min(u32::MAX as u64) as u32,
        });
        if state.processed > 0 {
            on_event(ScanEvent::FileDone {
                processed: progress_base
                    .saturating_add(state.processed)
                    .min(u32::MAX as u64) as u32,
                total: progress_base
                    .saturating_add(state.discovered)
                    .min(u32::MAX as u64) as u32,
            });
        }
    }
    let mut caches = PassCaches::default();
    let mut metrics = RootMetrics::default();

    if state.phase == ScanPhase::Cues {
        match run_phase(
            db,
            folder,
            &mut state,
            ScanPhase::Cues,
            progress_base,
            artwork_cache,
            cancel,
            on_event,
            backend,
            &mut caches,
            &mut metrics,
            errors,
        )? {
            PhaseResult::Complete => transition_to_audio(db, &mut state, folder.id)?,
            PhaseResult::Cancelled => {
                mark_root_status(db, folder.id, "cancelled")?;
                report_root_without_prune(folder, &state, &metrics, "cancelled", started, on_event);
                return Ok(RootResult::Cancelled);
            }
            PhaseResult::TraversalIncomplete => {
                mark_root_status(db, folder.id, "error")?;
                report_root_without_prune(
                    folder,
                    &state,
                    &metrics,
                    "traversal-incomplete",
                    started,
                    on_event,
                );
                return Ok(RootResult::Incomplete(state.discovered));
            }
        }
    }

    match run_phase(
        db,
        folder,
        &mut state,
        ScanPhase::Audio,
        progress_base,
        artwork_cache,
        cancel,
        on_event,
        backend,
        &mut caches,
        &mut metrics,
        errors,
    )? {
        PhaseResult::Complete => {}
        PhaseResult::Cancelled => {
            mark_root_status(db, folder.id, "cancelled")?;
            report_root_without_prune(folder, &state, &metrics, "cancelled", started, on_event);
            return Ok(RootResult::Cancelled);
        }
        PhaseResult::TraversalIncomplete => {
            mark_root_status(db, folder.id, "error")?;
            report_root_without_prune(
                folder,
                &state,
                &metrics,
                "traversal-incomplete",
                started,
                on_event,
            );
            return Ok(RootResult::Incomplete(state.discovered));
        }
    }

    if cancel.load(Ordering::Acquire) {
        mark_root_status(db, folder.id, "cancelled")?;
        report_root_without_prune(folder, &state, &metrics, "cancelled", started, on_event);
        return Ok(RootResult::Cancelled);
    }
    on_event(ScanEvent::Cleanup);
    metrics.pruned = finish_root(db, folder, &state)?;

    // SACD images live outside the cue/audio phases: the walk keys on the
    // extension, the `SACDMTOC` sniff keeps DVD/data ISOs silent, and the
    // generation-based import owns the `sacd:` rows (sacd_scan.rs). A parser
    // or import rejection is a scan error like any unreadable file.
    let sacd = crate::sacd_scan::scan_root_for_sacd(
        db,
        folder.id,
        state.generation.min(i64::MAX as u64) as i64,
        Path::new(&folder.path),
        &crate::sacd_scan::SacdLabels::default(),
        cancel,
    );
    if sacd.candidates > 0 || sacd.removed > 0 || !sacd.failed.is_empty() {
        log::info!(
            "[local-scan] root_id={} sacd candidates={} imported={} unchanged={} ignored={} removed={} prune_authorized={} failed={}",
            folder.id,
            sacd.candidates,
            sacd.imported,
            sacd.unchanged,
            sacd.ignored,
            sacd.removed,
            sacd.prune_authorized,
            sacd.failed.len(),
        );
    }
    for (file_path, error) in sacd.failed {
        errors.push(ScanError { file_path, error });
    }
    log::info!(
        "[local-scan] root_id={} generation={} network={} discovered={} extracted={} reused={} pruned={} prune_authorized=true elapsed={:?}",
        folder.id,
        state.generation,
        folder.is_network,
        state.discovered,
        metrics.extracted,
        metrics.reused,
        metrics.pruned,
        started.elapsed(),
    );
    on_event(ScanEvent::RootFinished {
        root_id: folder.id,
        generation: state.generation,
        discovered: state.discovered,
        extracted: metrics.extracted,
        reused: metrics.reused,
        pruned: metrics.pruned,
        prune_authorized: true,
        elapsed: started.elapsed(),
    });
    Ok(RootResult::Complete(state.discovered))
}

/// Scan every enabled root (`None`) or the requested enabled roots. Existing
/// databases are authoritative and remain readable throughout each bounded
/// batch; the derived catalog catches up after the caller observes Finished.
pub fn scan_with_progress(
    db: &LibraryDatabase,
    folder_ids: Option<&[i64]>,
    artwork_cache: &Path,
    cancel: &AtomicBool,
    on_event: &(dyn Fn(ScanEvent) + Send + Sync),
) -> Result<(), LibraryError> {
    scan_with_backend(
        db,
        folder_ids,
        artwork_cache,
        cancel,
        on_event,
        &RealExtraction,
    )
}

fn scan_with_backend(
    db: &LibraryDatabase,
    folder_ids: Option<&[i64]>,
    artwork_cache: &Path,
    cancel: &AtomicBool,
    on_event: &(dyn Fn(ScanEvent) + Send + Sync),
    backend: &dyn ExtractionBackend,
) -> Result<(), LibraryError> {
    let all = db.get_folders_with_metadata()?;
    let mut targets = match folder_ids {
        None => all
            .into_iter()
            .filter(|folder| folder.enabled)
            .collect::<Vec<_>>(),
        Some(ids) => all
            .into_iter()
            .filter(|folder| folder.enabled && ids.contains(&folder.id))
            .collect::<Vec<_>>(),
    };
    if targets.is_empty() {
        return Err(LibraryError::Other(
            "No library folders to scan".to_string(),
        ));
    }

    on_event(ScanEvent::Started);
    let mut errors = Vec::new();
    let mut progress_base = 0_u64;
    for folder in &mut targets {
        if cancel.load(Ordering::Acquire) {
            on_event(ScanEvent::Finished {
                status: ScanStatus::Cancelled,
                errors,
            });
            return Ok(());
        }
        match scan_root(
            db,
            folder,
            progress_base,
            artwork_cache,
            cancel,
            on_event,
            backend,
            &mut errors,
        )? {
            RootResult::Cancelled => {
                on_event(ScanEvent::Finished {
                    status: ScanStatus::Cancelled,
                    errors,
                });
                return Ok(());
            }
            RootResult::Complete(discovered) | RootResult::Incomplete(discovered) => {
                progress_base = progress_base.saturating_add(discovered);
            }
        }
    }
    on_event(ScanEvent::Finished {
        status: ScanStatus::Complete,
        errors,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    struct FakeExtraction {
        calls: AtomicUsize,
        cancel_after: Option<usize>,
        cancel: Arc<AtomicBool>,
    }

    impl FakeExtraction {
        fn new(cancel: Arc<AtomicBool>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                cancel_after: None,
                cancel,
            }
        }

        fn cancelling(cancel: Arc<AtomicBool>, after: usize) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                cancel_after: Some(after),
                cancel,
            }
        }
    }

    impl ExtractionBackend for FakeExtraction {
        fn extract(&self, task: &ExtractTask) -> Result<Vec<LocalTrack>, String> {
            let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            if self.cancel_after.is_some_and(|after| call >= after) {
                self.cancel.store(true, Ordering::Release);
            }
            let make_track = |path: &Path, title: String, cue: Option<(&CueSheet, f64)>| {
                let mut track = LocalTrack::default();
                track.file_path = path.to_string_lossy().into_owned();
                track.title = title;
                track.artist = "Fixture Artist".to_string();
                track.album = "Fixture Album".to_string();
                track.album_group_key =
                    path.parent().unwrap_or(path).to_string_lossy().into_owned();
                track.album_group_title = "Fixture Album".to_string();
                track.file_size_bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
                track.last_modified = now_secs();
                track.indexed_at = now_secs();
                if let Some((sheet, start)) = cue {
                    track.cue_file_path = Some(sheet.file_path.clone());
                    track.cue_start_secs = Some(start);
                }
                track
            };
            Ok(match task {
                ExtractTask::Audio { path, .. } => vec![make_track(
                    path,
                    path.file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Track".to_string()),
                    None,
                )],
                ExtractTask::Cue { cue, audio_path } => cue
                    .tracks
                    .iter()
                    .map(|track| {
                        make_track(
                            audio_path,
                            track.title.clone(),
                            Some((cue, track.start_secs)),
                        )
                    })
                    .collect(),
            })
        }
    }

    struct AlwaysFail;

    impl ExtractionBackend for AlwaysFail {
        fn extract(&self, _task: &ExtractTask) -> Result<Vec<LocalTrack>, String> {
            Err("fixture extraction failure".to_string())
        }
    }

    fn fixture() -> (tempfile::TempDir, LibraryDatabase, i64, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("music");
        fs::create_dir_all(&root).unwrap();
        let db = LibraryDatabase::open(&temp.path().join("library.db")).unwrap();
        let root_id = db
            .add_folder_with_network_info(&root.to_string_lossy(), false, None)
            .unwrap();
        (temp, db, root_id, root)
    }

    fn run_fake(
        db: &LibraryDatabase,
        root_id: i64,
        temp: &tempfile::TempDir,
        cancel: &Arc<AtomicBool>,
        backend: &FakeExtraction,
    ) {
        scan_with_backend(
            db,
            Some(&[root_id]),
            &temp.path().join("art"),
            cancel,
            &|_| {},
            backend,
        )
        .unwrap();
    }

    fn track_rows(db: &LibraryDatabase) -> Vec<(i64, String)> {
        db.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id,file_path FROM local_tracks ORDER BY file_path")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        })
    }

    fn cue_rows(db: &LibraryDatabase) -> Vec<(i64, String, Option<String>)> {
        db.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id,title,cue_file_path FROM local_tracks ORDER BY cue_start_secs")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        })
    }

    fn write_cue(path: &Path, tracks: &[(&str, &str)]) {
        let mut contents = String::from(
            "PERFORMER \"Fixture Artist\"\nTITLE \"Fixture Album\"\nFILE \"album.flac\" WAVE\n",
        );
        for (number, (title, index)) in tracks.iter().enumerate() {
            contents.push_str(&format!(
                "  TRACK {:02} AUDIO\n    TITLE \"{}\"\n    INDEX 01 {}\n",
                number + 1,
                title,
                index
            ));
        }
        fs::write(path, contents).unwrap();
    }

    fn write_multi_file_cue(path: &Path, tracks: &[(&str, &str)]) {
        let mut contents = String::from("PERFORMER \"Fixture Artist\"\nTITLE \"Fixture Album\"\n");
        for (number, (file, title)) in tracks.iter().enumerate() {
            contents.push_str(&format!(
                "FILE \"{}\" WAVE\n  TRACK {:02} AUDIO\n    TITLE \"{}\"\n    INDEX 01 00:00:00\n",
                file,
                number + 1,
                title
            ));
        }
        fs::write(path, contents).unwrap();
    }

    fn write_artwork(path: &Path, rgb: [u8; 3]) {
        image::RgbImage::from_pixel(2, 2, image::Rgb(rgb))
            .save(path)
            .unwrap();
    }

    #[test]
    fn artwork_resolution_prefers_each_disc_then_collection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Box Set");
        let disc_one = root.join("Disc 01 - First");
        let disc_two = root.join("Disc 02 - Second");
        let disc_three = root.join("Disc 03 - No Own Art");
        for directory in [&disc_one, &disc_two, &disc_three] {
            fs::create_dir_all(directory).unwrap();
            fs::write(directory.join("01.flac"), b"not-a-real-audio-file").unwrap();
        }
        write_artwork(&root.join("cover.png"), [10, 10, 10]);
        write_artwork(&disc_one.join("cover.png"), [200, 10, 10]);
        // A uniquely named image is still authoritative for its own disc.
        write_artwork(&disc_two.join("TV Series Soundtrack 02.png"), [10, 200, 10]);

        let track = |disc: u32, directory: &Path| LocalTrack {
            file_path: directory.join("01.flac").to_string_lossy().into_owned(),
            album: "Box Set".to_string(),
            album_group_key: root.to_string_lossy().into_owned(),
            album_group_title: "Box Set".to_string(),
            disc_number: Some(disc),
            ..Default::default()
        };
        let mut tracks = vec![
            track(1, &disc_one),
            track(2, &disc_two),
            track(3, &disc_three),
        ];
        let mut caches = PassCaches::default();
        decorate_tracks(&mut tracks, temp.path(), &mut caches);

        let one = tracks[0].artwork_path.as_deref().unwrap();
        let two = tracks[1].artwork_path.as_deref().unwrap();
        let three = tracks[2].artwork_path.as_deref().unwrap();
        assert_ne!(one, two, "two distinct disc covers collapsed");
        assert_ne!(one, three, "disc 1 leaked into the collection fallback");
        assert_ne!(two, three, "disc 2 leaked into the collection fallback");
    }

    #[test]
    fn unchanged_files_skip_extraction_and_changed_file_keeps_its_id() {
        let (temp, db, root_id, root) = fixture();
        for index in 0..5 {
            fs::write(root.join(format!("track-{index}.flac")), b"fixture").unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let first = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &first);
        assert_eq!(first.calls.load(AtomicOrdering::SeqCst), 5);
        let before = track_rows(&db);

        let second = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &second);
        assert_eq!(second.calls.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(track_rows(&db), before);

        fs::write(root.join("track-2.flac"), b"fixture changed").unwrap();
        let third = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &third);
        assert_eq!(third.calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(track_rows(&db), before);
    }

    #[test]
    fn extraction_failure_preserves_pre_incremental_row_and_retries() {
        let (temp, db, root_id, root) = fixture();
        let path = root.join("unreadable.flac");
        fs::write(&path, b"not real audio").unwrap();
        let mut existing = LocalTrack::default();
        existing.file_path = path.to_string_lossy().into_owned();
        existing.title = "Previously indexed".to_string();
        existing.artist = "Fixture Artist".to_string();
        existing.album = "Fixture Album".to_string();
        existing.album_group_key = root.to_string_lossy().into_owned();
        existing.album_group_title = existing.album.clone();
        let id = db.insert_track(&existing).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        scan_with_backend(
            &db,
            Some(&[root_id]),
            &temp.path().join("art"),
            &cancel,
            &|_| {},
            &AlwaysFail,
        )
        .unwrap();
        assert_eq!(track_rows(&db), vec![(id, existing.file_path.clone())]);
        let extraction_ok: i64 = db.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT extraction_ok FROM local_scan_files WHERE root_id=?1",
                    params![root_id],
                    |row| row.get(0),
                )
                .unwrap()
        });
        assert_eq!(extraction_ok, 0);

        let retry = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &retry);
        assert_eq!(retry.calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(track_rows(&db)[0].0, id);
    }

    #[test]
    fn cue_changes_remove_obsolete_virtual_tracks_and_parse_failure_never_duplicates_audio() {
        let (temp, db, root_id, root) = fixture();
        fs::write(root.join("album.flac"), b"fixture audio").unwrap();
        let cue_path = root.join("album.cue");
        write_cue(
            &cue_path,
            &[
                ("First movement", "00:00:00"),
                ("Second movement", "03:00:00"),
            ],
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let first = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &first);
        assert_eq!(first.calls.load(AtomicOrdering::SeqCst), 1);
        let initial = cue_rows(&db);
        assert_eq!(initial.len(), 2);
        assert!(initial.iter().all(|(_, _, cue)| cue.is_some()));

        write_cue(&cue_path, &[("First movement revised", "00:00:00")]);
        let changed = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &changed);
        let revised = cue_rows(&db);
        assert_eq!(changed.calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(revised.len(), 1);
        assert_eq!(revised[0].0, initial[0].0);
        assert_eq!(revised[0].1, "First movement revised");

        fs::write(&cue_path, b"TRACK 01 AUDIO\n").unwrap();
        let malformed = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &malformed);
        let preserved = cue_rows(&db);
        assert_eq!(malformed.calls.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(preserved, revised);
        assert!(preserved[0].2.is_some());

        write_cue(&cue_path, &[("First movement repaired", "00:00:00")]);
        let repaired = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &repaired);
        assert_eq!(repaired.calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(cue_rows(&db)[0].0, initial[0].0);

        fs::remove_file(cue_path).unwrap();
        let cue_removed = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &cue_removed);
        let plain = cue_rows(&db);
        assert_eq!(cue_removed.calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(plain.len(), 1);
        assert!(plain[0].2.is_none());
    }

    #[test]
    fn multi_file_cue_retires_virtual_rows_and_indexes_only_split_audio() {
        let (temp, db, root_id, root) = fixture();
        let first_path = root.join("01. First.flac");
        let second_path = root.join("02. Second.flac");
        fs::write(&first_path, b"fixture first audio").unwrap();
        fs::write(&second_path, b"fixture second audio").unwrap();
        let cue_path = root.join("leftover.cue");
        write_multi_file_cue(
            &cue_path,
            &[("01. First.flac", "First"), ("02. Second.flac", "Second")],
        );

        // Simulate rows produced by the legacy parser, which attached every
        // virtual entry to one arbitrary FILE from the sheet.
        for (index, title) in ["Legacy First", "Legacy Second"].iter().enumerate() {
            let mut stale = LocalTrack::default();
            stale.file_path = second_path.to_string_lossy().into_owned();
            stale.title = (*title).to_string();
            stale.artist = "Fixture Artist".to_string();
            stale.album = "Fixture Album".to_string();
            stale.album_group_key = root.to_string_lossy().into_owned();
            stale.album_group_title = stale.album.clone();
            stale.cue_file_path = Some(cue_path.to_string_lossy().into_owned());
            stale.cue_start_secs = Some(index as f64);
            db.insert_track(&stale).unwrap();
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let extraction = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &extraction);

        // Only the two real files require extraction; the valid multi-file
        // CUE is observed as a sidecar and never reaches the CUE backend.
        assert_eq!(extraction.calls.load(AtomicOrdering::SeqCst), 2);
        let rows = cue_rows(&db);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(_, _, cue)| cue.is_none()));
        let paths = track_rows(&db)
            .into_iter()
            .map(|(_, path)| path)
            .collect::<HashSet<_>>();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&first_path.to_string_lossy().into_owned()));
        assert!(paths.contains(&second_path.to_string_lossy().into_owned()));
    }

    #[test]
    fn root_prefix_treats_sql_wildcards_as_plain_path_characters() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("music%root");
        let sibling = temp.path().join("musicXroot");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::write(root.join("inside.flac"), b"inside").unwrap();
        let outside_path = sibling.join("outside.flac");
        fs::write(&outside_path, b"outside").unwrap();
        let db = LibraryDatabase::open(&temp.path().join("library.db")).unwrap();
        let root_id = db
            .add_folder_with_network_info(&root.to_string_lossy(), false, None)
            .unwrap();
        let mut outside = LocalTrack::default();
        outside.file_path = outside_path.to_string_lossy().into_owned();
        outside.title = "Outside".to_string();
        outside.artist = "Fixture Artist".to_string();
        outside.album = "Fixture Album".to_string();
        outside.album_group_key = sibling.to_string_lossy().into_owned();
        outside.album_group_title = outside.album.clone();
        let outside_id = db.insert_track(&outside).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        run_fake(
            &db,
            root_id,
            &temp,
            &cancel,
            &FakeExtraction::new(Arc::clone(&cancel)),
        );
        let rows = track_rows(&db);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|(id, _)| *id == outside_id));
    }

    #[test]
    fn progress_is_monotonic_across_multiple_roots() {
        let (temp, db, _first_id, first) = fixture();
        let second = temp.path().join("second-root");
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("one.flac"), b"one").unwrap();
        fs::write(second.join("two.flac"), b"two").unwrap();
        fs::write(second.join("three.flac"), b"three").unwrap();
        db.add_folder_with_network_info(&second.to_string_lossy(), false, None)
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Mutex::new(Vec::new());
        scan_with_backend(
            &db,
            None,
            &temp.path().join("art"),
            &cancel,
            &|event| {
                if let ScanEvent::FileDone { processed, .. } = event {
                    progress.lock().unwrap().push(processed);
                }
            },
            &FakeExtraction::new(Arc::clone(&cancel)),
        )
        .unwrap();
        let progress = progress.into_inner().unwrap();
        assert_eq!(progress.last(), Some(&3));
        assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn incremental_scan_metric_fixture_reuses_two_thousand_fingerprints() {
        let (temp, db, root_id, root) = fixture();
        const FILES: usize = 2_000;
        for index in 0..FILES {
            fs::write(root.join(format!("track-{index:04}.flac")), b"fixture").unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let first_metrics = Mutex::new(None);
        let first_backend = FakeExtraction::new(Arc::clone(&cancel));
        let first_wall = Instant::now();
        scan_with_backend(
            &db,
            Some(&[root_id]),
            &temp.path().join("art"),
            &cancel,
            &|event| {
                if let ScanEvent::RootFinished {
                    discovered,
                    extracted,
                    reused,
                    elapsed,
                    ..
                } = event
                {
                    *first_metrics.lock().unwrap() = Some((discovered, extracted, reused, elapsed));
                }
            },
            &first_backend,
        )
        .unwrap();
        let first_wall = first_wall.elapsed();
        assert_eq!(first_backend.calls.load(AtomicOrdering::SeqCst), FILES);
        assert_eq!(track_rows(&db).len(), FILES);
        let first_metrics = first_metrics.into_inner().unwrap().unwrap();
        assert_eq!(first_metrics.0, FILES as u64);
        assert_eq!(first_metrics.1, FILES as u64);
        assert_eq!(first_metrics.2, 0);

        let second_metrics = Mutex::new(None);
        let second_backend = FakeExtraction::new(Arc::clone(&cancel));
        let second_wall = Instant::now();
        scan_with_backend(
            &db,
            Some(&[root_id]),
            &temp.path().join("art"),
            &cancel,
            &|event| {
                if let ScanEvent::RootFinished {
                    discovered,
                    extracted,
                    reused,
                    elapsed,
                    ..
                } = event
                {
                    *second_metrics.lock().unwrap() =
                        Some((discovered, extracted, reused, elapsed));
                }
            },
            &second_backend,
        )
        .unwrap();
        let second_wall = second_wall.elapsed();
        assert_eq!(second_backend.calls.load(AtomicOrdering::SeqCst), 0);
        let second_metrics = second_metrics.into_inner().unwrap().unwrap();
        assert_eq!(second_metrics.0, FILES as u64);
        assert_eq!(second_metrics.1, 0);
        assert_eq!(second_metrics.2, FILES as u64);
        println!(
            "G_SCAN_METRIC files={FILES} first_root_ms={} first_wall_ms={} second_root_ms={} second_wall_ms={} first_extracted={} second_reused={}",
            first_metrics.3.as_millis(),
            first_wall.as_millis(),
            second_metrics.3.as_millis(),
            second_wall.as_millis(),
            first_metrics.1,
            second_metrics.2,
        );
    }

    #[test]
    fn completed_root_prunes_but_unavailable_network_root_never_does() {
        let (temp, db, root_id, root) = fixture();
        fs::write(root.join("keep.flac"), b"keep").unwrap();
        fs::write(root.join("remove.flac"), b"remove").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        run_fake(
            &db,
            root_id,
            &temp,
            &cancel,
            &FakeExtraction::new(Arc::clone(&cancel)),
        );
        fs::remove_file(root.join("remove.flac")).unwrap();
        run_fake(
            &db,
            root_id,
            &temp,
            &cancel,
            &FakeExtraction::new(Arc::clone(&cancel)),
        );
        assert_eq!(track_rows(&db).len(), 1);

        db.update_folder_settings(root_id, None, true, true, Some("nfs"), true)
            .unwrap();
        let hidden = temp.path().join("network-offline");
        fs::rename(&root, &hidden).unwrap();
        let attempted_prune = AtomicBool::new(true);
        scan_with_backend(
            &db,
            Some(&[root_id]),
            &temp.path().join("art"),
            &cancel,
            &|event| {
                if let ScanEvent::RootFinished {
                    prune_authorized, ..
                } = event
                {
                    attempted_prune.store(prune_authorized, Ordering::Release);
                }
            },
            &FakeExtraction::new(Arc::clone(&cancel)),
        )
        .unwrap();
        assert_eq!(track_rows(&db).len(), 1);
        assert!(!attempted_prune.load(Ordering::Acquire));
        let prune_authorized: i64 = db.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT prune_authorized FROM local_scan_roots WHERE root_id=?1",
                    params![root_id],
                    |row| row.get(0),
                )
                .unwrap()
        });
        // The last good generation remains a valid snapshot even though the
        // current inaccessible attempt was not allowed to prune it.
        assert_eq!(prune_authorized, 1);
    }

    #[test]
    fn cancelled_generation_resumes_without_duplicates_or_prune() {
        let (temp, db, root_id, root) = fixture();
        for index in 0..230 {
            fs::write(root.join(format!("track-{index:04}.flac")), b"fixture").unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let first = FakeExtraction::cancelling(Arc::clone(&cancel), 2);
        run_fake(&db, root_id, &temp, &cancel, &first);
        let state = load_root_state(&db, root_id).unwrap().unwrap();
        assert_eq!(state.status, "cancelled");
        assert!(!state.checkpoint_path.is_empty());
        assert!(track_rows(&db).len() <= SCAN_BATCH_FILES);

        cancel.store(false, Ordering::Release);
        let second = FakeExtraction::new(Arc::clone(&cancel));
        run_fake(&db, root_id, &temp, &cancel, &second);
        let rows = track_rows(&db);
        assert_eq!(rows.len(), 230);
        assert_eq!(
            rows.iter()
                .map(|(id, _)| *id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            rows.len()
        );
        let complete = load_root_state(&db, root_id).unwrap().unwrap();
        assert_eq!(complete.generation, state.generation);
        assert_eq!(complete.status, "complete");
    }

    #[test]
    fn cancellation_during_the_final_batch_never_authorizes_prune() {
        let (temp, db, root_id, root) = fixture();
        fs::write(root.join("keep.flac"), b"keep").unwrap();
        fs::write(root.join("temporarily-absent.flac"), b"present").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        run_fake(
            &db,
            root_id,
            &temp,
            &cancel,
            &FakeExtraction::new(Arc::clone(&cancel)),
        );
        assert_eq!(track_rows(&db).len(), 2);
        fs::remove_file(root.join("temporarily-absent.flac")).unwrap();
        fs::write(root.join("keep.flac"), b"keep changed").unwrap();

        let prune_authorized = AtomicBool::new(true);
        let cancelling = FakeExtraction::cancelling(Arc::clone(&cancel), 1);
        scan_with_backend(
            &db,
            Some(&[root_id]),
            &temp.path().join("art"),
            &cancel,
            &|event| {
                if let ScanEvent::RootFinished {
                    prune_authorized: allowed,
                    ..
                } = event
                {
                    prune_authorized.store(allowed, Ordering::Release);
                }
            },
            &cancelling,
        )
        .unwrap();
        assert!(!prune_authorized.load(Ordering::Acquire));
        assert_eq!(track_rows(&db).len(), 2);
        assert_eq!(
            load_root_state(&db, root_id).unwrap().unwrap().status,
            "cancelled"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycle_is_visible_and_blocks_prune_until_a_clean_pass() {
        use std::os::unix::fs::symlink;

        let (temp, db, root_id, root) = fixture();
        fs::write(root.join("track.flac"), b"fixture").unwrap();
        symlink(&root, root.join("cycle")).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let events = Mutex::new(Vec::new());
        scan_with_backend(
            &db,
            Some(&[root_id]),
            &temp.path().join("art"),
            &cancel,
            &|event| {
                if let ScanEvent::Finished { errors, .. } = event {
                    events.lock().unwrap().extend(errors);
                }
            },
            &FakeExtraction::new(Arc::clone(&cancel)),
        )
        .unwrap();
        assert!(!events.lock().unwrap().is_empty());
        let state = load_root_state(&db, root_id).unwrap().unwrap();
        assert_eq!(state.status, "error");
        fs::remove_file(root.join("cycle")).unwrap();
        run_fake(
            &db,
            root_id,
            &temp,
            &cancel,
            &FakeExtraction::new(Arc::clone(&cancel)),
        );
        assert_eq!(
            load_root_state(&db, root_id).unwrap().unwrap().status,
            "complete"
        );
    }
}
