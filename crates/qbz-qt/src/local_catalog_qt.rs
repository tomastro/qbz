//! Background bootstrap for the derived Local Library catalog.
//!
//! C is intentionally invisible: current Local Library readers remain the
//! session fallback. This module only schedules the frontend-agnostic worker
//! after boot has had time to paint, coalesces progress into logs, and signals
//! bounded cancellation when the Qt event loop exits.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use qbz_local_catalog::{
    ActiveCatalog, BootstrapLayout, BootstrapOutcome, BootstrapProgress, CatalogError,
    ProjectionOutcome, ProjectionProgress, SourceKind,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogProgressDoc<'a> {
    active: bool,
    phase: &'a str,
    source: &'a str,
    source_instance: &'a str,
    source_done: u64,
    source_total: u64,
    overall_done: u64,
    overall_total: u64,
    source_index: usize,
    source_count: usize,
}

fn publish_progress(doc: CatalogProgressDoc<'_>) {
    let json = serde_json::to_string(&doc).unwrap_or_else(|_| "{\"active\":false}".to_string());
    crate::local_bridge::ui(move |mut bridge| {
        bridge
            .as_mut()
            .set_local_catalog_progress_json(cxx_qt_lib::QString::from(json.as_str()));
    });
}

fn publish_bootstrap_progress(progress: &BootstrapProgress) {
    publish_progress(CatalogProgressDoc {
        active: true,
        phase: "bootstrap",
        source: progress.source.source.as_str(),
        source_instance: &progress.source.source_instance,
        source_done: progress.committed_rows,
        source_total: progress.source_rows_total,
        overall_done: progress.overall_rows_written,
        overall_total: progress.overall_rows_total,
        source_index: progress.source_index,
        source_count: progress.source_count,
    });
}

fn publish_projection_progress(progress: &ProjectionProgress) {
    publish_progress(CatalogProgressDoc {
        active: true,
        phase: "reconcile",
        source: progress.source.source.as_str(),
        source_instance: &progress.source.source_instance,
        source_done: progress.rows_written,
        source_total: progress.source_rows_total,
        overall_done: progress.overall_rows_written,
        overall_total: progress.overall_rows_total,
        source_index: progress.source_index,
        source_count: progress.source_count,
    });
}

fn clear_progress() {
    crate::local_bridge::ui(|mut bridge| {
        bridge
            .as_mut()
            .set_local_catalog_progress_json(cxx_qt_lib::QString::from("{\"active\":false}"));
    });
}

enum RefreshOutcome {
    Bootstrap(BootstrapOutcome),
    Projection(ProjectionOutcome),
}

static STARTED: AtomicBool = AtomicBool::new(false);
static CANCELLED: AtomicBool = AtomicBool::new(false);
static CATCHUP_RUNNING: AtomicBool = AtomicBool::new(false);
static CATCHUP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// One profile-scoped sidecar fed by the actual legacy cache locations. Plex
/// remains installation-wide in the current backend; local and remote mirrors
/// follow the active (or guest) user.
pub(crate) fn locations() -> Option<qbz_local_catalog::LegacyLocations> {
    let root = dirs::data_dir()?.join("qbz");
    let user_id = qbz_app::user_data::UserDataPaths::load_last_user_id().unwrap_or(0);
    let user_dir = root.join("users").join(user_id.to_string());
    let mut enabled_sources = BTreeSet::from([SourceKind::Local, SourceKind::Offline]);
    if crate::local_plex::is_configured() {
        enabled_sources.insert(SourceKind::Plex);
    }
    for source in crate::media_servers_qt::configured_words() {
        match source {
            "jellyfin" => {
                enabled_sources.insert(SourceKind::Jellyfin);
            }
            "subsonic" => {
                enabled_sources.insert(SourceKind::Subsonic);
            }
            _ => {}
        }
    }
    Some(qbz_local_catalog::LegacyLocations {
        catalog_dir: user_dir.clone(),
        local_database: user_dir.join("library.db"),
        plex_database: root.join("plex_cache.db"),
        remote_database: user_dir.join("remote_cache.db"),
        enabled_sources,
    })
}

pub(crate) fn start() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    CANCELLED.store(false, Ordering::Release);
    CATCHUP_RUNNING.store(true, Ordering::Release);
    crate::spawn(async {
        // Component.onCompleted triggers `on_boot` before the first paint.
        // This delay keeps source probes and SQLite open off that critical path.
        tokio::time::sleep(Duration::from_secs(1)).await;
        if CANCELLED.load(Ordering::Acquire) {
            worker_finished();
            return;
        }
        let Some(locations) = locations() else {
            log::warn!("[local-catalog] fallback=missing-data-directory");
            worker_finished();
            return;
        };
        let result = tokio::task::spawn_blocking(move || {
            let mut last_publish = std::time::Instant::now();
            let mut last_source = None;
            let bootstrap = qbz_local_catalog::bootstrap_legacy_caches_at_with_progress(
                &locations,
                &CANCELLED,
                |progress| {
                    publish_bootstrap_progress(progress);
                    let source_changed = last_source.as_ref() != Some(&progress.source);
                    if source_changed
                        || progress.source_complete
                        || last_publish.elapsed() >= Duration::from_secs(2)
                    {
                        log::info!(
                            "[local-catalog] phase=bootstrap generation={} source={:?} rows={} checkpoint={} complete={}",
                            progress.generation,
                            progress.source,
                            progress.committed_rows,
                            progress.checkpoint_cursor,
                            progress.source_complete
                        );
                        last_publish = std::time::Instant::now();
                        last_source = Some(progress.source.clone());
                    }
                },
            )?;
            let projection = if matches!(bootstrap, BootstrapOutcome::Activated { .. })
                && !CANCELLED.load(Ordering::Acquire)
            {
                let mut last_projection = std::time::Instant::now();
                let mut last_projection_source = None;
                let visible_sources = locations.enabled_sources.clone();
                Some(qbz_local_catalog::reconcile_legacy_caches_at_with_progress(
                    &locations,
                    &CANCELLED,
                    |progress| {
                        if !visible_sources.contains(&progress.source.source) {
                            return;
                        }
                        publish_projection_progress(progress);
                        let source_changed =
                            last_projection_source.as_ref() != Some(&progress.source);
                        if source_changed
                            || progress.source_complete
                            || last_projection.elapsed() >= Duration::from_secs(2)
                        {
                            log::info!(
                                "[local-catalog] phase=catch-up generation={} source={:?} rows={} checkpoint={} complete={} prune_authorized={}",
                                progress.generation,
                                progress.source,
                                progress.rows_written,
                                progress.checkpoint_cursor,
                                progress.source_complete,
                                progress.prune_authorized
                            );
                            last_projection = std::time::Instant::now();
                            last_projection_source = Some(progress.source.clone());
                        }
                    },
                )?)
            } else {
                None
            };
            Ok::<_, CatalogError>((bootstrap, projection))
        })
        .await;
        match result {
            Ok(Ok((bootstrap, projection))) => {
                let ready = matches!(&bootstrap, BootstrapOutcome::Activated { .. });
                match bootstrap {
                    BootstrapOutcome::Activated {
                        generation,
                        track_count,
                        resumed_rows,
                    } => log::info!(
                        "[local-catalog] phase=active generation={generation} tracks={track_count} resumed_rows={resumed_rows}"
                    ),
                    BootstrapOutcome::Paused {
                        generation,
                        source,
                        committed_rows,
                    } => log::info!(
                        "[local-catalog] phase=paused generation={generation} source={source:?} committed_rows={committed_rows}"
                    ),
                    BootstrapOutcome::Fallback(reason) => {
                        log::warn!("[local-catalog] fallback={reason:?}")
                    }
                }
                if let Some(projection) = projection {
                    log_projection(projection);
                }
                if ready && crate::local_tracks_model_qt::requested() {
                    crate::local_bridge_ops::load_tracks(true);
                }
                if ready && crate::local_albums_model_qt::requested() {
                    crate::local_albums_model_qt::retry_last(true);
                }
                if ready && crate::local_artists_model_qt::requested() {
                    crate::local_artists_model_qt::retry_last();
                }
            }
            Ok(Err(CatalogError::InsufficientSpace {
                required_bytes,
                available_bytes,
            })) => log::warn!(
                "[local-catalog] phase=paused reason=low-disk required_bytes={required_bytes} available_bytes={available_bytes}"
            ),
            Ok(Err(error)) => log::warn!("[local-catalog] fallback=bootstrap-error error={error}"),
            Err(error) => log::warn!("[local-catalog] fallback=worker-join error={error}"),
        }
        clear_progress();
        worker_finished();
    });
}

/// Coalesced post-write hook for local scan and media-server sync completion.
/// If the boot worker is still running, one follow-up pass is remembered.
pub(crate) fn request_catch_up() {
    CATCHUP_REQUESTED.store(true, Ordering::Release);
    if CATCHUP_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::spawn(async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        CATCHUP_REQUESTED.store(false, Ordering::Release);
        if CANCELLED.load(Ordering::Acquire) {
            worker_finished();
            return;
        }
        let Some(locations) = locations() else {
            log::warn!("[local-catalog] fallback=missing-data-directory");
            worker_finished();
            return;
        };
        let result = tokio::task::spawn_blocking(move || {
            if !matches!(
                BootstrapLayout::new(&locations.catalog_dir).open_active(),
                ActiveCatalog::Ready { .. }
            ) {
                let mut last_progress = std::time::Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .unwrap_or_else(std::time::Instant::now);
                return qbz_local_catalog::bootstrap_legacy_caches_at_with_progress(
                    &locations,
                    &CANCELLED,
                    |progress| {
                        if progress.source_complete
                            || last_progress.elapsed() >= Duration::from_millis(100)
                        {
                            publish_bootstrap_progress(progress);
                            last_progress = std::time::Instant::now();
                        }
                    },
                )
                .map(RefreshOutcome::Bootstrap);
            }
            let mut last_publish = std::time::Instant::now();
            let mut last_source = None;
            let visible_sources = locations.enabled_sources.clone();
            qbz_local_catalog::reconcile_legacy_caches_at_with_progress(
                &locations,
                &CANCELLED,
                |progress| {
                    if !visible_sources.contains(&progress.source.source) {
                        return;
                    }
                    publish_projection_progress(progress);
                    let source_changed = last_source.as_ref() != Some(&progress.source);
                    if source_changed
                        || progress.source_complete
                        || last_publish.elapsed() >= Duration::from_secs(2)
                    {
                        log::info!(
                            "[local-catalog] phase=catch-up generation={} source={:?} rows={} checkpoint={} complete={} prune_authorized={}",
                            progress.generation,
                            progress.source,
                            progress.rows_written,
                            progress.checkpoint_cursor,
                            progress.source_complete,
                            progress.prune_authorized
                        );
                        last_publish = std::time::Instant::now();
                        last_source = Some(progress.source.clone());
                    }
                },
            )
            .map(RefreshOutcome::Projection)
        })
        .await;
        match result {
            Ok(Ok(outcome)) => {
                let ready = match outcome {
                    RefreshOutcome::Projection(outcome) => {
                        log_projection(outcome);
                        true
                    }
                    RefreshOutcome::Bootstrap(BootstrapOutcome::Activated {
                        generation,
                        track_count,
                        resumed_rows,
                    }) => {
                        log::info!(
                            "[local-catalog] phase=active generation={generation} tracks={track_count} resumed_rows={resumed_rows}"
                        );
                        true
                    }
                    RefreshOutcome::Bootstrap(BootstrapOutcome::Paused {
                        generation,
                        source,
                        committed_rows,
                    }) => {
                        log::info!(
                            "[local-catalog] phase=paused generation={generation} source={source:?} committed_rows={committed_rows}"
                        );
                        false
                    }
                    RefreshOutcome::Bootstrap(BootstrapOutcome::Fallback(reason)) => {
                        log::warn!("[local-catalog] fallback={reason:?}");
                        false
                    }
                };
                if ready && crate::local_tracks_model_qt::requested() {
                    crate::local_bridge_ops::load_tracks(true);
                }
                if ready && crate::local_albums_model_qt::requested() {
                    crate::local_albums_model_qt::retry_last(true);
                }
                if ready && crate::local_artists_model_qt::requested() {
                    crate::local_artists_model_qt::retry_last();
                }
            }
            Ok(Err(CatalogError::InsufficientSpace {
                required_bytes,
                available_bytes,
            })) => log::warn!(
                "[local-catalog] phase=catch-up-paused reason=low-disk required_bytes={required_bytes} available_bytes={available_bytes}"
            ),
            Ok(Err(error)) => log::warn!("[local-catalog] fallback=catch-up-error error={error}"),
            Err(error) => log::warn!("[local-catalog] fallback=catch-up-join error={error}"),
        }
        clear_progress();
        worker_finished();
    });
}

fn log_projection(outcome: ProjectionOutcome) {
    match outcome {
        ProjectionOutcome::UpToDate {
            generation,
            track_count,
        } => log::info!(
            "[local-catalog] phase=fresh generation={generation} tracks={track_count}"
        ),
        ProjectionOutcome::Activated {
            generation,
            track_count,
            changed_sources,
            resumed_rows,
        } => log::info!(
            "[local-catalog] phase=catch-up-active generation={generation} tracks={track_count} changed_sources={changed_sources} resumed_rows={resumed_rows}"
        ),
        ProjectionOutcome::Paused {
            generation,
            source,
            committed_rows,
        } => log::info!(
            "[local-catalog] phase=catch-up-paused generation={generation} source={source:?} committed_rows={committed_rows}"
        ),
    }
}

fn worker_finished() {
    CATCHUP_RUNNING.store(false, Ordering::Release);
    if CATCHUP_REQUESTED.swap(false, Ordering::AcqRel) && !CANCELLED.load(Ordering::Acquire) {
        request_catch_up();
    }
}

pub(crate) fn cancel() {
    CANCELLED.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_latch_is_visible_to_the_worker() {
        CANCELLED.store(false, Ordering::Release);
        cancel();
        assert!(CANCELLED.load(Ordering::Acquire));
        CANCELLED.store(false, Ordering::Release);
    }
}
