use std::collections::HashSet;
use std::fs;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use rusqlite::Connection;
use tempfile::tempdir;

use crate::{
    bootstrap_legacy_caches, reconcile_legacy_caches, ActiveCatalog, BootstrapBatch,
    BootstrapLayout, BootstrapOutcome, ProjectedTrack, ProjectionOutcome, QueryDescriptor,
    ReconciliationBatch, SourceKey, SourceKind, SourceProbe, TrackRef, BOOTSTRAP_BATCH_ROWS,
};

fn key(kind: SourceKind, instance: &str) -> SourceKey {
    SourceKey {
        source: kind,
        source_instance: instance.to_string(),
    }
}

fn probe(source: &SourceKey, version: &str, rows: u64) -> SourceProbe {
    SourceProbe {
        source: source.clone(),
        source_path: std::path::PathBuf::from(format!("{}.db", source.source.as_str())),
        snapshot_version: version.to_string(),
        row_count: rows,
        page_bytes: rows * 512,
        integrity_ok: true,
    }
}

fn row(source: &SourceKey, id: u64, title: impl Into<String>) -> ProjectedTrack {
    ProjectedTrack {
        track_ref: TrackRef {
            source: source.source,
            source_instance: source.source_instance.clone(),
            native_id: id.to_string(),
        },
        source_raw: source.source.as_str().to_string(),
        local_track_id: (source.source == SourceKind::Local).then_some(id as i64),
        local_path: (source.source == SourceKind::Local).then(|| format!("/music/{id}.flac")),
        native_album_id: Some(format!("album-{}", id / 10)),
        source_copy_id: None,
        title: title.into(),
        artist: format!("Artist {}", id % 17),
        album_artist: format!("Artist {}", id % 17),
        album: format!("Album {}", id / 10),
        duration_ms: 200_000 + id,
        year: Some(2020),
        disc_number: Some(1),
        track_number: Some((id % 20 + 1) as u32),
        format: "flac".to_string(),
        bit_depth: Some(24),
        sample_rate_hz: Some(96_000),
        artwork_token: None,
        isrc: None,
        musicbrainz_recording_id: None,
        added_at: id as i64,
        available: true,
        observed_generation: 0,
        credits: Vec::new(),
    }
}

fn bootstrap_source(
    session: &mut crate::BootstrapSession,
    source: &SourceKey,
    version: &str,
    rows: &[ProjectedTrack],
) {
    let mut cursor = String::new();
    for (batch_index, chunk) in rows.chunks(BOOTSTRAP_BATCH_ROWS).enumerate() {
        let end = (batch_index * BOOTSTRAP_BATCH_ROWS + chunk.len()).to_string();
        let complete = batch_index * BOOTSTRAP_BATCH_ROWS + chunk.len() == rows.len();
        let saved = session
            .apply_batch(&BootstrapBatch {
                source: source.clone(),
                snapshot_version: version.to_string(),
                expected_cursor: cursor,
                next_cursor: end,
                tracks: chunk.to_vec(),
                complete,
            })
            .unwrap();
        cursor = saved.checkpoint_cursor;
    }
}

fn reconcile_range(
    session: &mut crate::ProjectionSession,
    source: &SourceKey,
    version: &str,
    rows: &[ProjectedTrack],
    start: usize,
    stop: usize,
) {
    let mut cursor = if start == 0 {
        String::new()
    } else {
        start.to_string()
    };
    let mut offset = start;
    while offset < stop {
        let end = (offset + BOOTSTRAP_BATCH_ROWS).min(stop);
        let saved = session
            .apply_batch(&ReconciliationBatch {
                source: source.clone(),
                snapshot_version: version.to_string(),
                expected_cursor: cursor,
                next_cursor: end.to_string(),
                tracks: rows[offset..end].to_vec(),
                complete: end == rows.len(),
            })
            .unwrap();
        cursor = saved.checkpoint_cursor;
        offset = end;
    }
}

#[test]
fn full_source_reconciliation_resumes_and_prunes_only_on_completion() {
    let temp = tempdir().unwrap();
    let layout = BootstrapLayout::new(temp.path());
    let plex = key(SourceKind::Plex, "plex-a");
    let jellyfin = key(SourceKind::Jellyfin, "jf-a");
    let original_plex = (1..=600)
        .map(|id| row(&plex, id, format!("Old {id}")))
        .collect::<Vec<_>>();
    let original_jellyfin = (1..=50)
        .map(|id| row(&jellyfin, id, format!("Jellyfin {id}")))
        .collect::<Vec<_>>();
    let initial_probes = [probe(&plex, "plex-v1", 600), probe(&jellyfin, "jf-v1", 50)];
    let (mut bootstrap, _) = layout.prepare(&initial_probes, Some(u64::MAX)).unwrap();
    bootstrap_source(&mut bootstrap, &plex, "plex-v1", &original_plex);
    bootstrap_source(&mut bootstrap, &jellyfin, "jf-v1", &original_jellyfin);
    bootstrap.activate(&initial_probes).unwrap();

    let mut current_plex = (101..=600)
        .map(|id| {
            row(
                &plex,
                id,
                if id == 200 {
                    "Updated 200".to_string()
                } else {
                    format!("Old {id}")
                },
            )
        })
        .collect::<Vec<_>>();
    current_plex.extend((1_000..=1_100).map(|id| row(&plex, id, format!("New {id}"))));
    let current_probe = probe(&plex, "plex-v2", current_plex.len() as u64);
    let all_probes = [current_probe.clone(), initial_probes[1].clone()];
    let projection_started = Instant::now();
    let (mut interrupted, _) = layout
        .prepare_projection(&all_probes, Some(u64::MAX))
        .unwrap();
    interrupted.begin_source(&plex, "plex-v2").unwrap();
    reconcile_range(&mut interrupted, &plex, "plex-v2", &current_plex, 0, 250);
    assert_eq!(interrupted.source_watermark(&plex).unwrap().unwrap(), "");

    // The active generation remains wholly readable and unpruned while g2 is
    // incomplete, even with a reader held across the side-by-side write.
    let ActiveCatalog::Ready {
        catalog: old_reader,
        ..
    } = layout.open_active()
    else {
        panic!("generation one must remain active")
    };
    assert_eq!(old_reader.stats().unwrap().track_count, 650);
    assert!(old_reader
        .resolve(&TrackRef {
            source: SourceKind::Plex,
            source_instance: "plex-a".to_string(),
            native_id: "1".to_string(),
        })
        .unwrap()
        .is_some());
    drop(interrupted);

    let (mut resumed, _) = layout
        .prepare_projection(&all_probes, Some(u64::MAX))
        .unwrap();
    let checkpoint = resumed.checkpoint(&plex).unwrap().unwrap();
    assert_eq!(checkpoint.checkpoint_rows, 250);
    reconcile_range(
        &mut resumed,
        &plex,
        "plex-v2",
        &current_plex,
        250,
        current_plex.len(),
    );
    assert_eq!(resumed.source_watermark(&plex).unwrap().unwrap(), "plex-v2");
    let manifest = resumed.activate(&[current_probe]).unwrap();
    assert_eq!(manifest.active_generation, 2);
    assert_eq!(manifest.previous_generation, Some(1));
    assert!(layout.generation_path(1).is_file());

    let ActiveCatalog::Ready { catalog, .. } = layout.open_active() else {
        panic!("generation two must be active")
    };
    assert_eq!(catalog.stats().unwrap().track_count, 651);
    assert!(catalog
        .resolve(&TrackRef {
            source: SourceKind::Plex,
            source_instance: "plex-a".to_string(),
            native_id: "1".to_string(),
        })
        .unwrap()
        .is_none());
    assert_eq!(
        catalog
            .resolve(&TrackRef {
                source: SourceKind::Plex,
                source_instance: "plex-a".to_string(),
                native_id: "200".to_string(),
            })
            .unwrap()
            .unwrap()
            .title,
        "Updated 200"
    );
    assert_eq!(
        catalog.count_tracks(&QueryDescriptor::tracks()).unwrap(),
        651
    );
    let ids = catalog
        .query_tracks(&QueryDescriptor::tracks(), None, 500)
        .unwrap()
        .rows
        .into_iter()
        .map(|track| track.track_ref)
        .collect::<HashSet<_>>();
    assert_eq!(ids.len(), 500);
    println!(
        "D_PROJECTION_METRIC observed_rows={} pruned_rows={} retained_other_source_rows={} elapsed_ms={:.3} catalog_bytes={}",
        current_plex.len(),
        100,
        original_jellyfin.len(),
        projection_started.elapsed().as_secs_f64() * 1_000.0,
        fs::metadata(layout.generation_path(2)).unwrap().len()
    );
    drop(old_reader);
}

#[test]
fn projection_preflight_accounts_for_copying_the_active_generation() {
    let temp = tempdir().unwrap();
    let layout = BootstrapLayout::new(temp.path());
    let local = key(SourceKind::Local, "library");
    let initial_probe = probe(&local, "v1", 0);
    let (mut bootstrap, _) = layout
        .prepare(&[initial_probe.clone()], Some(u64::MAX))
        .unwrap();
    bootstrap
        .apply_batch(&BootstrapBatch {
            source: local,
            snapshot_version: "v1".to_string(),
            expected_cursor: String::new(),
            next_cursor: String::new(),
            tracks: Vec::new(),
            complete: true,
        })
        .unwrap();
    bootstrap.activate(&[initial_probe.clone()]).unwrap();

    let error = match layout.prepare_projection(&[initial_probe], Some(256 * 1024 * 1024)) {
        Ok(_) => panic!("copying an active catalog needs room above the fixed floor"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        crate::CatalogError::InsufficientSpace { .. }
    ));
    assert!(!layout.building_path(2).exists());
}

#[test]
fn legacy_catch_up_detects_authoritative_updates_and_then_is_a_no_op() {
    let temp = tempdir().unwrap();
    let path = temp.path().join("library.db");
    create_local(&path);
    assert!(matches!(
        bootstrap_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap(),
        BootstrapOutcome::Activated {
            generation: 1,
            track_count: 3,
            ..
        }
    ));
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute("UPDATE local_tracks SET title='Changed' WHERE id=2", [])
            .unwrap();
        conn.execute("DELETE FROM local_tracks WHERE id=1", [])
            .unwrap();
        conn.execute(
            "INSERT INTO local_tracks VALUES
             (4,'/music/d.flac','Added','Artist','Artist','Album',180,2024,1,4,
              'flac',24,96000,NULL,40,'album')",
            [],
        )
        .unwrap();
    }
    let outcome = reconcile_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap();
    assert!(matches!(
        outcome,
        ProjectionOutcome::Activated {
            generation: 2,
            track_count: 3,
            changed_sources: 1,
            ..
        }
    ));
    let layout = BootstrapLayout::new(temp.path());
    let ActiveCatalog::Ready { catalog, .. } = layout.open_active() else {
        panic!("catch-up did not activate")
    };
    let local_ref = |id: &str| TrackRef {
        source: SourceKind::Local,
        source_instance: "library".to_string(),
        native_id: id.to_string(),
    };
    assert!(catalog.resolve(&local_ref("1")).unwrap().is_none());
    assert_eq!(
        catalog.resolve(&local_ref("2")).unwrap().unwrap().title,
        "Changed"
    );
    assert!(catalog.resolve(&local_ref("4")).unwrap().is_some());
    drop(catalog);

    // A metadata-only edit keeps count, max id and max indexed_at unchanged;
    // the SQLite file epoch still makes it visible to catch-up.
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE local_tracks SET title='Metadata only' WHERE id=3",
            [],
        )
        .unwrap();
    assert!(matches!(
        reconcile_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap(),
        ProjectionOutcome::Activated {
            generation: 3,
            track_count: 3,
            changed_sources: 1,
            ..
        }
    ));
    let ActiveCatalog::Ready { catalog, .. } = layout.open_active() else {
        panic!("metadata catch-up did not activate")
    };
    assert_eq!(
        catalog.resolve(&local_ref("3")).unwrap().unwrap().title,
        "Metadata only"
    );
    drop(catalog);

    assert!(matches!(
        reconcile_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap(),
        ProjectionOutcome::UpToDate {
            generation: 3,
            track_count: 3,
        }
    ));
    assert!(!layout.building_path(4).exists());

    fs::write(&path, b"corrupt authoritative fixture").unwrap();
    assert!(reconcile_legacy_caches(temp.path(), &AtomicBool::new(false)).is_err());
    let ActiveCatalog::Ready { catalog, manifest } = layout.open_active() else {
        panic!("source failure must preserve the last complete generation")
    };
    assert_eq!(manifest.active_generation, 3);
    assert_eq!(catalog.stats().unwrap().track_count, 3);
}

#[test]
fn remote_metadata_sidecar_overlays_bootstrap_and_triggers_catch_up() {
    let temp = tempdir().unwrap();
    let remote_path = temp.path().join("remote_cache.db");
    Connection::open(&remote_path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE remote_cache_tracks (
                 id INTEGER PRIMARY KEY, source TEXT NOT NULL, item_id TEXT NOT NULL,
                 server_id TEXT, title TEXT, artist TEXT, album_artist TEXT,
                 album TEXT, duration_ms INTEGER, codec TEXT, container TEXT,
                 bit_depth INTEGER, sample_rate_hz INTEGER, artwork_token TEXT,
                 updated_at INTEGER, year INTEGER, disc_number INTEGER,
                 track_number INTEGER, album_id TEXT
             );
             INSERT INTO remote_cache_tracks VALUES
                 (1,'jellyfin','track-1','server-a','Original title','Original artist',
                  'Original album artist','Original album',220000,'flac','flac',24,
                  96000,'remote-art',30,2022,1,1,'album-9');",
        )
        .unwrap();

    let sidecar_path = temp.path().join("metadata_sidecars.db");
    let sidecar = Connection::open(&sidecar_path).unwrap();
    sidecar
        .execute_batch(
            "CREATE TABLE remote_album_tag_sidecars (
                 source TEXT NOT NULL, source_instance TEXT NOT NULL,
                 album_id TEXT NOT NULL, payload_json TEXT NOT NULL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY(source,source_instance,album_id)
             );",
        )
        .unwrap();
    let payload = serde_json::json!({
        "version": 2,
        "updatedAt": 100,
        "album": {
            "albumTitle": "Edited album",
            "albumArtist": "Edited album artist",
            "year": 2025,
            "genre": "Progressive rock",
            "catalogNumber": "CAT-9"
        },
        "tracks": [{
            "filePath": "track-1",
            "cueStartSecs": null,
            "title": "Edited title",
            "discNumber": 2,
            "trackNumber": 7
        }],
        "extendedAlbum": {
            "albumArtists": ["Edited album artist"],
            "compilation": false,
            "musicbrainzReleaseId": "release-9",
            "musicbrainzReleaseGroupId": "group-9",
            "musicbrainzAlbumArtistIds": ["artist-9"],
            "discogsReleaseId": "99",
            "artworkPath": "/cache/edited-cover.jpg"
        },
        "extendedTracks": [{
            "filePath": "track-1",
            "cueStartSecs": null,
            "artistCredit": "Edited track artist",
            "artists": ["Edited track artist"],
            "composers": ["Composer"],
            "performers": ["Player (guitar)"],
            "musicbrainzRecordingId": "recording-1",
            "musicbrainzTrackId": "mb-track-1",
            "musicbrainzArtistIds": ["artist-1"]
        }]
    })
    .to_string();
    sidecar
        .execute(
            "INSERT INTO remote_album_tag_sidecars VALUES
             ('jellyfin','server-a','album-9',?1,100)",
            [&payload],
        )
        .unwrap();
    drop(sidecar);

    assert!(matches!(
        bootstrap_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap(),
        BootstrapOutcome::Activated {
            generation: 1,
            track_count: 1,
            ..
        }
    ));
    let layout = BootstrapLayout::new(temp.path());
    let track_ref = TrackRef {
        source: SourceKind::Jellyfin,
        source_instance: "server-a".to_string(),
        native_id: "track-1".to_string(),
    };
    let ActiveCatalog::Ready { catalog, .. } = layout.open_active() else {
        panic!("sidecar bootstrap did not activate")
    };
    let projected = catalog.resolve(&track_ref).unwrap().unwrap();
    assert_eq!(projected.title, "Edited title");
    assert_eq!(projected.artist, "Edited track artist");
    assert_eq!(projected.album, "Edited album");
    assert_eq!(projected.album_artist, "Edited album artist");
    assert_eq!(projected.year, Some(2025));
    assert_eq!(projected.disc_number, Some(2));
    assert_eq!(projected.track_number, Some(7));
    assert_eq!(
        projected.artwork_token.as_deref(),
        Some("qbz-local-art:/cache/edited-cover.jpg")
    );
    drop(catalog);

    let changed = payload.replace("Edited title", "Changed from sidecar");
    Connection::open(&sidecar_path)
        .unwrap()
        .execute(
            "UPDATE remote_album_tag_sidecars
                SET payload_json=?1,updated_at=101
              WHERE source='jellyfin' AND source_instance='server-a'
                AND album_id='album-9'",
            [&changed],
        )
        .unwrap();
    assert!(matches!(
        reconcile_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap(),
        ProjectionOutcome::Activated {
            generation: 2,
            track_count: 1,
            changed_sources: 1,
            ..
        }
    ));
    let ActiveCatalog::Ready { catalog, .. } = layout.open_active() else {
        panic!("sidecar catch-up did not activate")
    };
    assert_eq!(
        catalog.resolve(&track_ref).unwrap().unwrap().title,
        "Changed from sidecar"
    );
}

fn create_local(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE local_tracks (
             id INTEGER PRIMARY KEY,file_path TEXT NOT NULL,title TEXT NOT NULL,
             artist TEXT NOT NULL,album_artist TEXT,album TEXT NOT NULL,
             duration_secs INTEGER,year INTEGER,disc_number INTEGER,
             track_number INTEGER,format TEXT,bit_depth INTEGER,sample_rate REAL,
             artwork_path TEXT,indexed_at INTEGER,album_group_key TEXT
         );
         INSERT INTO local_tracks VALUES
             (1,'/music/a.flac','A','Artist','Artist','Album',180,2024,1,1,'flac',24,96000,NULL,10,'album'),
             (2,'/music/b.flac','B','Artist','Artist','Album',180,2024,1,2,'flac',24,96000,NULL,20,'album'),
             (3,'/music/c.flac','C','Artist','Artist','Album',180,2024,1,3,'flac',24,96000,NULL,30,'album');",
    )
    .unwrap();
}

#[test]
fn sacd_virtual_tracks_project_as_stable_local_rows() {
    let temp = tempdir().unwrap();
    let path = temp.path().join("library.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE local_tracks (
             id INTEGER PRIMARY KEY,file_path TEXT NOT NULL,title TEXT NOT NULL,
             artist TEXT NOT NULL,album_artist TEXT,album TEXT NOT NULL,
             duration_secs INTEGER,year INTEGER,disc_number INTEGER,
             track_number INTEGER,format TEXT,bit_depth INTEGER,sample_rate REAL,
             artwork_path TEXT,indexed_at INTEGER,album_group_key TEXT,source TEXT
         );
         INSERT INTO local_tracks VALUES
             (41,'sacd:/nas/disc.iso#1','Movement I','Composer','Orchestra',
              'Symphony',720,NULL,1,1,'DSD',1,2822400,'/art/sacd',100,
              'sacd|||sacd-fingerprint','user'),
             (42,'sacd:/nas/disc.iso#2','Movement II','Composer','Orchestra',
              'Symphony',680,NULL,1,2,'DSD',1,2822400,'/art/sacd',100,
              'sacd|||sacd-fingerprint','user');",
    )
    .unwrap();
    drop(conn);

    assert!(matches!(
        bootstrap_legacy_caches(temp.path(), &AtomicBool::new(false)).unwrap(),
        BootstrapOutcome::Activated { track_count: 2, .. }
    ));
    let ActiveCatalog::Ready { catalog, .. } = BootstrapLayout::new(temp.path()).open_active()
    else {
        panic!("SACD fixture did not activate")
    };
    let projected = catalog
        .resolve(&TrackRef {
            source: SourceKind::Local,
            source_instance: "library".to_string(),
            native_id: "41".to_string(),
        })
        .unwrap()
        .unwrap();
    assert_eq!(projected.local_track_id, Some(41));
    assert_eq!(
        projected.local_path.as_deref(),
        Some("sacd:/nas/disc.iso#1")
    );
    assert_eq!(
        projected.native_album_id.as_deref(),
        Some("sacd|||sacd-fingerprint")
    );
    assert_eq!(projected.source_raw, "user");
    assert_eq!(projected.format, "dsd");
    assert_eq!(projected.sample_rate_hz, Some(2_822_400));
}
