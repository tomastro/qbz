use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::*;

fn source_for(index: usize) -> SourceKind {
    match index % 100 {
        0..=59 => SourceKind::Local,
        60..=79 => SourceKind::Plex,
        80..=89 => SourceKind::Jellyfin,
        90..=97 => SourceKind::Subsonic,
        _ => SourceKind::Offline,
    }
}

fn source_instance(source: SourceKind) -> &'static str {
    match source {
        SourceKind::Local => "local-main",
        SourceKind::Offline => "qobuz-user-7",
        SourceKind::Plex => "plex-server-a",
        SourceKind::Jellyfin => "jellyfin-server-b",
        SourceKind::Subsonic => "subsonic-server-c",
    }
}

fn projected(index: usize, source: SourceKind) -> ProjectedTrack {
    let artist_number = index % 50_000;
    let album_number = index % 80_000;
    let format = match index % 10 {
        0..=5 => "flac",
        6..=7 => "mp3",
        8 => "alac",
        _ => "dsf",
    };
    let artist = format!("Artist {artist_number:05}");
    ProjectedTrack {
        track_ref: TrackRef {
            source,
            source_instance: source_instance(source).to_string(),
            native_id: index.to_string(),
        },
        source_raw: source.as_str().to_string(),
        local_track_id: (source == SourceKind::Local).then_some(index as i64 + 1),
        local_path: (source == SourceKind::Local)
            .then(|| format!("/fixture/artist-{artist_number:05}/track-{index:07}.{format}")),
        native_album_id: Some(format!("album-{album_number:05}")),
        source_copy_id: None,
        title: format!("Track {index:07} Signal {:03}", index % 997),
        artist: artist.clone(),
        album_artist: artist.clone(),
        album: format!("Album {album_number:05}"),
        duration_ms: 120_000 + (index % 480_000) as u64,
        year: (index % 23 != 0).then_some(1950 + (index % 77) as u32),
        disc_number: Some((index % 3 + 1) as u32),
        track_number: Some((index % 24 + 1) as u32),
        format: format.to_string(),
        bit_depth: (format != "mp3").then_some(if index % 3 == 0 { 24 } else { 16 }),
        sample_rate_hz: Some(if index % 5 == 0 { 96_000 } else { 44_100 }),
        artwork_token: Some(format!("art-{:05}", album_number)),
        isrc: (index % 11 == 0).then(|| format!("ISRC{index:08}")),
        musicbrainz_recording_id: (index % 101 == 0).then(|| format!("mbid-{index:08}")),
        added_at: 1_700_000_000 + index as i64,
        available: index % 97 != 0,
        observed_generation: 1,
        credits: vec![ArtistCredit {
            display_name: artist,
            role: CreditRole::TrackArtist,
            ordinal: 0,
        }],
    }
}

fn insert_fixture(catalog: &mut Catalog, count: usize) {
    for start in (0..count).step_by(2_000) {
        let end = (start + 2_000).min(count);
        let batch = (start..end)
            .map(|index| projected(index, source_for(index)))
            .collect::<Vec<_>>();
        catalog.upsert_tracks(&batch).unwrap();
    }
}

fn collect_all(catalog: &Catalog, descriptor: &QueryDescriptor) -> Vec<TrackRecord> {
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let page = catalog
            .query_tracks(descriptor, cursor.as_ref(), 137)
            .unwrap();
        let done = !page.has_more;
        cursor = page.next_cursor;
        rows.extend(page.rows);
        if done {
            break;
        }
    }
    rows
}

fn collect_all_albums(
    catalog: &Catalog,
    descriptor: &QueryDescriptor,
    page_size: usize,
) -> Vec<AlbumRecord> {
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let page = catalog
            .query_albums(descriptor, cursor.as_ref(), page_size)
            .unwrap();
        rows.extend(page.rows);
        if !page.has_more {
            break;
        }
        cursor = page.next_cursor;
    }
    rows
}

fn collect_all_artists(
    catalog: &Catalog,
    descriptor: &QueryDescriptor,
    page_size: usize,
) -> Vec<ArtistRecord> {
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let page = catalog
            .query_artists(descriptor, cursor.as_ref(), page_size)
            .unwrap();
        rows.extend(page.rows);
        if !page.has_more {
            break;
        }
        cursor = page.next_cursor;
    }
    rows
}

fn insert_album_fixture(catalog: &mut Catalog, count: usize) {
    let mut by_source = std::collections::BTreeMap::<SourceKey, Vec<ProjectedTrack>>::new();
    for index in 0..count {
        let source = source_for(index);
        let track = projected(index, source);
        by_source
            .entry(SourceKey {
                source: track.track_ref.source,
                source_instance: track.track_ref.source_instance.clone(),
            })
            .or_default()
            .push(track);
    }
    for (source, rows) in by_source {
        let version = format!("fixture-{}", source.source.as_str());
        let mut cursor = String::new();
        let total = rows.len();
        for (batch_index, batch) in rows.chunks(BOOTSTRAP_BATCH_ROWS).enumerate() {
            let committed = (batch_index * BOOTSTRAP_BATCH_ROWS + batch.len()).min(total);
            let saved = catalog
                .apply_bootstrap_batch(&BootstrapBatch {
                    source: source.clone(),
                    snapshot_version: version.clone(),
                    expected_cursor: cursor,
                    next_cursor: committed.to_string(),
                    tracks: batch.to_vec(),
                    complete: committed == total,
                })
                .unwrap();
            cursor = saved.checkpoint_cursor;
        }
    }
    catalog.rebuild_materialized_views().unwrap();
}

#[test]
fn schema_is_versioned_frontend_agnostic_and_fts5_enabled() {
    let catalog = Catalog::open_in_memory(7).unwrap();
    let stats = catalog.stats().unwrap();
    assert_eq!(stats.schema_version, crate::SCHEMA_VERSION);
    assert_eq!(stats.generation, 7);
    assert_eq!(stats.track_count, 0);
    let fts5: i64 = catalog
        .connection()
        .query_row(
            "SELECT sqlite_compileoption_used('ENABLE_FTS5')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fts5, 1);
    let tables: HashSet<String> = catalog
        .connection()
        .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for required in [
        "source_state",
        "logical_albums",
        "editions",
        "source_copies",
        "tracks",
        "artist_credits",
        "artist_identity_credits",
        "albums_materialized",
        "artists_materialized",
        "artist_source_stats",
        "edition_artists",
        "tracks_fts",
        "albums_fts",
        "artists_fts",
    ] {
        assert!(tables.contains(required), "missing {required}");
    }
}

#[test]
fn artist_keyset_search_and_source_scope_cover_every_remote_only_artist_once() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    insert_album_fixture(&mut catalog, 1_231);

    let all = QueryDescriptor::artists();
    let rows = collect_all_artists(&catalog, &all, 17);
    let keys = rows
        .iter()
        .map(|row| row.artist_key.clone())
        .collect::<HashSet<_>>();
    let expected_available = (0..1_231).filter(|index| index % 97 != 0).count();
    assert_eq!(rows.len(), expected_available);
    assert_eq!(keys.len(), rows.len());
    assert_eq!(catalog.count_artists(&all).unwrap(), rows.len() as u64);
    assert_eq!(
        catalog.count_artist_entries(&all).unwrap(),
        rows.len() as u64 + 1
    );

    let jellyfin_source = SourceKey {
        source: SourceKind::Jellyfin,
        source_instance: source_instance(SourceKind::Jellyfin).to_string(),
    };
    let jellyfin = QueryDescriptor::artists().with_sources(vec![jellyfin_source.clone()]);
    let remote = collect_all_artists(&catalog, &jellyfin, 11);
    assert!(!remote.is_empty());
    assert!(remote.iter().all(|row| row.source == "jellyfin"));
    assert_eq!(
        catalog.count_artists(&jellyfin).unwrap(),
        remote.len() as u64
    );
    assert!(remote
        .iter()
        .all(|row| row.album_count == 1 && row.track_count == 1));

    let needle = remote[0]
        .display_name
        .chars()
        .skip(7)
        .take(3)
        .collect::<String>();
    let searched = collect_all_artists(
        &catalog,
        &QueryDescriptor::artists()
            .with_sources(vec![jellyfin_source])
            .with_search(needle),
        7,
    );
    assert!(!searched.is_empty());
    assert!(searched
        .iter()
        .all(|row| row.source == "jellyfin" && keys.contains(&row.artist_key)));
    let searched_descriptor = QueryDescriptor::artists()
        .with_sources(vec![SourceKey {
            source: SourceKind::Jellyfin,
            source_instance: source_instance(SourceKind::Jellyfin).to_string(),
        }])
        .with_search(
            remote[0]
                .display_name
                .chars()
                .skip(7)
                .take(3)
                .collect::<String>(),
        );
    assert_eq!(
        catalog.count_artists(&searched_descriptor).unwrap(),
        searched.len() as u64
    );
    assert_eq!(
        catalog.count_artist_entries(&searched_descriptor).unwrap(),
        searched.len() as u64 + 1
    );
}

#[test]
fn artist_credit_counts_and_album_relationship_span_every_source() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    let mut local = projected(10, SourceKind::Local);
    local.native_album_id = Some("local-edition".to_string());
    local.credits.push(ArtistCredit {
        display_name: "Beyoncé".to_string(),
        role: CreditRole::Featured,
        ordinal: 1,
    });
    let mut remote = projected(80, SourceKind::Jellyfin);
    remote.native_album_id = Some("remote-edition".to_string());
    remote.credits.push(ArtistCredit {
        display_name: "Beyonce".to_string(),
        role: CreditRole::Performer,
        ordinal: 1,
    });
    for track in [local, remote] {
        let source = SourceKey {
            source: track.track_ref.source,
            source_instance: track.track_ref.source_instance.clone(),
        };
        catalog
            .apply_bootstrap_batch(&BootstrapBatch {
                source,
                snapshot_version: "artist-credit-v1".to_string(),
                expected_cursor: String::new(),
                next_cursor: "1".to_string(),
                tracks: vec![track],
                complete: true,
            })
            .unwrap();
    }
    catalog.rebuild_materialized_views().unwrap();

    let key = normalize_artist_key("Beyoncé");
    let row = collect_all_artists(&catalog, &QueryDescriptor::artists(), 5)
        .into_iter()
        .find(|row| row.artist_key == key)
        .expect("credit-only artist is materialized");
    assert_eq!(row.album_count, 2);
    assert_eq!(row.track_count, 2);
    assert_eq!(row.source, "mixed");

    let sources = vec![
        SourceKey {
            source: SourceKind::Local,
            source_instance: source_instance(SourceKind::Local).to_string(),
        },
        SourceKey {
            source: SourceKind::Jellyfin,
            source_instance: source_instance(SourceKind::Jellyfin).to_string(),
        },
    ];
    assert_eq!(catalog.count_artist_albums(&key, &sources).unwrap(), 2);
    let page = catalog
        .query_artist_albums(&key, &sources, None, 1)
        .unwrap();
    assert_eq!(page.rows.len(), 1);
    assert!(page.has_more);
    let second = catalog
        .query_artist_albums(&key, &sources, page.next_cursor.as_ref(), 1)
        .unwrap();
    assert_eq!(second.rows.len(), 1);
    assert!(!second.has_more);
    assert_ne!(page.rows[0].edition_id, second.rows[0].edition_id);

    let jellyfin = &sources[1..];
    assert_eq!(catalog.count_artist_albums(&key, jellyfin).unwrap(), 1);
    let remote_row = collect_all_artists(
        &catalog,
        &QueryDescriptor::artists().with_sources(jellyfin.to_vec()),
        5,
    )
    .into_iter()
    .find(|row| row.artist_key == key)
    .unwrap();
    assert_eq!((remote_row.album_count, remote_row.track_count), (1, 1));
    assert_eq!(remote_row.source, "jellyfin");
}

#[test]
fn repeated_album_artist_families_collapse_without_losing_contributors() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    let family = "新世紀エヴァンゲリオン";
    let contributors = [
        ("鷺巣詩郎", "Shiro Sagisu"),
        ("林原めぐみ", "Megumi Hayashibara"),
        ("高橋洋子", "Yoko Takahashi"),
    ];
    let mut tracks = contributors
        .iter()
        .enumerate()
        .map(|(index, (artist, romanized))| {
            let mut track = projected(20_000 + index, SourceKind::Subsonic);
            track.native_album_id = Some(format!("eva-{index}"));
            track.artist = (*artist).to_string();
            track.album_artist = format!("{family} • {romanized}");
            track.credits = vec![
                ArtistCredit {
                    display_name: (*artist).to_string(),
                    role: CreditRole::TrackArtist,
                    ordinal: 0,
                },
                ArtistCredit {
                    display_name: track.album_artist.clone(),
                    role: CreditRole::AlbumArtist,
                    ordinal: 0,
                },
            ];
            track
        })
        .collect::<Vec<_>>();

    let mut collaboration = projected(20_010, SourceKind::Subsonic);
    collaboration.native_album_id = Some("one-off-collaboration".to_string());
    collaboration.artist = "Alice".to_string();
    collaboration.album_artist = "Alice • Bob".to_string();
    collaboration.credits = vec![
        ArtistCredit {
            display_name: "Alice".to_string(),
            role: CreditRole::TrackArtist,
            ordinal: 0,
        },
        ArtistCredit {
            display_name: collaboration.album_artist.clone(),
            role: CreditRole::AlbumArtist,
            ordinal: 0,
        },
    ];
    tracks.push(collaboration);

    let source = SourceKey {
        source: SourceKind::Subsonic,
        source_instance: source_instance(SourceKind::Subsonic).to_string(),
    };
    catalog
        .apply_bootstrap_batch(&BootstrapBatch {
            source,
            snapshot_version: "album-artist-families-v1".to_string(),
            expected_cursor: String::new(),
            next_cursor: tracks.len().to_string(),
            tracks,
            complete: true,
        })
        .unwrap();
    catalog.rebuild_materialized_views().unwrap();

    let raw_family_variants: i64 = catalog
        .connection()
        .query_row(
            "SELECT COUNT(DISTINCT display_name) FROM artist_credits
              WHERE role='album_artist' AND display_name LIKE ?1",
            [format!("{family} • %")],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw_family_variants, contributors.len() as i64);
    catalog.rebuild_materialized_views().unwrap();

    let artists = collect_all_artists(&catalog, &QueryDescriptor::artists(), 2);
    let keys = artists
        .iter()
        .map(|row| row.artist_key.as_str())
        .collect::<HashSet<_>>();
    let family_key = normalize_artist_key(family);
    let family_row = artists
        .iter()
        .find(|row| row.artist_key == family_key)
        .expect("the repeated album-artist family is one root artist");
    assert_eq!((family_row.album_count, family_row.track_count), (3, 3));
    for (artist, romanized) in contributors {
        assert!(keys.contains(normalize_artist_key(artist).as_str()));
        assert!(!keys.contains(normalize_artist_key(&format!("{family} • {romanized}")).as_str()));
    }
    assert!(keys.contains(normalize_artist_key("Alice • Bob").as_str()));
}

#[test]
fn artist_album_keysets_cross_pages_without_omissions_or_duplicates() {
    const ALBUMS: usize = 731;
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    let source = SourceKey {
        source: SourceKind::Jellyfin,
        source_instance: source_instance(SourceKind::Jellyfin).to_string(),
    };
    let mut rows = (0..ALBUMS)
        .map(|index| {
            let mut track = projected(80_000 + index, SourceKind::Jellyfin);
            track.native_album_id = Some(format!("guest-album-{index:04}"));
            track.album = format!("Guest Album {index:04}");
            track.credits.push(ArtistCredit {
                display_name: "Remote Guest".to_string(),
                role: CreditRole::Featured,
                ordinal: 1,
            });
            track
        })
        .collect::<Vec<_>>();
    let expected_available = rows.iter().filter(|track| track.available).count();
    let mut cursor = String::new();
    for (batch_index, batch) in rows.chunks_mut(BOOTSTRAP_BATCH_ROWS).enumerate() {
        let committed = (batch_index * BOOTSTRAP_BATCH_ROWS + batch.len()).min(ALBUMS);
        let saved = catalog
            .apply_bootstrap_batch(&BootstrapBatch {
                source: source.clone(),
                snapshot_version: "artist-albums-731".to_string(),
                expected_cursor: cursor,
                next_cursor: committed.to_string(),
                tracks: batch.to_vec(),
                complete: committed == ALBUMS,
            })
            .unwrap();
        cursor = saved.checkpoint_cursor;
    }
    catalog.rebuild_materialized_views().unwrap();

    let key = normalize_artist_key("Remote Guest");
    assert_eq!(
        catalog
            .count_artist_albums(&key, &[source.clone()])
            .unwrap(),
        expected_available as u64
    );
    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let page = catalog
            .query_artist_albums(&key, std::slice::from_ref(&source), cursor.as_ref(), 37)
            .unwrap();
        ids.extend(page.rows.into_iter().map(|row| row.edition_id));
        if !page.has_more {
            break;
        }
        cursor = page.next_cursor;
    }
    assert_eq!(ids.len(), expected_available);
    assert_eq!(
        ids.iter().copied().collect::<HashSet<_>>().len(),
        expected_available
    );
}

#[test]
fn album_keyset_pages_cover_every_id_once_for_all_orders_and_groups() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    insert_album_fixture(&mut catalog, 731);

    for group in [TrackGroup::Off, TrackGroup::Name, TrackGroup::Artist] {
        for sort in [
            TrackSort::ArtistAsc,
            TrackSort::ArtistDesc,
            TrackSort::TitleAsc,
            TrackSort::TitleDesc,
            TrackSort::YearAsc,
            TrackSort::YearDesc,
            TrackSort::AddedDesc,
        ] {
            let descriptor = QueryDescriptor::albums().with_group(group).with_sort(sort);
            let expected = catalog.count_albums(&descriptor).unwrap();
            let rows = collect_all_albums(&catalog, &descriptor, 17);
            let ids = rows
                .iter()
                .map(|row| row.edition_id)
                .collect::<HashSet<_>>();
            assert_eq!(rows.len() as u64, expected, "{group:?} {sort:?}");
            assert_eq!(ids.len(), rows.len(), "{group:?} {sort:?}");
        }
    }
}

#[test]
fn album_entry_counts_include_global_headers_and_never_cross_groups() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    insert_album_fixture(&mut catalog, 257);

    for group in [TrackGroup::Off, TrackGroup::Name, TrackGroup::Artist] {
        let descriptor = QueryDescriptor::albums()
            .with_group(group)
            .with_sort(TrackSort::TitleAsc);
        let rows = collect_all_albums(&catalog, &descriptor, 19);
        for columns in [1, 3, 7] {
            let expected = if group == TrackGroup::Off {
                rows.len().div_ceil(columns)
            } else {
                let mut counts = std::collections::BTreeMap::<String, usize>::new();
                for row in &rows {
                    let key = if group == TrackGroup::Artist {
                        normalize_sort_key(&row.artist)
                    } else {
                        normalize_sort_key(&row.title)
                            .chars()
                            .next()
                            .unwrap_or('#')
                            .to_string()
                    };
                    *counts.entry(key).or_default() += 1;
                }
                counts
                    .values()
                    .map(|count| 1 + count.div_ceil(columns))
                    .sum()
            };
            assert_eq!(
                catalog.count_album_entries(&descriptor, columns).unwrap(),
                expected as u64,
                "{group:?} columns={columns}"
            );
        }
    }
}

#[test]
fn album_fts_filters_and_descriptor_bound_cursors_are_exact() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    insert_album_fixture(&mut catalog, 1_200);

    let broad = QueryDescriptor::albums().with_search("Album");
    let broad_rows = collect_all_albums(&catalog, &broad, 23);
    assert!(broad_rows.len() > 100);
    assert_eq!(
        broad_rows.len() as u64,
        catalog.count_albums(&broad).unwrap()
    );

    let filter_cases: [(QueryDescriptor, fn(&AlbumRecord) -> bool); 5] = [
        (
            QueryDescriptor::albums().with_source_buckets(vec!["plex".to_string()]),
            |row: &AlbumRecord| row.source == SourceKind::Plex,
        ),
        (
            QueryDescriptor::albums().with_formats(vec!["flac".to_string()]),
            |row: &AlbumRecord| row.format == "flac",
        ),
        (
            QueryDescriptor::albums().including_other_formats(true),
            |row: &AlbumRecord| {
                !["flac", "alac", "ape", "wav", "wave", "mp3", "aac"].contains(&row.format.as_str())
            },
        ),
        (
            QueryDescriptor::albums().with_quality_tiers(vec!["hires".to_string()]),
            |row: &AlbumRecord| matches!(row.quality_tier.as_str(), "hires" | "max"),
        ),
        // DSD is a tier of its own: an album holding a DSD copy is 'dsd',
        // never folded into hires.
        (
            QueryDescriptor::albums().with_quality_tiers(vec!["dsd".to_string()]),
            |row: &AlbumRecord| row.quality_tier == "dsd",
        ),
    ];
    for (descriptor, predicate) in filter_cases {
        let rows = collect_all_albums(&catalog, &descriptor, 29);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(predicate));
        assert_eq!(
            rows.len() as u64,
            catalog.count_albums(&descriptor).unwrap()
        );
    }

    let title = QueryDescriptor::albums().with_sort(TrackSort::TitleAsc);
    let first = catalog.query_albums(&title, None, 5).unwrap();
    let cursor = first.next_cursor.expect("fixture spans multiple pages");
    let different = QueryDescriptor::albums().with_sort(TrackSort::YearDesc);
    assert!(matches!(
        catalog.query_albums(&different, Some(&cursor), 5),
        Err(CatalogError::CursorDescriptorMismatch)
    ));
}

#[test]
fn album_query_and_broad_search_metric_uses_a_mixed_deterministic_fixture() {
    const ALBUMS: usize = 20_000;
    let mut catalog = Catalog::open_in_memory(9).unwrap();
    insert_album_fixture(&mut catalog, ALBUMS);

    let descriptor = QueryDescriptor::albums().including_unavailable();
    let count_started = Instant::now();
    let count = catalog.count_albums(&descriptor).unwrap();
    let count_time = count_started.elapsed();
    let (page, query) = catalog.query_albums_timed(&descriptor, None, 100).unwrap();
    let broad = descriptor.with_search("Album");
    let broad_count_started = Instant::now();
    let broad_count = catalog.count_albums(&broad).unwrap();
    let broad_count_time = broad_count_started.elapsed();
    let (broad_page, broad_query) = catalog.query_albums_timed(&broad, None, 100).unwrap();
    let counts = catalog
        .stats()
        .unwrap()
        .source_counts
        .into_iter()
        .map(|(source, rows)| format!("{}={rows}", source.source.as_str()))
        .collect::<Vec<_>>()
        .join(",");

    assert_eq!(count, ALBUMS as u64);
    assert_eq!(page.rows.len(), 100);
    assert!(page.has_more);
    assert_eq!(broad_count, ALBUMS as u64);
    assert_eq!(broad_page.rows.len(), 100);
    println!(
        "F1_ALBUMS_QUERY total={ALBUMS} first_albums={} count_ms={:.3} query_ms={:.3} broad_count_ms={:.3} broad_query_ms={:.3} source_counts={counts}",
        page.rows.len(),
        count_time.as_secs_f64() * 1_000.0,
        query.sql_time.as_secs_f64() * 1_000.0,
        broad_count_time.as_secs_f64() * 1_000.0,
        broad_query.sql_time.as_secs_f64() * 1_000.0,
    );
}

#[test]
fn artist_query_and_broad_search_metric_uses_a_mixed_deterministic_fixture() {
    const ARTISTS: usize = 20_000;
    let mut catalog = Catalog::open_in_memory(10).unwrap();
    insert_album_fixture(&mut catalog, ARTISTS);

    let descriptor = QueryDescriptor::artists();
    let expected = (0..ARTISTS).filter(|index| index % 97 != 0).count() as u64;
    let count_started = Instant::now();
    let count = catalog.count_artists(&descriptor).unwrap();
    let count_time = count_started.elapsed();
    let (page, query) = catalog.query_artists_timed(&descriptor, None, 100).unwrap();
    let broad = descriptor.clone().with_search("Artist");
    let broad_count_started = Instant::now();
    let broad_count = catalog.count_artists(&broad).unwrap();
    let broad_count_time = broad_count_started.elapsed();
    let (broad_page, broad_query) = catalog.query_artists_timed(&broad, None, 100).unwrap();
    let source_counts = [
        SourceKind::Local,
        SourceKind::Offline,
        SourceKind::Plex,
        SourceKind::Jellyfin,
        SourceKind::Subsonic,
    ]
    .into_iter()
    .map(|source| {
        let scoped = QueryDescriptor::artists().with_sources(vec![SourceKey {
            source,
            source_instance: source_instance(source).to_string(),
        }]);
        format!(
            "{}={}",
            source.as_str(),
            catalog.count_artists(&scoped).unwrap()
        )
    })
    .collect::<Vec<_>>()
    .join(",");

    assert_eq!(count, expected);
    assert_eq!(page.rows.len(), 100);
    assert!(page.has_more);
    assert_eq!(broad_count, expected);
    assert_eq!(broad_page.rows.len(), 100);
    assert_eq!(
        catalog.count_artist_entries(&descriptor).unwrap(),
        expected + 1
    );

    let plan = catalog
        .connection()
        .prepare(
            "EXPLAIN QUERY PLAN SELECT artist_key FROM artists_materialized
              WHERE available=1 ORDER BY sort_name,artist_key LIMIT 101",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        plan.iter().all(|step| !step.contains("TEMP B-TREE")),
        "{plan:?}"
    );

    println!(
        "F2_ARTISTS_QUERY total={count} first_artists={} count_ms={:.3} query_ms={:.3} broad_count_ms={:.3} broad_query_ms={:.3} source_counts={source_counts}",
        page.rows.len(),
        count_time.as_secs_f64() * 1_000.0,
        query.sql_time.as_secs_f64() * 1_000.0,
        broad_count_time.as_secs_f64() * 1_000.0,
        broad_query.sql_time.as_secs_f64() * 1_000.0,
    );
}

#[test]
fn opening_an_unrelated_sqlite_file_is_refused_without_mutating_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("authoritative.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute("CREATE TABLE authoritative_secret(value TEXT)", [])
        .unwrap();
    drop(connection);

    assert!(matches!(
        Catalog::open(&path, 1),
        Err(CatalogError::NotCatalog)
    ));
    let connection = rusqlite::Connection::open(&path).unwrap();
    let catalog_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'catalog_meta'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(catalog_tables, 0);
}

#[test]
fn active_catalog_runtime_integrity_probe_is_read_only_and_complete() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("local_catalog-v1-g1.db");
    let mut building = Catalog::open(&path, 1).unwrap();
    insert_album_fixture(&mut building, 321);
    drop(building);

    let active = Catalog::open_read_only(&path, 1).unwrap();
    let report = active.runtime_integrity_check().unwrap();
    assert!(report.sqlite_ok, "{report:?}");
    assert_eq!(report.foreign_key_violations, 0);
    assert!(report.materialized_views_ok);
}

#[test]
fn identical_native_ids_from_distinct_sources_never_collide() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    let mut local = projected(42, SourceKind::Local);
    let mut plex = projected(43, SourceKind::Plex);
    local.source_raw = "qobuz_purchase".to_string();
    local.track_ref.native_id = "same".to_string();
    plex.track_ref.native_id = "same".to_string();
    catalog
        .upsert_tracks(&[local.clone(), plex.clone()])
        .unwrap();

    assert_eq!(catalog.stats().unwrap().track_count, 2);
    assert_eq!(
        catalog
            .resolve(&local.track_ref)
            .unwrap()
            .unwrap()
            .track_ref,
        local.track_ref
    );
    assert_eq!(
        catalog.resolve(&plex.track_ref).unwrap().unwrap().track_ref,
        plex.track_ref
    );
    let resolved = catalog.resolve(&local.track_ref).unwrap().unwrap();
    assert_eq!(resolved.source_raw, "qobuz_purchase");
}

#[test]
fn album_records_preserve_their_physical_source_words() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    let mut local = projected(1, SourceKind::Local);
    local.source_raw = "user".to_string();
    let mut jellyfin = projected(2, SourceKind::Jellyfin);
    jellyfin.source_raw = "jellyfin".to_string();
    for (source, track) in [
        (
            SourceKey {
                source: SourceKind::Local,
                source_instance: source_instance(SourceKind::Local).to_string(),
            },
            local,
        ),
        (
            SourceKey {
                source: SourceKind::Jellyfin,
                source_instance: source_instance(SourceKind::Jellyfin).to_string(),
            },
            jellyfin,
        ),
    ] {
        catalog
            .apply_bootstrap_batch(&BootstrapBatch {
                source,
                snapshot_version: "source-words".to_string(),
                expected_cursor: String::new(),
                next_cursor: "1".to_string(),
                tracks: vec![track],
                complete: true,
            })
            .unwrap();
    }
    catalog.rebuild_materialized_views().unwrap();

    let rows = collect_all_albums(&catalog, &QueryDescriptor::albums(), 20);
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|row| row.source_words == vec!["user".to_string()]));
    assert!(rows
        .iter()
        .any(|row| row.source_words == vec!["jellyfin".to_string()]));
}

#[test]
fn upsert_retains_identity_and_fts_tracks_updates_and_deletes() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    let mut track = projected(1, SourceKind::Jellyfin);
    track.title = "Old Needle".to_string();
    catalog.upsert_tracks(&[track.clone()]).unwrap();
    assert_eq!(
        catalog
            .count_tracks(&QueryDescriptor::tracks().with_search("Old Needle"))
            .unwrap(),
        1
    );

    track.title = "New Signal".to_string();
    catalog.upsert_tracks(&[track.clone()]).unwrap();
    assert_eq!(catalog.stats().unwrap().track_count, 1);
    assert_eq!(
        catalog
            .count_tracks(&QueryDescriptor::tracks().with_search("Old Needle"))
            .unwrap(),
        0
    );
    assert_eq!(
        catalog
            .count_tracks(&QueryDescriptor::tracks().with_search("New Signal"))
            .unwrap(),
        1
    );
    assert!(catalog.remove_track(&track.track_ref).unwrap());
    assert_eq!(catalog.stats().unwrap().track_count, 0);
    let integrity = catalog.integrity_check().unwrap();
    assert!(integrity.sqlite_ok && integrity.fts_ok);
    assert_eq!(integrity.foreign_key_violations, 0);
}

#[test]
fn reconciliation_keeps_album_and_artist_fts_in_sync_across_update_and_delete() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    let source = SourceKey {
        source: SourceKind::Jellyfin,
        source_instance: source_instance(SourceKind::Jellyfin).to_string(),
    };
    let mut track = projected(7, SourceKind::Jellyfin);
    track.album = "Old Album Needle".to_string();
    track.album_artist = "Old Artist Needle".to_string();
    track.artist = track.album_artist.clone();
    track.credits = vec![ArtistCredit {
        display_name: track.artist.clone(),
        role: CreditRole::TrackArtist,
        ordinal: 0,
    }];

    catalog.begin_reconciliation(&source, "snapshot-1").unwrap();
    catalog
        .apply_reconciliation_batch(&ReconciliationBatch {
            source: source.clone(),
            snapshot_version: "snapshot-1".to_string(),
            expected_cursor: String::new(),
            next_cursor: "1".to_string(),
            tracks: vec![track.clone()],
            complete: true,
        })
        .unwrap();
    catalog.rebuild_materialized_views().unwrap();
    assert_eq!(
        catalog
            .count_albums(&QueryDescriptor::albums().with_search("Old Album Needle"))
            .unwrap(),
        1
    );
    assert_eq!(
        catalog
            .count_artists(&QueryDescriptor::artists().with_search("Old Artist Needle"))
            .unwrap(),
        1
    );

    track.album = "New Album Signal".to_string();
    track.album_artist = "New Artist Signal".to_string();
    track.artist = track.album_artist.clone();
    track.credits[0].display_name = track.artist.clone();
    catalog.begin_reconciliation(&source, "snapshot-2").unwrap();
    catalog
        .apply_reconciliation_batch(&ReconciliationBatch {
            source: source.clone(),
            snapshot_version: "snapshot-2".to_string(),
            expected_cursor: String::new(),
            next_cursor: "1".to_string(),
            tracks: vec![track],
            complete: true,
        })
        .unwrap();
    catalog.rebuild_materialized_views().unwrap();
    assert_eq!(
        catalog
            .count_albums(&QueryDescriptor::albums().with_search("Old Album Needle"))
            .unwrap(),
        0
    );
    assert_eq!(
        catalog
            .count_artists(&QueryDescriptor::artists().with_search("Old Artist Needle"))
            .unwrap(),
        0
    );
    assert_eq!(
        catalog
            .count_albums(&QueryDescriptor::albums().with_search("New Album Signal"))
            .unwrap(),
        1
    );
    assert_eq!(
        catalog
            .count_artists(&QueryDescriptor::artists().with_search("New Artist Signal"))
            .unwrap(),
        1
    );

    catalog.begin_reconciliation(&source, "snapshot-3").unwrap();
    catalog
        .apply_reconciliation_batch(&ReconciliationBatch {
            source,
            snapshot_version: "snapshot-3".to_string(),
            expected_cursor: String::new(),
            next_cursor: String::new(),
            tracks: Vec::new(),
            complete: true,
        })
        .unwrap();
    catalog.rebuild_materialized_views().unwrap();
    assert_eq!(catalog.stats().unwrap().track_count, 0);
    assert_eq!(
        catalog
            .count_albums(&QueryDescriptor::albums().with_search("New Album Signal"))
            .unwrap(),
        0
    );
    assert_eq!(
        catalog
            .count_artists(&QueryDescriptor::artists().with_search("New Artist Signal"))
            .unwrap(),
        0
    );
    let integrity = catalog.integrity_check().unwrap();
    assert!(integrity.sqlite_ok && integrity.fts_ok);
    assert_eq!(integrity.foreign_key_violations, 0);
}

#[test]
fn artist_keys_preserve_roles_fold_diacritics_and_keep_non_latin_names() {
    assert_eq!(normalize_artist_key("Beyoncé & Co."), "beyonce co");
    assert_eq!(normalize_artist_key("宇多田 ヒカル"), "宇多田 ヒカル");
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    let mut track = projected(3, SourceKind::Subsonic);
    track.credits = vec![
        ArtistCredit {
            display_name: "Beyoncé".to_string(),
            role: CreditRole::TrackArtist,
            ordinal: 0,
        },
        ArtistCredit {
            display_name: "Guest".to_string(),
            role: CreditRole::Featured,
            ordinal: 1,
        },
    ];
    catalog.upsert_tracks(&[track]).unwrap();
    let credits: Vec<(String, String)> = catalog
        .connection()
        .prepare("SELECT artist_key, role FROM artist_credits ORDER BY ordinal")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        credits,
        vec![
            ("beyonce".to_string(), "track_artist".to_string()),
            ("guest".to_string(), "featured".to_string())
        ]
    );
}

#[test]
fn every_sort_uses_keysets_without_omissions_or_duplicates() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    insert_fixture(&mut catalog, 1_701);
    let expected = catalog.count_tracks(&QueryDescriptor::tracks()).unwrap() as usize;
    for sort in [
        TrackSort::Default,
        TrackSort::TitleAsc,
        TrackSort::TitleDesc,
        TrackSort::ArtistAsc,
        TrackSort::ArtistDesc,
        TrackSort::YearAsc,
        TrackSort::YearDesc,
        TrackSort::AddedDesc,
    ] {
        let rows = collect_all(&catalog, &QueryDescriptor::tracks().with_sort(sort));
        let ids: HashSet<TrackRef> = rows.iter().map(|row| row.track_ref.clone()).collect();
        assert_eq!(rows.len(), expected, "sort {sort:?}");
        assert_eq!(ids.len(), rows.len(), "sort {sort:?}");
    }
}

#[test]
fn search_source_format_availability_and_group_are_descriptor_scoped() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    insert_fixture(&mut catalog, 2_500);
    let search = QueryDescriptor::tracks().with_search("Signal 042");
    assert!(catalog.count_tracks(&search).unwrap() > 0);
    assert!(matches!(
        catalog.count_tracks(&QueryDescriptor::tracks().with_search("Si")),
        Err(CatalogError::SearchTooShort)
    ));

    let plex_flac = QueryDescriptor::tracks()
        .with_sources(vec![SourceKey {
            source: SourceKind::Plex,
            source_instance: source_instance(SourceKind::Plex).to_string(),
        }])
        .with_formats(vec!["FLAC".to_string()]);
    let rows = collect_all(&catalog, &plex_flac);
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| {
        row.track_ref.source == SourceKind::Plex && row.format == "flac" && row.available
    }));

    let available = catalog.count_tracks(&QueryDescriptor::tracks()).unwrap();
    let all = catalog
        .count_tracks(&QueryDescriptor::tracks().including_unavailable())
        .unwrap();
    assert!(all > available);

    let grouped = collect_all(
        &catalog,
        &QueryDescriptor::tracks()
            .with_sort(TrackSort::AddedDesc)
            .with_group(TrackGroup::Artist),
    );
    assert_eq!(grouped.len(), available as usize);

    let first = catalog
        .query_tracks(&QueryDescriptor::tracks(), None, 50)
        .unwrap();
    let cursor = first.next_cursor.expect("fixture has another page");
    assert!(matches!(
        catalog.query_tracks(
            &QueryDescriptor::tracks().with_formats(vec!["flac".to_string()]),
            Some(&cursor),
            50,
        ),
        Err(CatalogError::CursorDescriptorMismatch)
    ));
    assert!(matches!(
        catalog.count_tracks(&QueryDescriptor::albums()),
        Err(CatalogError::InvalidInput(_))
    ));
}

#[test]
fn track_source_bucket_quality_and_other_format_filters_match_the_ui_funnel() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    insert_fixture(&mut catalog, 2_500);

    // Downloaded Qobuz copies can be owned by the Local catalog source. The
    // UI nevertheless presents them through Offline, based on source_raw.
    let mut purchased = projected(9_999, SourceKind::Local);
    purchased.track_ref.native_id = "local-qobuz-purchase".to_string();
    purchased.source_raw = "qobuz_purchase".to_string();
    purchased.available = true;
    catalog.upsert_tracks(&[purchased]).unwrap();

    let local = QueryDescriptor::tracks().with_source_buckets(vec!["local".to_string()]);
    let local_rows = collect_all(&catalog, &local);
    assert!(!local_rows.is_empty());
    assert!(local_rows.iter().all(|row| {
        row.track_ref.source == SourceKind::Local
            && !matches!(row.source_raw.as_str(), "qobuz_download" | "qobuz_purchase")
    }));
    assert_eq!(
        catalog.count_tracks(&local).unwrap(),
        local_rows.len() as u64
    );

    let offline = QueryDescriptor::tracks().with_source_buckets(vec!["offline".to_string()]);
    let offline_rows = collect_all(&catalog, &offline);
    assert!(offline_rows
        .iter()
        .any(|row| row.source_raw == "qobuz_purchase"));
    assert!(offline_rows.iter().all(|row| {
        row.track_ref.source == SourceKind::Offline
            || matches!(row.source_raw.as_str(), "qobuz_download" | "qobuz_purchase")
    }));
    assert_eq!(
        catalog.count_tracks(&offline).unwrap(),
        offline_rows.len() as u64
    );

    let plex_hires_flac = QueryDescriptor::tracks()
        .with_source_buckets(vec!["plex".to_string()])
        .with_formats(vec!["flac".to_string()])
        .with_quality_tiers(vec!["hires".to_string()]);
    let plex_rows = collect_all(&catalog, &plex_hires_flac);
    assert!(!plex_rows.is_empty());
    assert!(plex_rows.iter().all(|row| {
        row.track_ref.source == SourceKind::Plex
            && row.format == "flac"
            && row.bit_depth.is_some_and(|depth| depth >= 24)
    }));
    assert_eq!(
        catalog.count_tracks(&plex_hires_flac).unwrap(),
        plex_rows.len() as u64
    );

    let other = QueryDescriptor::tracks().including_other_formats(true);
    let other_rows = collect_all(&catalog, &other);
    assert!(!other_rows.is_empty());
    assert!(other_rows.iter().all(|row| row.format == "dsf"));
    assert_eq!(
        catalog.count_tracks(&other).unwrap(),
        other_rows.len() as u64
    );

    // The DSD quality chip is the fixture's dsf rows exactly, and Hi-Res no
    // longer folds them in.
    let dsd = QueryDescriptor::tracks().with_quality_tiers(vec!["dsd".to_string()]);
    let dsd_rows = collect_all(&catalog, &dsd);
    assert_eq!(dsd_rows.len(), other_rows.len());
    assert!(dsd_rows.iter().all(|row| row.format == "dsf"));
    let hires = QueryDescriptor::tracks().with_quality_tiers(vec!["hires".to_string()]);
    assert!(collect_all(&catalog, &hires)
        .iter()
        .all(|row| row.format != "dsf"));
}

#[test]
fn artist_group_uses_track_artist_globally_across_keyset_pages() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    let mut zed = projected(1, SourceKind::Local);
    zed.artist = "Zed Performer".to_string();
    zed.album_artist = "Alpha Album Artist".to_string();
    let mut alpha = projected(2, SourceKind::Plex);
    alpha.artist = "Alpha Performer".to_string();
    alpha.album_artist = "Zed Album Artist".to_string();
    catalog.upsert_tracks(&[zed, alpha]).unwrap();

    let descriptor = QueryDescriptor::tracks().with_group(TrackGroup::Artist);
    let first = catalog.query_tracks(&descriptor, None, 1).unwrap();
    assert_eq!(first.rows[0].artist, "Alpha Performer");
    let cursor = first.next_cursor.expect("second grouped page");
    assert_eq!(cursor.group_key(TrackGroup::Artist), "alpha performer");
    let second = catalog.query_tracks(&descriptor, Some(&cursor), 1).unwrap();
    assert_eq!(second.rows[0].artist, "Zed Performer");
    assert!(!second.has_more);
}

#[test]
fn album_hierarchy_keeps_weak_same_name_matches_reversible() {
    let catalog = Catalog::open_in_memory(1).unwrap();
    catalog
        .connection()
        .execute_batch(
            "INSERT INTO logical_albums
                 (stable_key,display_title,sort_title,display_artist,sort_artist,
                  association_strength,association_evidence)
             VALUES
                 ('plex:a','Album','album','Artist','artist','source_native','plex:a'),
                 ('jellyfin:b','Album','album','Artist','artist','source_native','jellyfin:b');
             INSERT INTO editions
                 (logical_album_id,edition_key,display_title,display_artist,evidence_kind,evidence_value)
             SELECT logical_album_id, stable_key || ':edition', display_title, display_artist,
                    'source_native', stable_key
               FROM logical_albums;
             INSERT INTO source_copies
                 (edition_id,source_kind,source_instance,native_album_id)
             SELECT edition_id,
                    CASE WHEN edition_key LIKE 'plex:%' THEN 'plex' ELSE 'jellyfin' END,
                    CASE WHEN edition_key LIKE 'plex:%' THEN 'plex-a' ELSE 'jellyfin-b' END,
                    edition_key
               FROM editions;",
        )
        .unwrap();
    let counts: (i64, i64, i64) = catalog
        .connection()
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM logical_albums),
                 (SELECT COUNT(*) FROM editions),
                 (SELECT COUNT(*) FROM source_copies)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (2, 2, 2));
}

#[test]
fn sort_indices_match_the_actual_order_by_without_temp_sort() {
    let mut catalog = Catalog::open_in_memory(1).unwrap();
    insert_fixture(&mut catalog, 100);
    for (index, order) in [
        (
            "idx_tracks_default",
            "sort_album,sort_artist,disc_sort,track_sort,sort_title,catalog_id",
        ),
        ("idx_tracks_title_asc", "sort_title,sort_artist,catalog_id"),
        (
            "idx_tracks_title_desc",
            "sort_title DESC,sort_artist,catalog_id",
        ),
        (
            "idx_tracks_artist_asc",
            "sort_artist,sort_album,disc_sort,track_sort,catalog_id",
        ),
        (
            "idx_tracks_artist_desc",
            "sort_artist DESC,sort_album,disc_sort,track_sort,catalog_id",
        ),
        (
            "idx_tracks_year_asc",
            "year_missing,year_value,sort_album,disc_sort,track_sort,catalog_id",
        ),
        (
            "idx_tracks_year_desc",
            "year_missing,year_value DESC,sort_album,disc_sort,track_sort,catalog_id",
        ),
        (
            "idx_tracks_added_desc",
            "added_at DESC,sort_album,disc_sort,track_sort,catalog_id",
        ),
        (
            "idx_tracks_group_artist",
            "sort_track_artist,sort_album,sort_title,catalog_id",
        ),
    ] {
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT catalog_id FROM tracks
              WHERE available=1 ORDER BY {order} LIMIT 250"
        );
        let details = catalog
            .connection()
            .prepare(&sql)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" | ");
        assert!(details.contains(index), "{details}");
        assert!(!details.contains("TEMP B-TREE"), "{details}");
    }

    let fts_plan = catalog
        .connection()
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT t.catalog_id
               FROM tracks_fts
               CROSS JOIN tracks t NOT INDEXED
              WHERE tracks_fts MATCH '\"Signal 042\"'
                AND t.catalog_id=tracks_fts.rowid
                AND t.available=1
              ORDER BY t.sort_album,t.sort_artist,t.disc_sort,t.track_sort,
                       t.sort_title,t.catalog_id
              LIMIT 250",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join(" | ");
    assert!(fts_plan.contains("VIRTUAL TABLE INDEX"), "{fts_plan}");
    assert!(fts_plan.contains("INTEGER PRIMARY KEY"), "{fts_plan}");
}

#[test]
fn album_sort_indices_match_the_paged_order_by_without_temp_sort() {
    let catalog = Catalog::open_in_memory(1).unwrap();
    for (index, order) in [
        (
            "idx_albums_materialized_artist",
            "sort_artist,sort_title,edition_id",
        ),
        (
            "idx_albums_materialized_artist_desc",
            "sort_artist DESC,sort_title,edition_id",
        ),
        (
            "idx_albums_materialized_title",
            "sort_title,sort_artist,edition_id",
        ),
        (
            "idx_albums_materialized_title_desc",
            "sort_title DESC,sort_artist,edition_id",
        ),
        (
            "idx_albums_materialized_year",
            "year_missing,year_value,sort_title,edition_id",
        ),
        (
            "idx_albums_materialized_year_desc",
            "year_missing,year_value DESC,sort_title,edition_id",
        ),
        (
            "idx_albums_materialized_added",
            "added_at DESC,sort_title,edition_id",
        ),
    ] {
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT edition_id FROM albums_materialized
              WHERE available=1 ORDER BY {order} LIMIT 100"
        );
        let details = catalog
            .connection()
            .prepare(&sql)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" | ");
        assert!(details.contains(index), "{details}");
        assert!(!details.contains("TEMP B-TREE"), "{details}");
    }
}

/// The contract's reproducible scale gate. It is ignored in ordinary unit
/// runs because it intentionally creates and removes a several-hundred-MiB
/// database; the implementation handoff records an explicit execution.
#[test]
#[ignore = "explicit 1M catalog fixture and query benchmark"]
fn fixture_one_million_tracks_meets_the_query_shape_gate() {
    const TRACKS: usize = 1_000_000;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("local_catalog-v1-g1.db.building");
    let mut catalog = Catalog::open(&path, 1).unwrap();
    let insert_started = Instant::now();
    for start in (0..TRACKS).step_by(5_000) {
        let end = (start + 5_000).min(TRACKS);
        let batch = (start..end)
            .map(|index| projected(index, source_for(index)))
            .collect::<Vec<_>>();
        catalog.upsert_tracks(&batch).unwrap();
        if end % 100_000 == 0 {
            println!("[catalog-1m] projected={end}");
        }
    }
    let insert_time = insert_started.elapsed();
    catalog
        .connection()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let stats = catalog.stats().unwrap();
    assert_eq!(stats.track_count, TRACKS as u64);
    let integrity = catalog.integrity_check().unwrap();
    assert!(integrity.sqlite_ok && integrity.fts_ok);
    assert_eq!(integrity.foreign_key_violations, 0);

    let default = QueryDescriptor::tracks();
    let (_, first_warmup) = catalog.query_tracks_timed(&default, None, 250).unwrap();
    let (_, first_warm) = catalog.query_tracks_timed(&default, None, 250).unwrap();
    let broad_fts = QueryDescriptor::tracks().with_search("Signal 731");
    let (broad_page, broad_fts_cold) = catalog.query_tracks_timed(&broad_fts, None, 250).unwrap();
    assert_eq!(broad_page.rows.len(), 250);
    let (_, broad_fts_warm) = catalog.query_tracks_timed(&broad_fts, None, 250).unwrap();
    let selective_fts = QueryDescriptor::tracks().with_search("0731042");
    let (selective_page, selective_fts_cold) = catalog
        .query_tracks_timed(&selective_fts, None, 250)
        .unwrap();
    assert_eq!(selective_page.rows.len(), 1);
    let (_, selective_fts_warm) = catalog
        .query_tracks_timed(&selective_fts, None, 250)
        .unwrap();

    let deep_cursor = catalog
        .connection()
        .query_row(
            "SELECT sort_title,sort_artist,sort_track_artist,sort_album,
                    year_missing,year_value,disc_sort,track_sort,added_at,catalog_id
               FROM tracks WHERE available=1
              ORDER BY sort_title,sort_artist,catalog_id
              LIMIT 1 OFFSET 800000",
            [],
            |row| {
                Ok(TrackCursor {
                    sort: TrackSort::TitleAsc,
                    descriptor_key: crate::catalog::descriptor_key(
                        &QueryDescriptor::tracks().with_sort(TrackSort::TitleAsc),
                    ),
                    sort_title: row.get(0)?,
                    sort_artist: row.get(1)?,
                    sort_track_artist: row.get(2)?,
                    sort_album: row.get(3)?,
                    year_missing: row.get(4)?,
                    year_value: row.get(5)?,
                    disc_sort: row.get(6)?,
                    track_sort: row.get(7)?,
                    added_at: row.get(8)?,
                    row_id: row.get(9)?,
                })
            },
        )
        .unwrap();
    let title = QueryDescriptor::tracks().with_sort(TrackSort::TitleAsc);
    let (_, deep_metrics) = catalog
        .query_tracks_timed(&title, Some(&deep_cursor), 250)
        .unwrap();

    drop(catalog);
    let reopened = Catalog::open(&path, 1).unwrap();
    let (_, connection_cold) = reopened.query_tracks_timed(&default, None, 250).unwrap();
    let file_bytes = std::fs::metadata(&path).unwrap().len();
    println!(
        "[catalog-1m] rows={TRACKS} db_bytes={file_bytes} insert_s={:.3} first_warmup_ms={:.3} first_warm_ms={:.3} broad_fts_cold_ms={:.3} broad_fts_warm_ms={:.3} selective_fts_cold_ms={:.3} selective_fts_warm_ms={:.3} deep_keyset_ms={:.3} connection_cold_ms={:.3}",
        insert_time.as_secs_f64(),
        first_warmup.sql_time.as_secs_f64() * 1000.0,
        first_warm.sql_time.as_secs_f64() * 1000.0,
        broad_fts_cold.sql_time.as_secs_f64() * 1000.0,
        broad_fts_warm.sql_time.as_secs_f64() * 1000.0,
        selective_fts_cold.sql_time.as_secs_f64() * 1000.0,
        selective_fts_warm.sql_time.as_secs_f64() * 1000.0,
        deep_metrics.sql_time.as_secs_f64() * 1000.0,
        connection_cold.sql_time.as_secs_f64() * 1000.0,
    );

    assert!(first_warm.sql_time <= Duration::from_millis(50));
    assert!(broad_fts_warm.sql_time <= Duration::from_millis(250));
    assert!(selective_fts_warm.sql_time <= Duration::from_millis(50));
    assert!(deep_metrics.sql_time <= Duration::from_millis(100));
    assert!(connection_cold.sql_time <= Duration::from_millis(250));
}
