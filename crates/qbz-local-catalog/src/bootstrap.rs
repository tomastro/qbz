use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{Catalog, CatalogError, ProjectedTrack, Result, SourceKey, SCHEMA_VERSION};

pub const BOOTSTRAP_BATCH_ROWS: usize = 250;
const CATALOG_BYTES_PER_TRACK: u64 = 1_280;
const SAFETY_FLOOR_BYTES: u64 = 256 * 1024 * 1024;
const MANIFEST_NAME: &str = "local_catalog-v1-manifest.json";
pub(crate) const LOCK_NAME: &str = "local_catalog-v1-bootstrap.lock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProbe {
    pub source: SourceKey,
    pub source_path: PathBuf,
    pub snapshot_version: String,
    pub row_count: u64,
    pub page_bytes: u64,
    pub integrity_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightReport {
    pub source_count: usize,
    pub track_count: u64,
    pub source_page_bytes: u64,
    pub estimated_catalog_bytes: u64,
    pub required_available_bytes: u64,
    pub available_bytes: u64,
}

impl PreflightReport {
    pub fn evaluate(probes: &[SourceProbe], available_bytes: u64) -> Result<Self> {
        if let Some(probe) = probes.iter().find(|probe| !probe.integrity_ok) {
            return Err(CatalogError::InvalidSource(format!(
                "{} failed SQLite quick_check",
                probe.source_path.display()
            )));
        }
        let track_count = probes
            .iter()
            .fold(0_u64, |total, probe| total.saturating_add(probe.row_count));
        let mut source_files = BTreeMap::<&Path, u64>::new();
        for probe in probes {
            source_files
                .entry(&probe.source_path)
                .and_modify(|bytes| *bytes = (*bytes).max(probe.page_bytes))
                .or_insert(probe.page_bytes);
        }
        let source_page_bytes = source_files
            .values()
            .fold(0_u64, |total, bytes| total.saturating_add(*bytes));
        let row_estimate = track_count.saturating_mul(CATALOG_BYTES_PER_TRACK);
        let estimated_catalog_bytes = source_page_bytes.max(row_estimate);
        let margin = estimated_catalog_bytes.saturating_add(3) / 4;
        let required_available_bytes = estimated_catalog_bytes
            .saturating_add(margin)
            .saturating_add(SAFETY_FLOOR_BYTES);
        let report = Self {
            source_count: probes.len(),
            track_count,
            source_page_bytes,
            estimated_catalog_bytes,
            required_available_bytes,
            available_bytes,
        };
        if available_bytes < required_available_bytes {
            return Err(CatalogError::InsufficientSpace {
                required_bytes: required_available_bytes,
                available_bytes,
            });
        }
        Ok(report)
    }

    pub(crate) fn with_catalog_floor(mut self, catalog_bytes: u64) -> Result<Self> {
        if catalog_bytes <= self.estimated_catalog_bytes {
            return Ok(self);
        }
        self.estimated_catalog_bytes = catalog_bytes;
        let margin = catalog_bytes.saturating_add(3) / 4;
        self.required_available_bytes = catalog_bytes
            .saturating_add(margin)
            .saturating_add(SAFETY_FLOOR_BYTES);
        if self.available_bytes < self.required_available_bytes {
            return Err(CatalogError::InsufficientSpace {
                required_bytes: self.required_available_bytes,
                available_bytes: self.available_bytes,
            });
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootstrapManifest {
    pub manifest_version: u32,
    pub schema_version: u32,
    pub active_generation: u64,
    pub previous_generation: Option<u64>,
    pub activated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GenerationPruneReport {
    pub removed_generations: usize,
    pub reclaimed_bytes: u64,
    pub failed_generations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    NoManifest,
    InvalidManifest(String),
    MissingGeneration(PathBuf),
    CatalogRejected(String),
}

pub enum ActiveCatalog {
    Ready {
        catalog: Catalog,
        manifest: BootstrapManifest,
    },
    Fallback(FallbackReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCheckpoint {
    pub source: SourceKey,
    pub available: bool,
    pub checkpoint_cursor: String,
    pub checkpoint_rows: u64,
    pub checkpoint_version: String,
    pub complete_generation: u64,
    pub complete: bool,
}

#[derive(Debug, Clone)]
pub struct BootstrapBatch {
    pub source: SourceKey,
    pub snapshot_version: String,
    pub expected_cursor: String,
    pub next_cursor: String,
    pub tracks: Vec<ProjectedTrack>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapOutcome {
    Activated {
        generation: u64,
        track_count: u64,
        resumed_rows: u64,
    },
    Paused {
        generation: u64,
        source: Option<SourceKey>,
        committed_rows: u64,
    },
    Fallback(FallbackReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapProgress {
    pub generation: u64,
    pub source: SourceKey,
    pub committed_rows: u64,
    pub source_rows_total: u64,
    pub overall_rows_written: u64,
    pub overall_rows_total: u64,
    /// One-based position of this source in the current bootstrap.
    pub source_index: usize,
    pub source_count: usize,
    pub checkpoint_cursor: String,
    pub source_complete: bool,
}

#[derive(Debug, Clone)]
pub struct BootstrapLayout {
    data_dir: PathBuf,
}

impl BootstrapLayout {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.data_dir.join(MANIFEST_NAME)
    }

    pub fn generation_path(&self, generation: u64) -> PathBuf {
        self.data_dir
            .join(format!("local_catalog-v1-g{generation}.db"))
    }

    pub fn building_path(&self, generation: u64) -> PathBuf {
        self.data_dir
            .join(format!("local_catalog-v1-g{generation}.db.building"))
    }

    pub fn read_manifest(&self) -> std::result::Result<BootstrapManifest, FallbackReason> {
        let path = self.manifest_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(FallbackReason::NoManifest)
            }
            Err(error) => return Err(FallbackReason::InvalidManifest(error.to_string())),
        };
        let manifest: BootstrapManifest = serde_json::from_slice(&bytes)
            .map_err(|error| FallbackReason::InvalidManifest(error.to_string()))?;
        if manifest.manifest_version != 1 || manifest.schema_version != SCHEMA_VERSION {
            return Err(FallbackReason::InvalidManifest(format!(
                "unsupported manifest/schema {}/{}",
                manifest.manifest_version, manifest.schema_version
            )));
        }
        Ok(manifest)
    }

    pub fn open_active(&self) -> ActiveCatalog {
        let manifest = match self.read_manifest() {
            Ok(manifest) => manifest,
            Err(reason) => return ActiveCatalog::Fallback(reason),
        };
        let path = self.generation_path(manifest.active_generation);
        if !path.is_file() {
            return ActiveCatalog::Fallback(FallbackReason::MissingGeneration(path));
        }
        match Catalog::open_read_only(&path, manifest.active_generation) {
            Ok(catalog) => ActiveCatalog::Ready { catalog, manifest },
            Err(error) => {
                ActiveCatalog::Fallback(FallbackReason::CatalogRejected(error.to_string()))
            }
        }
    }

    pub fn available_bytes(&self) -> Result<u64> {
        let probe_path = nearest_existing_ancestor(&self.data_dir).ok_or_else(|| {
            CatalogError::InvalidInput(format!(
                "no existing ancestor for {}",
                self.data_dir.display()
            ))
        })?;
        filesystem_available_bytes(probe_path)
    }

    pub fn preflight(&self, probes: &[SourceProbe]) -> Result<PreflightReport> {
        PreflightReport::evaluate(probes, self.available_bytes()?)
    }

    pub fn prepare(
        &self,
        probes: &[SourceProbe],
        available_bytes: Option<u64>,
    ) -> Result<(BootstrapSession, PreflightReport)> {
        let available = match available_bytes {
            Some(bytes) => bytes,
            None => self.available_bytes()?,
        };
        let preflight = PreflightReport::evaluate(probes, available)?;
        fs::create_dir_all(&self.data_dir)?;
        let lock = acquire_lock(&self.data_dir.join(LOCK_NAME))?;
        let active_generation = self
            .read_manifest()
            .ok()
            .map(|manifest| manifest.active_generation);
        let generation = self.next_building_generation(active_generation)?;
        let building_path = self.building_path(generation);
        let base_generation = active_generation.unwrap_or(0);
        let catalog = match Catalog::open(&building_path, generation).and_then(|mut catalog| {
            let (phase, base) = catalog.build_phase()?;
            if phase.is_empty() {
                catalog.set_build_phase("bootstrap", base_generation)?;
                Ok(catalog)
            } else if phase == "bootstrap" && base == base_generation {
                Ok(catalog)
            } else {
                Err(CatalogError::InvalidInput(format!(
                    "building generation belongs to {phase:?} base {base}"
                )))
            }
        }) {
            Ok(catalog) => catalog,
            Err(error) if building_path.is_file() => {
                // Only the exact incomplete derived sidecar is recoverable here.
                // Authoritative databases and active generations are never targets.
                fs::remove_file(&building_path)?;
                remove_sqlite_sidecars(&building_path)?;
                let mut catalog = Catalog::open(&building_path, generation).map_err(|retry| {
                    CatalogError::InvalidSource(format!(
                        "rebuild after rejected sidecar ({error}) also failed: {retry}"
                    ))
                })?;
                catalog.set_build_phase("bootstrap", base_generation)?;
                catalog
            }
            Err(error) => return Err(error),
        };
        Ok((
            BootstrapSession {
                layout: self.clone(),
                catalog,
                generation,
                building_path,
                active_generation,
                _lock: lock,
            },
            preflight,
        ))
    }

    pub(crate) fn write_manifest(&self, manifest: &BootstrapManifest) -> Result<()> {
        let path = self.manifest_path();
        let temporary = self.data_dir.join(format!("{MANIFEST_NAME}.tmp"));
        let bytes = serde_json::to_vec_pretty(manifest)?;
        {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        fs::rename(&temporary, &path)?;
        sync_directory(&self.data_dir)?;
        self.prune_obsolete_generations(manifest);
        Ok(())
    }

    /// Retain only the active catalog and its immediate rollback generation.
    ///
    /// The manifest is the source of truth and is always published before this
    /// maintenance runs. Cleanup is deliberately best-effort: Windows may
    /// still have an older generation open briefly, and a failed unlink must
    /// never make a successfully activated catalog look like a failed one.
    /// A later startup or activation will retry any generation left behind.
    pub fn prune_obsolete_generations(
        &self,
        manifest: &BootstrapManifest,
    ) -> GenerationPruneReport {
        let entries = match fs::read_dir(&self.data_dir) {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!(
                    "[local-catalog] generation cleanup could not scan {}: {error}",
                    self.data_dir.display()
                );
                return GenerationPruneReport {
                    failed_generations: 1,
                    ..GenerationPruneReport::default()
                };
            }
        };
        let mut generations = BTreeSet::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let generation = [".db", ".db-wal", ".db-shm"]
                .into_iter()
                .find_map(|suffix| parse_generation_name(&name, suffix));
            if let Some(generation) = generation {
                generations.insert(generation);
            }
        }
        // A schema rebuild may not be able to validate the previous manifest,
        // leaving `previous_generation` empty even though the immediately
        // preceding finalized database is still the safest rollback artifact.
        let previous_generation = manifest.previous_generation.or_else(|| {
            generations
                .range(..manifest.active_generation)
                .next_back()
                .copied()
        });
        let keep = [Some(manifest.active_generation), previous_generation]
            .into_iter()
            .flatten()
            .collect::<BTreeSet<_>>();
        let stale = generations
            .into_iter()
            .filter(|generation| !keep.contains(generation))
            .collect::<Vec<_>>();

        let mut report = GenerationPruneReport::default();
        for generation in stale {
            let database = self.generation_path(generation);
            let mut failed = false;
            let mut removed_any = false;
            let database_bytes = fs::metadata(&database)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let database_gone = match fs::remove_file(&database) {
                Ok(()) => {
                    removed_any = true;
                    report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(database_bytes);
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(error) => {
                    failed = true;
                    log::warn!(
                        "[local-catalog] generation cleanup could not remove {}: {error}",
                        database.display()
                    );
                    false
                }
            };
            // If Windows still has the database open, leave its WAL/SHM pair
            // untouched as well. The next maintenance pass retries the trio.
            for path in database_gone
                .then(|| {
                    [
                        PathBuf::from(format!("{}-wal", database.display())),
                        PathBuf::from(format!("{}-shm", database.display())),
                    ]
                })
                .into_iter()
                .flatten()
            {
                let bytes = fs::metadata(&path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0);
                match fs::remove_file(&path) {
                    Ok(()) => {
                        removed_any = true;
                        report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(bytes);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        failed = true;
                        log::warn!(
                            "[local-catalog] generation cleanup could not remove {}: {error}",
                            path.display()
                        );
                    }
                }
            }
            if failed {
                report.failed_generations += 1;
            } else if removed_any {
                report.removed_generations += 1;
            }
        }
        if report.removed_generations > 0 || report.failed_generations > 0 {
            log::info!(
                "[local-catalog] generation cleanup: removed={} reclaimed_bytes={} failed={} retained_active={} retained_previous={:?}",
                report.removed_generations,
                report.reclaimed_bytes,
                report.failed_generations,
                manifest.active_generation,
                previous_generation
            );
        }
        report
    }

    pub(crate) fn next_building_generation(&self, active_generation: Option<u64>) -> Result<u64> {
        let mut maximum_final = active_generation.unwrap_or(0);
        let mut building = Vec::new();
        for entry in fs::read_dir(&self.data_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(generation) = parse_generation_name(&name, ".db") {
                maximum_final = maximum_final.max(generation);
            } else if let Some(generation) = parse_generation_name(&name, ".db.building") {
                building.push(generation);
            }
        }
        Ok(building
            .into_iter()
            .filter(|generation| *generation > maximum_final)
            .min()
            .unwrap_or_else(|| maximum_final.saturating_add(1).max(1)))
    }
}

pub struct BootstrapSession {
    layout: BootstrapLayout,
    catalog: Catalog,
    generation: u64,
    building_path: PathBuf,
    active_generation: Option<u64>,
    _lock: File,
}

impl BootstrapSession {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn building_path(&self) -> &Path {
        &self.building_path
    }

    pub fn checkpoint(&self, source: &SourceKey) -> Result<Option<SourceCheckpoint>> {
        self.catalog.source_checkpoint(source)
    }

    pub fn restart_changed_source(
        &mut self,
        source: &SourceKey,
        snapshot_version: &str,
    ) -> Result<()> {
        self.catalog
            .restart_projection_source(source, snapshot_version)
    }

    pub fn apply_batch(&mut self, batch: &BootstrapBatch) -> Result<SourceCheckpoint> {
        self.catalog.apply_bootstrap_batch(batch)
    }

    pub fn stats(&self) -> Result<crate::CatalogStats> {
        self.catalog.stats()
    }

    pub fn activate(mut self, expected: &[SourceProbe]) -> Result<BootstrapManifest> {
        self.catalog.rebuild_materialized_views()?;
        if !self.catalog.materialized_views_valid()? {
            return Err(CatalogError::ActivationNotReady(
                "album/artist materializations do not match projected tracks".to_string(),
            ));
        }
        let expected_counts = expected
            .iter()
            .filter(|probe| probe.row_count > 0)
            .map(|probe| (probe.source.clone(), probe.row_count))
            .collect::<BTreeMap<_, _>>();
        let actual_stats = self.catalog.stats()?;
        let actual_counts = actual_stats
            .source_counts
            .iter()
            .cloned()
            .collect::<BTreeMap<_, _>>();
        if expected_counts != actual_counts {
            return Err(CatalogError::ActivationNotReady(format!(
                "source counts differ: expected {expected_counts:?}, actual {actual_counts:?}"
            )));
        }
        for probe in expected {
            let checkpoint = self
                .catalog
                .source_checkpoint(&probe.source)?
                .ok_or_else(|| {
                    CatalogError::ActivationNotReady(format!(
                        "missing checkpoint for {:?}",
                        probe.source
                    ))
                })?;
            if !checkpoint.complete
                || checkpoint.checkpoint_version != probe.snapshot_version
                || checkpoint.checkpoint_rows != probe.row_count
            {
                return Err(CatalogError::ActivationNotReady(format!(
                    "incomplete checkpoint for {:?}: {checkpoint:?}",
                    probe.source
                )));
            }
        }
        let integrity = self.catalog.integrity_check()?;
        if !integrity.sqlite_ok || integrity.foreign_key_violations != 0 || !integrity.fts_ok {
            return Err(CatalogError::ActivationNotReady(format!(
                "integrity failed: {integrity:?}"
            )));
        }
        self.catalog.checkpoint_for_activation()?;
        let generation = self.generation;
        let final_path = self.layout.generation_path(generation);
        drop(self.catalog);
        fs::rename(&self.building_path, &final_path)?;
        sync_directory(self.layout.data_dir())?;
        let manifest = BootstrapManifest {
            manifest_version: 1,
            schema_version: SCHEMA_VERSION,
            active_generation: generation,
            previous_generation: self.active_generation,
            activated_at_unix_ms: now_unix_ms(),
        };
        self.layout.write_manifest(&manifest)?;
        Ok(manifest)
    }
}

pub(crate) fn acquire_lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use rustix::fs::{flock, FlockOperation};
        flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            if error == rustix::io::Errno::WOULDBLOCK {
                CatalogError::BootstrapBusy
            } else {
                CatalogError::Io(std::io::Error::from_raw_os_error(error.raw_os_error()))
            }
        })?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
        use windows_sys::Win32::Storage::FileSystem::{
            LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
        };

        let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED =
            unsafe { std::mem::zeroed() };
        // SAFETY: `file` is open, so its handle is valid for the call; the
        // zeroed OVERLAPPED lives for the whole call; locking 0..u64::MAX is
        // the documented "lock the entire file" idiom. The lock is released
        // when `file` drops and the handle closes, matching flock().
        let ok = unsafe {
            LockFileEx(
                file.as_raw_handle() as _,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            )
        };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            return Err(
                if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
                    CatalogError::BootstrapBusy
                } else {
                    CatalogError::Io(error)
                },
            );
        }
    }
    Ok(file)
}

pub(crate) fn remove_sqlite_sidecars(database: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", database.display()));
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn nearest_existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|candidate| candidate.exists())
}

fn parse_generation_name(name: &str, suffix: &str) -> Option<u64> {
    name.strip_prefix("local_catalog-v1-g")?
        .strip_suffix(suffix)?
        .parse()
        .ok()
}

#[cfg(unix)]
fn filesystem_available_bytes(path: &Path) -> Result<u64> {
    let status = rustix::fs::statvfs(path)
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    Ok(status.f_bavail.saturating_mul(status.f_frsize))
}

#[cfg(windows)]
fn filesystem_available_bytes(path: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    // GetDiskFreeSpaceExW wants a path that exists. `available_bytes()` already
    // hands us the nearest existing ancestor, but keep the walk local so a
    // future caller cannot turn this into ERROR_PATH_NOT_FOUND on first run.
    let probe = nearest_existing_ancestor(path).unwrap_or(path);
    let wide: Vec<u16> = probe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_to_caller: u64 = 0;
    // SAFETY: `wide` is NUL-terminated and outlives the call; `free_to_caller`
    // is a valid u64 slot; the remaining two out-params are documented as
    // optional and are passed as null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(CatalogError::Io(std::io::Error::last_os_error()));
    }
    Ok(free_to_caller)
}

#[cfg(not(any(unix, windows)))]
fn filesystem_available_bytes(_path: &Path) -> Result<u64> {
    Err(CatalogError::InvalidInput(
        "filesystem free-space preflight is unsupported on this platform".to_string(),
    ))
}

#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}
