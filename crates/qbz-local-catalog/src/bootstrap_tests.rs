use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use rusqlite::Connection;
use tempfile::tempdir;

use crate::{
    bootstrap_legacy_caches, bootstrap_legacy_caches_at_with_progress, ActiveCatalog,
    BootstrapBatch, BootstrapLayout, BootstrapManifest, BootstrapOutcome, Catalog, CatalogError,
    FallbackReason, LegacyLocations, PreflightReport, ProjectedTrack, ProjectionOutcome,
    QueryDescriptor, SourceKey, SourceKind, SourceProbe, TrackRef, BOOTSTRAP_BATCH_ROWS,
    SCHEMA_VERSION,
};

fn source(kind: SourceKind, instance: &str) -> SourceKey {
    SourceKey {
        source: kind,
        source_instance: instance.to_string(),
    }
}

fn probe(path: &std::path::Path, source: &SourceKey, version: &str, rows: u64) -> SourceProbe {
    SourceProbe {
        source: source.clone(),
        source_path: path.to_path_buf(),
        snapshot_version: version.to_string(),
        row_count: rows,
        page_bytes: rows.saturating_mul(512),
        integrity_ok: true,
    }
}

fn track(source: &SourceKey, id: u64) -> ProjectedTrack {
    ProjectedTrack {
        track_ref: TrackRef {
            source: source.source,
            source_instance: source.source_instance.clone(),
            native_id: id.to_string(),
        },
        source_raw: source.source.as_str().to_string(),
        local_track_id: (source.source == SourceKind::Local).then_some(id as i64),
        local_path: (source.source == SourceKind::Local).then(|| format!("/music/{id}.flac")),
        native_album_id: Some(format!("album-{}", id / 20)),
        source_copy_id: None,
        title: format!("Track {id:06}"),
        artist: format!("Artist {}", id % 31),
        album_artist: format!("Artist {}", id % 31),
        album: format!("Album {}", id % 97),
        duration_ms: 180_000 + id,
        year: Some(1980 + (id % 44) as u32),
        disc_number: Some(1),
        track_number: Some((id % 20 + 1) as u32),
        format: "flac".to_string(),
        bit_depth: Some(24),
        sample_rate_hz: Some(96_000),
        artwork_token: Some(format!("art-{id}")),
        isrc: None,
        musicbrainz_recording_id: None,
        added_at: id as i64,
        available: true,
        observed_generation: 0,
        credits: Vec::new(),
    }
}

fn apply_range(
    session: &mut crate::BootstrapSession,
    source: &SourceKey,
    version: &str,
    start: u64,
    end: u64,
    total: u64,
) {
    let mut cursor = if start == 0 {
        String::new()
    } else {
        start.to_string()
    };
    let mut next = start;
    while next < end {
        let batch_end = (next + BOOTSTRAP_BATCH_ROWS as u64).min(end);
        let rows = (next..batch_end)
            .map(|id| track(source, id + 1))
            .collect::<Vec<_>>();
        let complete = batch_end == total;
        let saved = session
            .apply_batch(&BootstrapBatch {
                source: source.clone(),
                snapshot_version: version.to_string(),
                expected_cursor: cursor,
                next_cursor: batch_end.to_string(),
                tracks: rows,
                complete,
            })
            .unwrap();
        cursor = saved.checkpoint_cursor;
        next = batch_end;
    }
}

#[test]
fn preflight_enforces_margin_before_creating_a_sidecar() {
    let temp = tempdir().unwrap();
    let data_dir = temp.path().join("catalog-data");
    let source_path = temp.path().join("library.db");
    fs::write(&source_path, b"authoritative-fixture").unwrap();
    let before = fs::read(&source_path).unwrap();
    let key = source(SourceKind::Local, "library");
    let probe = probe(&source_path, &key, "v1", 10_000);
    let layout = BootstrapLayout::new(&data_dir);

    let error = match layout.prepare(&[probe.clone()], Some(0)) {
        Ok(_) => panic!("zero free bytes must fail preflight"),
        Err(error) => error,
    };
    assert!(matches!(error, CatalogError::InsufficientSpace { .. }));
    assert!(!data_dir.exists());
    assert_eq!(fs::read(&source_path).unwrap(), before);

    let report = PreflightReport::evaluate(&[probe], u64::MAX).unwrap();
    assert_eq!(report.estimated_catalog_bytes, 12_800_000);
    assert_eq!(
        report.required_available_bytes,
        12_800_000 + 3_200_000 + 256 * 1024 * 1024
    );
}

#[test]
fn page_bytes_are_counted_once_for_multiple_sources_in_one_cache() {
    let path = std::path::Path::new("remote_cache.db");
    let probes = [
        SourceProbe {
            source: source(SourceKind::Jellyfin, "server"),
            source_path: path.to_path_buf(),
            snapshot_version: "a".to_string(),
            row_count: 1,
            page_bytes: 9_000,
            integrity_ok: true,
        },
        SourceProbe {
            source: source(SourceKind::Subsonic, "server"),
            source_path: path.to_path_buf(),
            snapshot_version: "b".to_string(),
            row_count: 1,
            page_bytes: 9_000,
            integrity_ok: true,
        },
    ];
    let report = PreflightReport::evaluate(&probes, u64::MAX).unwrap();
    assert_eq!(report.source_page_bytes, 9_000);
}

#[test]
fn building_file_is_never_active_and_previous_generation_survives() {
    let temp = tempdir().unwrap();
    let layout = BootstrapLayout::new(temp.path());
    let key = source(SourceKind::Local, "library");
    let first_probe = probe(&temp.path().join("library.db"), &key, "v1", 2);
    let (mut first, _) = layout
        .prepare(&[first_probe.clone()], Some(u64::MAX))
        .unwrap();
    apply_range(&mut first, &key, "v1", 0, 2, 2);
    let manifest = first.activate(&[first_probe]).unwrap();
    assert_eq!(manifest.active_generation, 1);

    let next_probe = probe(&temp.path().join("library.db"), &key, "v2", 3);
    let (mut second, _) = layout
        .prepare(&[next_probe.clone()], Some(u64::MAX))
        .unwrap();
    apply_range(&mut second, &key, "v2", 0, 1, 3);
    assert!(second.building_path().is_file());
    match layout.open_active() {
        ActiveCatalog::Ready { catalog, manifest } => {
            assert_eq!(manifest.active_generation, 1);
            assert_eq!(catalog.stats().unwrap().track_count, 2);
        }
        ActiveCatalog::Fallback(reason) => panic!("unexpected fallback: {reason:?}"),
    }
    drop(second);
    assert_eq!(layout.read_manifest().unwrap().active_generation, 1);
}

#[test]
fn activation_prunes_generations_older_than_the_rollback_generation() {
    let temp = tempdir().unwrap();
    let layout = BootstrapLayout::new(temp.path());
    let key = source(SourceKind::Local, "library");

    for generation in 1..=3 {
        let snapshot = format!("v{generation}");
        let current_probe = probe(&temp.path().join("library.db"), &key, &snapshot, generation);
        let (mut session, _) = layout
            .prepare(&[current_probe.clone()], Some(u64::MAX))
            .unwrap();
        apply_range(&mut session, &key, &snapshot, 0, generation, generation);
        session.activate(&[current_probe]).unwrap();

        if generation == 2 {
            fs::write(
                PathBuf::from(format!("{}-wal", layout.generation_path(1).display())),
                b"stale wal",
            )
            .unwrap();
            fs::write(
                PathBuf::from(format!("{}-shm", layout.generation_path(1).display())),
                b"stale shm",
            )
            .unwrap();
            fs::write(
                temp.path().join("local_catalog-v1-g1.db.backup"),
                b"keep me",
            )
            .unwrap();
        }
    }

    let manifest = layout.read_manifest().unwrap();
    assert_eq!(manifest.active_generation, 3);
    assert_eq!(manifest.previous_generation, Some(2));
    assert!(!layout.generation_path(1).exists());
    assert!(!PathBuf::from(format!("{}-wal", layout.generation_path(1).display())).exists());
    assert!(!PathBuf::from(format!("{}-shm", layout.generation_path(1).display())).exists());
    assert!(layout.generation_path(2).is_file());
    assert!(layout.generation_path(3).is_file());
    assert!(temp.path().join("local_catalog-v1-g1.db.backup").is_file());
}

#[test]
fn interrupted_bootstrap_resumes_at_ten_fifty_and_ninety_nine_percent() {
    const TOTAL: u64 = 1_001;
    for stopped in [100_u64, 500, 990] {
        let temp = tempdir().unwrap();
        let layout = BootstrapLayout::new(temp.path());
        let key = source(SourceKind::Plex, "server-a");
        let probe = probe(&temp.path().join("plex_cache.db"), &key, "stable", TOTAL);
        let (mut interrupted, _) = layout.prepare(&[probe.clone()], Some(u64::MAX)).unwrap();
        apply_range(&mut interrupted, &key, "stable", 0, stopped, TOTAL);
        drop(interrupted);

        let (mut resumed, _) = layout.prepare(&[probe.clone()], Some(u64::MAX)).unwrap();
        let checkpoint = resumed.checkpoint(&key).unwrap().unwrap();
        assert_eq!(checkpoint.checkpoint_rows, stopped);
        assert_eq!(checkpoint.checkpoint_cursor, stopped.to_string());
        assert!(!checkpoint.complete);
        apply_range(&mut resumed, &key, "stable", stopped, TOTAL, TOTAL);
        resumed.activate(&[probe]).unwrap();

        let ActiveCatalog::Ready { catalog, .. } = layout.open_active() else {
            panic!("resumed catalog was not activated")
        };
        let page = catalog
            .query_tracks(&QueryDescriptor::tracks(), None, 500)
            .unwrap();
        assert_eq!(catalog.stats().unwrap().track_count, TOTAL);
        assert_eq!(page.rows.len(), 500);
        let distinct = page
            .rows
            .iter()
            .map(|row| row.track_ref.clone())
            .collect::<HashSet<_>>();
        assert_eq!(distinct.len(), 500);
    }
}

#[test]
fn snapshot_change_restarts_only_the_derived_source() {
    let temp = tempdir().unwrap();
    let layout = BootstrapLayout::new(temp.path());
    let key = source(SourceKind::Jellyfin, "jelly");
    let v1 = probe(&temp.path().join("remote_cache.db"), &key, "v1", 4);
    let (mut session, _) = layout.prepare(&[v1], Some(u64::MAX)).unwrap();
    apply_range(&mut session, &key, "v1", 0, 2, 4);
    assert!(matches!(
        session.apply_batch(&BootstrapBatch {
            source: key.clone(),
            snapshot_version: "v2".to_string(),
            expected_cursor: "2".to_string(),
            next_cursor: "3".to_string(),
            tracks: vec![track(&key, 3)],
            complete: false,
        }),
        Err(CatalogError::SourceSnapshotChanged(_))
    ));
    session.restart_changed_source(&key, "v2").unwrap();
    assert_eq!(session.stats().unwrap().track_count, 0);
    let checkpoint = session.checkpoint(&key).unwrap().unwrap();
    assert_eq!(checkpoint.checkpoint_rows, 0);
    assert_eq!(checkpoint.checkpoint_version, "v2");
}

#[test]
fn corrupt_building_is_rebuilt_but_unrelated_files_are_preserved() {
    let temp = tempdir().unwrap();
    let layout = BootstrapLayout::new(temp.path());
    let source_path = temp.path().join("library.db");
    fs::write(&source_path, b"do-not-touch").unwrap();
    fs::write(layout.building_path(1), b"not sqlite").unwrap();
    let key = source(SourceKind::Local, "library");
    let probe = probe(&source_path, &key, "v1", 0);
    let (mut session, _) = layout.prepare(&[probe.clone()], Some(u64::MAX)).unwrap();
    session
        .apply_batch(&BootstrapBatch {
            source: key,
            snapshot_version: "v1".to_string(),
            expected_cursor: String::new(),
            next_cursor: String::new(),
            tracks: Vec::new(),
            complete: true,
        })
        .unwrap();
    session.activate(&[probe]).unwrap();
    assert_eq!(fs::read(source_path).unwrap(), b"do-not-touch");
}

#[test]
fn corrupt_active_catalog_falls_back_without_promoting_a_building_file() {
    let temp = tempdir().unwrap();
    let layout = BootstrapLayout::new(temp.path());
    let key = source(SourceKind::Local, "library");
    let probe = probe(&temp.path().join("library.db"), &key, "v1", 1);
    let (mut session, _) = layout.prepare(&[probe.clone()], Some(u64::MAX)).unwrap();
    apply_range(&mut session, &key, "v1", 0, 1, 1);
    session.activate(&[probe]).unwrap();
    fs::write(layout.generation_path(1), b"corrupt derived data").unwrap();
    Catalog::open(&layout.building_path(2), 2).unwrap();

    assert!(matches!(
        layout.open_active(),
        ActiveCatalog::Fallback(FallbackReason::CatalogRejected(_))
    ));
}

#[test]
fn legacy_fixture_bootstraps_all_sources_read_only_and_is_idempotent() {
    let temp = tempdir().unwrap();
    create_local_fixture(&temp.path().join("library.db"));
    create_plex_fixture(&temp.path().join("plex_cache.db"));
    create_remote_fixture(&temp.path().join("remote_cache.db"));
    let source_sizes = ["library.db", "plex_cache.db", "remote_cache.db"].map(|name| {
        let path = temp.path().join(name);
        (path.clone(), fs::metadata(path).unwrap().len())
    });

    let started = Instant::now();
    let outcome = bootstrap_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap();
    let elapsed = started.elapsed();
    assert!(matches!(
        outcome,
        BootstrapOutcome::Activated {
            generation: 1,
            track_count: 505,
            ..
        }
    ));
    let ActiveCatalog::Ready { catalog, .. } = BootstrapLayout::new(temp.path()).open_active()
    else {
        panic!("legacy fixture did not activate")
    };
    let stats = catalog.stats().unwrap();
    let counts = stats.source_counts.into_iter().collect::<HashMap<_, _>>();
    assert_eq!(counts[&source(SourceKind::Local, "library")], 2);
    assert_eq!(counts[&source(SourceKind::Plex, "plex-server")], 501);
    assert_eq!(counts[&source(SourceKind::Jellyfin, "jf-server")], 1);
    assert_eq!(counts[&source(SourceKind::Subsonic, "sub-server")], 1);
    let aggregate_counts: (i64, i64, i64) = catalog
        .connection()
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM albums_materialized),
                 (SELECT COALESCE(SUM(track_count),0) FROM albums_materialized),
                 (SELECT COUNT(*) FROM artists_materialized)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(aggregate_counts.1, 505);
    assert!(aggregate_counts.0 >= 4);
    assert!(aggregate_counts.2 >= 5);
    let catalog_bytes = fs::metadata(BootstrapLayout::new(temp.path()).generation_path(1))
        .unwrap()
        .len();
    println!(
        "C_BOOTSTRAP_METRIC rows=505 elapsed_ms={:.3} catalog_bytes={catalog_bytes} albums={} artists={}",
        elapsed.as_secs_f64() * 1_000.0,
        aggregate_counts.0,
        aggregate_counts.2
    );
    for (path, size) in source_sizes {
        assert_eq!(fs::metadata(path).unwrap().len(), size);
    }

    let second = bootstrap_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap();
    assert!(matches!(
        second,
        BootstrapOutcome::Activated {
            generation: 1,
            track_count: 505,
            resumed_rows: 0,
        }
    ));
    assert!(!BootstrapLayout::new(temp.path()).building_path(2).exists());
}

#[test]
fn plex_album_metadata_art_fills_missing_track_art_and_reconciles() {
    let temp = tempdir().unwrap();
    let path = temp.path().join("plex_cache.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE plex_cache_tracks (
             rating_key TEXT PRIMARY KEY, server_id TEXT, title TEXT NOT NULL,
             artist TEXT, album TEXT, duration_ms INTEGER, container TEXT,
             bit_depth INTEGER, sampling_rate_hz INTEGER, artwork_path TEXT,
             collection_artwork_path TEXT, updated_at INTEGER, year INTEGER,
             disc_number INTEGER, track_number INTEGER, parent_rating_key TEXT
         );
         CREATE TABLE plex_cache_album_artwork (
             server_id TEXT NOT NULL, parent_rating_key TEXT NOT NULL,
             artwork_path TEXT, checked_at INTEGER NOT NULL,
             PRIMARY KEY(server_id,parent_rating_key)
         );
         INSERT INTO plex_cache_tracks VALUES
             ('track-1','plex-server','Song','Rammstein','1994',180000,'flac',
              16,44100,NULL,NULL,10,2002,1,1,'60335');
         INSERT INTO plex_cache_album_artwork VALUES
             ('plex-server','60335','/library/metadata/60320/thumb/1',20);",
    )
    .unwrap();
    drop(conn);

    assert!(matches!(
        bootstrap_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap(),
        BootstrapOutcome::Activated { track_count: 1, .. }
    ));
    let track_ref = TrackRef {
        source: SourceKind::Plex,
        source_instance: "plex-server".to_string(),
        native_id: "track-1".to_string(),
    };
    let layout = BootstrapLayout::new(temp.path());
    let ActiveCatalog::Ready { catalog, .. } = layout.open_active() else {
        panic!("Plex artwork fixture did not activate")
    };
    assert_eq!(
        catalog
            .resolve(&track_ref)
            .unwrap()
            .unwrap()
            .artwork_token
            .as_deref(),
        Some("/library/metadata/60320/thumb/1")
    );
    drop(catalog);

    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE plex_cache_album_artwork
                SET artwork_path='/library/metadata/60320/thumb/2',checked_at=21
              WHERE server_id='plex-server' AND parent_rating_key='60335'",
            [],
        )
        .unwrap();
    assert!(matches!(
        crate::reconcile_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap(),
        ProjectionOutcome::Activated {
            changed_sources: 1,
            ..
        }
    ));
    let ActiveCatalog::Ready { catalog, .. } = layout.open_active() else {
        panic!("Plex artwork refresh did not activate")
    };
    assert_eq!(
        catalog
            .resolve(&track_ref)
            .unwrap()
            .unwrap()
            .artwork_token
            .as_deref(),
        Some("/library/metadata/60320/thumb/2")
    );
}

#[test]
fn one_source_bootstrap_reports_identical_source_and_overall_progress() {
    let temp = tempdir().unwrap();
    create_local_fixture(&temp.path().join("library.db"));
    let mut progress = Vec::new();

    let outcome = bootstrap_legacy_caches_at_with_progress(
        &LegacyLocations::co_located(temp.path()),
        &AtomicBool::new(false),
        |event| progress.push(event.clone()),
    )
    .unwrap();

    assert!(matches!(outcome, BootstrapOutcome::Activated { .. }));
    assert!(!progress.is_empty());
    assert!(progress.iter().all(|event| {
        event.source_index == 1
            && event.source_count == 1
            && event.source_rows_total == event.overall_rows_total
            && event.committed_rows == event.overall_rows_written
    }));
    let final_event = progress.last().unwrap();
    assert!(final_event.source_complete);
    assert_eq!(final_event.source_rows_total, 2);
    assert_eq!(final_event.overall_rows_total, 2);
}

/// Explicit recovery-cost gate using the production Rust legacy reader,
/// projector, materialized views, FTS activation check and side-by-side rename.
/// Ordinary unit runs skip it because the 200k case intentionally creates a
/// large temporary catalog and measures wall time rather than correctness only.
#[test]
#[ignore = "explicit 20k/200k end-to-end legacy recovery metric"]
fn legacy_recovery_cost_uses_the_real_projector_at_scale() {
    for rows in [20_000_u64, 200_000] {
        let temp = tempdir().unwrap();
        let source_path = temp.path().join("plex_cache.db");
        create_plex_scale_fixture(&source_path, rows);
        let source_bytes = fs::metadata(&source_path).unwrap().len();
        let source_before = fs::read(&source_path).unwrap();

        let started = Instant::now();
        let outcome = bootstrap_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap();
        let elapsed = started.elapsed();
        assert!(matches!(
            outcome,
            BootstrapOutcome::Activated {
                generation: 1,
                track_count,
                ..
            } if track_count == rows
        ));
        let layout = BootstrapLayout::new(temp.path());
        let catalog_bytes = fs::metadata(layout.generation_path(1)).unwrap().len();
        let ActiveCatalog::Ready { catalog, .. } = layout.open_active() else {
            panic!("scaled recovery fixture did not activate")
        };
        let runtime = catalog.runtime_integrity_check().unwrap();
        assert!(runtime.sqlite_ok && runtime.materialized_views_ok);
        assert_eq!(runtime.foreign_key_violations, 0);
        assert_eq!(fs::read(&source_path).unwrap(), source_before);

        let source = Connection::open(&source_path).unwrap();
        source
            .execute(
                "UPDATE plex_cache_tracks
                    SET title='Changed Track', updated_at=?1
                  WHERE rating_key='1'",
                [rows as i64 + 1],
            )
            .unwrap();
        drop(source);
        let changed_source = fs::read(&source_path).unwrap();
        let reconcile_started = Instant::now();
        let projection =
            crate::reconcile_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap();
        let reconcile_elapsed = reconcile_started.elapsed();
        assert!(matches!(
            projection,
            ProjectionOutcome::Activated {
                generation: 2,
                track_count,
                changed_sources: 1,
                ..
            } if track_count == rows
        ));
        assert_eq!(fs::read(&source_path).unwrap(), changed_source);
        println!(
            "RECOVERY_METRIC rows={rows} bootstrap_ms={:.3} reconcile_ms={:.3} source_bytes={source_bytes} catalog_bytes={catalog_bytes}",
            elapsed.as_secs_f64() * 1_000.0,
            reconcile_elapsed.as_secs_f64() * 1_000.0,
        );
    }
}

#[test]
fn obsolete_catalog_schema_rebuilds_side_by_side_without_touching_sources() {
    let temp = tempdir().unwrap();
    let source_path = temp.path().join("library.db");
    create_local_fixture(&source_path);
    let source_bytes = fs::metadata(&source_path).unwrap().len();
    let layout = BootstrapLayout::new(temp.path());
    let obsolete_generation = 4;
    let obsolete_bytes = b"obsolete derived catalog";
    fs::write(layout.generation_path(obsolete_generation), obsolete_bytes).unwrap();
    layout
        .write_manifest(&BootstrapManifest {
            manifest_version: 1,
            schema_version: SCHEMA_VERSION - 1,
            active_generation: obsolete_generation,
            previous_generation: None,
            activated_at_unix_ms: 1,
        })
        .unwrap();

    let outcome = bootstrap_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap();
    assert!(matches!(
        outcome,
        BootstrapOutcome::Activated {
            generation: 5,
            track_count: 2,
            ..
        }
    ));
    assert_eq!(fs::read(layout.generation_path(4)).unwrap(), obsolete_bytes);
    assert_eq!(fs::metadata(source_path).unwrap().len(), source_bytes);
    let ActiveCatalog::Ready { catalog, manifest } = layout.open_active() else {
        panic!("rebuilt catalog did not activate")
    };
    assert_eq!(manifest.schema_version, SCHEMA_VERSION);
    assert_eq!(catalog.stats().unwrap().track_count, 2);
}

#[test]
fn split_profile_and_global_cache_locations_build_one_user_catalog() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("qbz");
    let user = root.join("users/42");
    fs::create_dir_all(&user).unwrap();
    create_local_fixture(&user.join("library.db"));
    create_plex_fixture(&root.join("plex_cache.db"));
    create_remote_fixture(&user.join("remote_cache.db"));
    let locations = LegacyLocations {
        catalog_dir: user.clone(),
        local_database: user.join("library.db"),
        plex_database: root.join("plex_cache.db"),
        remote_database: user.join("remote_cache.db"),
        enabled_sources: SourceKind::ALL.into_iter().collect(),
    };

    let outcome =
        bootstrap_legacy_caches_at_with_progress(&locations, &AtomicBool::new(false), |_| {})
            .unwrap();
    assert!(matches!(
        outcome,
        BootstrapOutcome::Activated {
            track_count: 505,
            ..
        }
    ));
    let ActiveCatalog::Ready { catalog, .. } = BootstrapLayout::new(&user).open_active() else {
        panic!("profile catalog did not activate")
    };
    assert_eq!(catalog.stats().unwrap().track_count, 505);
    assert!(!root.join("local_catalog-v1-manifest.json").exists());
}

#[test]
fn disabled_source_is_not_opened_and_is_pruned_from_an_existing_catalog() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("qbz");
    let user = root.join("users/42");
    fs::create_dir_all(&user).unwrap();
    create_local_fixture(&user.join("library.db"));
    create_plex_fixture(&root.join("plex_cache.db"));
    create_remote_fixture(&user.join("remote_cache.db"));
    let mut locations = LegacyLocations {
        catalog_dir: user.clone(),
        local_database: user.join("library.db"),
        plex_database: root.join("plex_cache.db"),
        remote_database: user.join("remote_cache.db"),
        enabled_sources: SourceKind::ALL.into_iter().collect(),
    };
    bootstrap_legacy_caches_at_with_progress(&locations, &AtomicBool::new(false), |_| {})
        .unwrap();

    locations.enabled_sources.remove(&SourceKind::Plex);
    let discovered = crate::discover_legacy_sources_at(&locations).unwrap();
    assert!(discovered
        .iter()
        .all(|source| source.source.source != SourceKind::Plex));
    assert!(matches!(
        crate::reconcile_legacy_caches_at_with_progress(
            &locations,
            &AtomicBool::new(false),
            |_| {}
        )
        .unwrap(),
        ProjectionOutcome::Activated {
            track_count: 4,
            changed_sources: 1,
            ..
        }
    ));
    let ActiveCatalog::Ready { catalog, .. } = BootstrapLayout::new(&user).open_active() else {
        panic!("profile catalog did not activate")
    };
    assert!(catalog
        .stats()
        .unwrap()
        .source_counts
        .iter()
        .all(|(source, _)| source.source != SourceKind::Plex));
    drop(catalog);

    locations.enabled_sources.insert(SourceKind::Plex);
    assert!(matches!(
        crate::reconcile_legacy_caches_at_with_progress(
            &locations,
            &AtomicBool::new(false),
            |_| {}
        )
        .unwrap(),
        ProjectionOutcome::Activated {
            track_count: 505,
            changed_sources: 1,
            ..
        }
    ));
}

fn create_local_fixture(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE local_tracks (
             id INTEGER PRIMARY KEY, file_path TEXT NOT NULL, title TEXT NOT NULL,
             artist TEXT NOT NULL, album_artist TEXT, album TEXT NOT NULL,
             duration_secs INTEGER, year INTEGER, disc_number INTEGER,
             track_number INTEGER, format TEXT, bit_depth INTEGER,
             sample_rate REAL, artwork_path TEXT, indexed_at INTEGER
         );
         INSERT INTO local_tracks VALUES
             (1,'/music/a.flac','Local A','Ártist','Ártist','Local Album',180,2020,1,1,'flac',24,96000,'/art/a',10),
             (2,'/music/b.mp3','Local B','Artist','Artist','Local Album',200,2020,1,2,'mp3',NULL,44100,NULL,11);",
    )
    .unwrap();
}

fn create_plex_fixture(path: &std::path::Path) {
    let mut conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE plex_cache_tracks (
             rating_key TEXT PRIMARY KEY, server_id TEXT, title TEXT NOT NULL,
             artist TEXT, album TEXT, duration_ms INTEGER, codec TEXT,
             container TEXT, bit_depth INTEGER, sampling_rate_hz INTEGER,
             artwork_path TEXT, updated_at INTEGER, year INTEGER,
             disc_number INTEGER, track_number INTEGER
         );",
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO plex_cache_tracks VALUES
                 (?1,'plex-server',?2,'Plex Artist','Plex Album',210000,'flac','flac',24,96000,'plex-art',20,2021,1,?3)",
            )
            .unwrap();
        for id in 1..=501 {
            insert
                .execute(rusqlite::params![
                    id.to_string(),
                    format!("Plex {id:04}"),
                    id
                ])
                .unwrap();
        }
    }
    tx.commit().unwrap();
}

fn create_plex_scale_fixture(path: &std::path::Path, rows: u64) {
    let mut conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=OFF;
         PRAGMA synchronous=OFF;
         CREATE TABLE plex_cache_tracks (
             rating_key TEXT PRIMARY KEY, server_id TEXT, title TEXT NOT NULL,
             artist TEXT, album TEXT, duration_ms INTEGER, codec TEXT,
             container TEXT, bit_depth INTEGER, sampling_rate_hz INTEGER,
             artwork_path TEXT, updated_at INTEGER, year INTEGER,
             disc_number INTEGER, track_number INTEGER
         );",
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO plex_cache_tracks VALUES
                 (?1,'scale-server',?2,?3,?4,210000,'flac','flac',24,96000,?5,?6,?7,?8,?9)",
            )
            .unwrap();
        for id in 1..=rows {
            insert
                .execute(rusqlite::params![
                    id.to_string(),
                    format!("Track {id:06}"),
                    format!("Artist {:05}", id % 10_000),
                    format!("Album {:05}", id / 12),
                    format!("art-{:05}", id / 12),
                    id as i64,
                    1980 + (id % 44) as i64,
                    1_i64,
                    (id % 12 + 1) as i64,
                ])
                .unwrap();
        }
    }
    tx.commit().unwrap();
}

fn create_remote_fixture(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE remote_cache_tracks (
             id INTEGER PRIMARY KEY, source TEXT NOT NULL, item_id TEXT NOT NULL,
             server_id TEXT, title TEXT, artist TEXT, album_artist TEXT,
             album TEXT, duration_ms INTEGER, codec TEXT, container TEXT,
             bit_depth INTEGER, sample_rate_hz INTEGER, artwork_token TEXT,
             updated_at INTEGER, year INTEGER, disc_number INTEGER,
             track_number INTEGER
         );
         INSERT INTO remote_cache_tracks VALUES
             (1,'jellyfin','same','jf-server','JF Track','JF Artist','JF Album Artist','JF Album',220000,'flac','flac',24,96000,'jf-art',30,2022,1,1),
             (2,'subsonic','same','sub-server','Sub Track','Sub Artist','Sub Album Artist','Sub Album',230000,'mp3','mp3',NULL,44100,'sub-art',31,2023,1,1);",
    )
    .unwrap();
}
