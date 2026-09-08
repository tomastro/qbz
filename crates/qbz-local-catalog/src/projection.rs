use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use crate::bootstrap::{
    acquire_lock, now_unix_ms, remove_sqlite_sidecars, sync_directory, ActiveCatalog,
    BootstrapLayout, BootstrapManifest, PreflightReport, SourceCheckpoint, SourceProbe, LOCK_NAME,
};
use crate::{Catalog, CatalogError, ProjectedTrack, Result, SourceKey, SCHEMA_VERSION};

#[derive(Debug, Clone)]
pub struct ReconciliationBatch {
    pub source: SourceKey,
    pub snapshot_version: String,
    pub expected_cursor: String,
    pub next_cursor: String,
    pub tracks: Vec<ProjectedTrack>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionProgress {
    pub generation: u64,
    pub source: SourceKey,
    pub rows_written: u64,
    pub source_rows_total: u64,
    pub overall_rows_written: u64,
    pub overall_rows_total: u64,
    /// One-based position among sources whose authoritative snapshot changed.
    pub source_index: usize,
    pub source_count: usize,
    pub checkpoint_cursor: String,
    pub source_complete: bool,
    pub prune_authorized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionOutcome {
    UpToDate {
        generation: u64,
        track_count: u64,
    },
    Activated {
        generation: u64,
        track_count: u64,
        changed_sources: usize,
        resumed_rows: u64,
    },
    Paused {
        generation: u64,
        source: SourceKey,
        committed_rows: u64,
    },
}

pub struct ProjectionSession {
    layout: BootstrapLayout,
    catalog: Catalog,
    generation: u64,
    building_path: PathBuf,
    active_generation: u64,
    _lock: File,
}

impl BootstrapLayout {
    pub fn prepare_projection(
        &self,
        probes: &[SourceProbe],
        available_bytes: Option<u64>,
    ) -> Result<(ProjectionSession, PreflightReport)> {
        let available = match available_bytes {
            Some(bytes) => bytes,
            None => self.available_bytes()?,
        };
        let active_hint = self.read_manifest().map_err(|reason| {
            CatalogError::ActivationNotReady(format!(
                "cannot preflight projection without a manifest: {reason:?}"
            ))
        })?;
        let active_bytes = fs::metadata(self.generation_path(active_hint.active_generation))?.len();
        let preflight =
            PreflightReport::evaluate(probes, available)?.with_catalog_floor(active_bytes)?;
        fs::create_dir_all(self.data_dir())?;
        let lock = acquire_lock(&self.data_dir().join(LOCK_NAME))?;
        let (active, active_manifest) = match self.open_active() {
            ActiveCatalog::Ready { catalog, manifest } => (catalog, manifest),
            ActiveCatalog::Fallback(reason) => {
                return Err(CatalogError::ActivationNotReady(format!(
                    "cannot project without an active catalog: {reason:?}"
                )))
            }
        };
        let active_generation = active_manifest.active_generation;
        let generation = self.next_building_generation(Some(active_generation))?;
        let building_path = self.building_path(generation);
        let catalog = if building_path.is_file() {
            match Catalog::open(&building_path, generation).and_then(|catalog| {
                let (phase, base) = catalog.build_phase()?;
                if phase == "projection" && base == active_generation {
                    Ok(catalog)
                } else {
                    Err(CatalogError::InvalidInput(format!(
                        "building generation belongs to {phase:?} base {base}"
                    )))
                }
            }) {
                Ok(catalog) => catalog,
                Err(_) => {
                    discard_building(&building_path)?;
                    clone_generation(&active, &building_path, active_generation, generation)?
                }
            }
        } else {
            clone_generation(&active, &building_path, active_generation, generation)?
        };
        Ok((
            ProjectionSession {
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
}

impl ProjectionSession {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn checkpoint(&self, source: &SourceKey) -> Result<Option<SourceCheckpoint>> {
        self.catalog.source_checkpoint(source)
    }

    pub fn source_watermark(&self, source: &SourceKey) -> Result<Option<String>> {
        self.catalog.source_watermark(source)
    }

    pub fn begin_source(&mut self, source: &SourceKey, snapshot_version: &str) -> Result<()> {
        self.catalog.begin_reconciliation(source, snapshot_version)
    }

    pub fn apply_batch(&mut self, batch: &ReconciliationBatch) -> Result<SourceCheckpoint> {
        self.catalog.apply_reconciliation_batch(batch)
    }

    pub fn stats(&self) -> Result<crate::CatalogStats> {
        self.catalog.stats()
    }

    pub fn activate(mut self, changed: &[SourceProbe]) -> Result<BootstrapManifest> {
        self.catalog.rebuild_materialized_views()?;
        if !self.catalog.materialized_views_valid()? {
            return Err(CatalogError::ActivationNotReady(
                "projection materializations do not match tracks".to_string(),
            ));
        }
        let actual = self
            .catalog
            .stats()?
            .source_counts
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        for probe in changed {
            let count = actual.get(&probe.source).copied().unwrap_or(0);
            if count != probe.row_count {
                return Err(CatalogError::ActivationNotReady(format!(
                    "reconciled count for {:?} is {count}, expected {}",
                    probe.source, probe.row_count
                )));
            }
            let checkpoint = self
                .catalog
                .source_checkpoint(&probe.source)?
                .ok_or_else(|| {
                    CatalogError::ActivationNotReady(format!(
                        "missing reconciled checkpoint for {:?}",
                        probe.source
                    ))
                })?;
            let watermark = self
                .catalog
                .source_watermark(&probe.source)?
                .unwrap_or_default();
            if !checkpoint.complete
                || checkpoint.checkpoint_rows != probe.row_count
                || checkpoint.checkpoint_version != probe.snapshot_version
                || watermark != probe.snapshot_version
            {
                return Err(CatalogError::ActivationNotReady(format!(
                    "watermark/checkpoint is incomplete for {:?}",
                    probe.source
                )));
            }
        }
        let integrity = self.catalog.integrity_check()?;
        if !integrity.sqlite_ok || integrity.foreign_key_violations != 0 || !integrity.fts_ok {
            return Err(CatalogError::ActivationNotReady(format!(
                "projection integrity failed: {integrity:?}"
            )));
        }
        self.catalog.checkpoint_for_activation()?;
        let final_path = self.layout.generation_path(self.generation);
        drop(self.catalog);
        fs::rename(&self.building_path, &final_path)?;
        sync_directory(self.layout.data_dir())?;
        let manifest = BootstrapManifest {
            manifest_version: 1,
            schema_version: SCHEMA_VERSION,
            active_generation: self.generation,
            previous_generation: Some(self.active_generation),
            activated_at_unix_ms: now_unix_ms(),
        };
        self.layout.write_manifest(&manifest)?;
        Ok(manifest)
    }
}

fn clone_generation(
    active: &Catalog,
    building_path: &Path,
    active_generation: u64,
    generation: u64,
) -> Result<Catalog> {
    discard_building(building_path)?;
    active.backup_to(building_path)?;
    Catalog::adopt_generation(building_path, active_generation, generation)
}

fn discard_building(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    remove_sqlite_sidecars(path)
}
