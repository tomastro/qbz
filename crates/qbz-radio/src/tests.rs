use super::db::{RadioDb, RadioSeed};
use super::engine::RadioEngine;

fn seed_artist_id() -> u64 {
    42
}

#[test]
fn radio_no_repetition() {
    let db = RadioDb::open_in_memory().unwrap();
    let session = db
        .create_session(
            RadioSeed::Artist {
                artist_id: seed_artist_id(),
            },
            123,
            5,
            25,
        )
        .unwrap();

    for track_id in 1u64..=120u64 {
        let artist_id = 1000 + (track_id % 10);
        let distance = if track_id % 7 == 0 {
            2
        } else if track_id % 3 == 0 {
            1
        } else {
            0
        };
        db.insert_pool_track(&session.id, track_id, artist_id, "test_pool", distance)
            .unwrap();
    }

    let engine = RadioEngine::new(db);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..120 {
        let t = engine.next_track(&session.id).unwrap();
        assert!(seen.insert(t.track_id), "Track repeated: {}", t.track_id);
    }
}

#[test]
fn radio_artist_spacing() {
    let db = RadioDb::open_in_memory().unwrap();
    let session = db
        .create_session(
            RadioSeed::Artist {
                artist_id: seed_artist_id(),
            },
            999,
            5,
            25,
        )
        .unwrap();

    let artists: Vec<u64> = (1u64..=6u64).collect();
    let mut track_id = 1u64;
    for a in &artists {
        for _ in 0..30 {
            db.insert_pool_track(&session.id, track_id, *a, "test_pool", 1)
                .unwrap();
            track_id += 1;
        }
    }

    let engine = RadioEngine::new(db);
    let mut recent: std::collections::VecDeque<u64> = std::collections::VecDeque::new();
    for _ in 0..120 {
        let t = engine.next_track(&session.id).unwrap();
        assert!(
            !recent.contains(&t.artist_id),
            "Artist repeated within spacing window: {}",
            t.artist_id
        );
        recent.push_back(t.artist_id);
        while recent.len() > 5 {
            recent.pop_front();
        }
    }
}

#[test]
fn radio_distance_constraint() {
    let db = RadioDb::open_in_memory().unwrap();
    let session = db
        .create_session(
            RadioSeed::Artist {
                artist_id: seed_artist_id(),
            },
            555,
            5,
            25,
        )
        .unwrap();

    for track_id in 1u64..=50u64 {
        db.insert_pool_track(&session.id, track_id, 1, "ok", 2)
            .unwrap();
    }
    for track_id in 1001u64..=1020u64 {
        db.insert_pool_track_unchecked(&session.id, track_id, 2, "too_far", 3)
            .unwrap();
    }

    let engine = RadioEngine::new(db);
    for _ in 0..50 {
        let t = engine.next_track(&session.id).unwrap();
        assert!(
            t.distance <= 2,
            "Selected track with distance {}",
            t.distance
        );
    }
}

#[test]
fn radio_stability_over_time_pool_constant_seed_present() {
    let db = RadioDb::open_in_memory().unwrap();
    let session = db
        .create_session(
            RadioSeed::Artist {
                artist_id: seed_artist_id(),
            },
            2025,
            5,
            25,
        )
        .unwrap();

    let seed_artist = seed_artist_id();
    let mut track_id = 1u64;

    for _ in 0..200 {
        db.insert_pool_track(&session.id, track_id, seed_artist, "seed_tracks", 0)
            .unwrap();
        track_id += 1;
    }

    for a in 100u64..=199u64 {
        for _ in 0..2 {
            db.insert_pool_track(&session.id, track_id, a, "similar_artist", 1)
                .unwrap();
            track_id += 1;
        }
    }

    let pool_size = db.pool_size(&session.id).unwrap();
    assert!(pool_size >= 350);

    let engine = RadioEngine::new(db);
    let mut picks: Vec<u64> = Vec::new();
    for _ in 0..220 {
        let t = engine.next_track(&session.id).unwrap();
        picks.push(t.artist_id);
    }

    let pool_size_after = engine.db().pool_size(&session.id).unwrap();
    assert_eq!(pool_size, pool_size_after);

    let seed_in_last_60 = picks.iter().rev().take(60).any(|a| *a == seed_artist);
    assert!(seed_in_last_60, "Seed artist missing late in session");
}
