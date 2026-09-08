//! DJ-mix sampling for Collections / Mixtapes.
//!
//! Pure functions used by the unique-track-count and shuffle-tracks paths.
//! No Tauri types here — fully unit-testable.
//!
//! See spec: qbz-nix-docs/superpowers/specs/2026-04-25-track-shuffle-mix-design.md

use qbz_models::QueueTrack as CoreQueueTrack;
use rand::RngExt;
use std::collections::{BTreeMap, BTreeSet};

/// Tracks whose normalized titles score at or above this Jaro/token-set
/// threshold are considered the same song (within the same normalized artist
/// bucket).
pub const SIMILARITY_THRESHOLD: f32 = 0.80;

/// No single album may contribute more than this fraction of the requested
/// sample size, after applying [`ALBUM_CAP_MIN`] as a floor.
pub const ALBUM_CAP_PCT: f32 = 0.30;

/// Floor for the per-album cap so that small requested sizes do not feel
/// artificially trimmed (e.g. requested = 20, cap = max(2, 6) = 6).
pub const ALBUM_CAP_MIN: usize = 2;

/// Lowercase, strip diacritics, drop bracketed parentheticals, drop ` - `
/// suffixes, drop `feat. X` patterns, drop punctuation, collapse whitespace.
pub fn normalize_title(s: &str) -> String {
    let lower = s.to_lowercase();
    let unaccented = strip_diacritics(&lower);
    let unbracketed = remove_brackets(&unaccented);
    let untrailed = strip_dash_suffix(&unbracketed);
    let unfeatured = strip_feat(&untrailed);
    let unpunct = strip_punctuation(&unfeatured);
    collapse_whitespace(&unpunct)
}

/// Lowercase + strip diacritics + trim. Parens are preserved (e.g. `Foo (band)`
/// must not collapse to `Foo`).
pub fn normalize_artist(s: &str) -> String {
    let lower = s.to_lowercase();
    let unaccented = strip_diacritics(&lower);
    collapse_whitespace(&unaccented)
}

/// Count of distinct songs in `tracks` after similarity-based grouping.
/// Deterministic — does not use an RNG. Same input always yields the same
/// count.
pub fn unique_track_count(tracks: &[CoreQueueTrack]) -> usize {
    if tracks.is_empty() {
        return 0;
    }
    let groups = build_similarity_groups(tracks);
    groups.iter().copied().collect::<BTreeSet<usize>>().len()
}

/// Removes near-duplicate tracks from `tracks`. Two tracks are considered the
/// same song when their normalized artists match exactly AND their normalized
/// titles score at or above [`SIMILARITY_THRESHOLD`]. From each duplicate
/// group, one survivor is picked at random via `rng`.
///
/// The output preserves the original input order of the surviving tracks.
pub fn dedup_by_similarity<R: rand::Rng>(
    tracks: Vec<CoreQueueTrack>,
    rng: &mut R,
) -> Vec<CoreQueueTrack> {
    if tracks.is_empty() {
        return tracks;
    }

    let groups = build_similarity_groups(&tracks);

    // Bucket original indices by their group representative.
    let mut by_group: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &g) in groups.iter().enumerate() {
        by_group.entry(g).or_default().push(i);
    }

    // Pick one random survivor index per group.
    let mut survivors: BTreeSet<usize> = BTreeSet::new();
    for indices in by_group.values() {
        let chosen = if indices.len() == 1 {
            indices[0]
        } else {
            indices[rng.random_range(0..indices.len())]
        };
        survivors.insert(chosen);
    }

    tracks
        .into_iter()
        .enumerate()
        .filter(|(i, _)| survivors.contains(i))
        .map(|(_, t)| t)
        .collect()
}

/// For each track index, returns the index of the group representative under
/// the artist-bucketed token-set similarity grouping. Tracks in different
/// artist buckets always end up in different groups.
fn build_similarity_groups(tracks: &[CoreQueueTrack]) -> Vec<usize> {
    let n = tracks.len();
    let mut parent: Vec<usize> = (0..n).collect();

    // Bucket indices by normalized artist.
    let mut by_artist: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, t) in tracks.iter().enumerate() {
        by_artist
            .entry(normalize_artist(&t.artist))
            .or_default()
            .push(i);
    }

    // Within each artist bucket, union pairs whose normalized titles are
    // similar enough.
    for indices in by_artist.values() {
        let titles: Vec<String> = indices
            .iter()
            .map(|&i| normalize_title(&tracks[i].title))
            .collect();
        for a in 0..indices.len() {
            for b in (a + 1)..indices.len() {
                if token_set_ratio(&titles[a], &titles[b]) >= SIMILARITY_THRESHOLD {
                    uf_union(&mut parent, indices[a], indices[b]);
                }
            }
        }
    }

    (0..n).map(|i| uf_find(&mut parent, i)).collect()
}

fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]]; // path compression (halving)
        x = parent[x];
    }
    x
}

fn uf_union(parent: &mut [usize], a: usize, b: usize) {
    let ra = uf_find(parent, a);
    let rb = uf_find(parent, b);
    if ra != rb {
        // Smaller index becomes parent so behavior is order-stable for tests
        // that don't shuffle.
        let (root, child) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parent[child] = root;
    }
}

/// Picks up to `requested` tracks from `tracks` such that no album contributes
/// more than `cap = max(ALBUM_CAP_MIN, ceil(requested * ALBUM_CAP_PCT))` picks.
/// Albums are drawn round-robin in a per-round randomized order; within each
/// album, the surviving picks are themselves shuffled.
///
/// May return fewer than `requested` tracks if the per-album cap and bucket
/// sizes do not add up to `requested`.
pub fn hybrid_sample<R: rand::Rng>(
    tracks: Vec<CoreQueueTrack>,
    requested: usize,
    rng: &mut R,
) -> Vec<CoreQueueTrack> {
    if tracks.is_empty() || requested == 0 {
        return Vec::new();
    }

    let cap =
        (((requested as f32) * ALBUM_CAP_PCT).ceil() as usize).max(ALBUM_CAP_MIN);

    // Bucket original indices by album_id (None goes to a synthetic bucket).
    let mut by_album: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, t) in tracks.iter().enumerate() {
        let key = t
            .album_id
            .clone()
            .unwrap_or_else(|| "_unknown".to_string());
        by_album.entry(key).or_default().push(i);
    }

    // Shuffle each bucket and apply quota = min(cap, bucket.len()) by truncating.
    let mut buckets: Vec<Vec<usize>> = by_album
        .into_values()
        .map(|mut indices| {
            fisher_yates(&mut indices, rng);
            indices.truncate(cap);
            indices
        })
        .collect();

    // Round-robin pick with album order permuted each round. `pop` from each
    // bucket — order within a bucket is already random, so pop_back is fine.
    let mut picked: Vec<usize> = Vec::with_capacity(requested);
    while picked.len() < requested {
        fisher_yates(&mut buckets, rng);
        let mut progress = false;
        for bucket in buckets.iter_mut() {
            if let Some(idx) = bucket.pop() {
                picked.push(idx);
                progress = true;
                if picked.len() >= requested {
                    break;
                }
            }
        }
        if !progress {
            break;
        }
    }

    // Map indices back to tracks, preserving the picked order.
    use std::collections::HashMap;
    let mut by_index: HashMap<usize, CoreQueueTrack> =
        tracks.into_iter().enumerate().collect();
    picked
        .into_iter()
        .filter_map(|i| by_index.remove(&i))
        .collect()
}

fn fisher_yates<T, R: rand::Rng>(slice: &mut [T], rng: &mut R) {
    if slice.len() < 2 {
        return;
    }
    for i in (1..slice.len()).rev() {
        let j = rng.random_range(0..=i);
        slice.swap(i, j);
    }
}

/// Token-set similarity in `[0.0, 1.0]`, modeled on RapidFuzz's
/// `token_set_ratio`. Inputs are expected to be already normalized.
pub fn token_set_ratio(a: &str, b: &str) -> f32 {
    use std::collections::BTreeSet;

    let tokens_a: BTreeSet<&str> = a.split_whitespace().collect();
    let tokens_b: BTreeSet<&str> = b.split_whitespace().collect();

    if tokens_a.is_empty() && tokens_b.is_empty() {
        return 1.0;
    }
    if tokens_a.is_empty() || tokens_b.is_empty() {
        return 0.0;
    }

    let inter: Vec<&str> = tokens_a.intersection(&tokens_b).copied().collect();
    let diff_a: Vec<&str> = tokens_a.difference(&tokens_b).copied().collect();
    let diff_b: Vec<&str> = tokens_b.difference(&tokens_a).copied().collect();

    let t1 = inter.join(" ");
    let t2 = join_with_intersection(&t1, &diff_a);
    let t3 = join_with_intersection(&t1, &diff_b);

    let r12 = strsim::normalized_levenshtein(&t1, &t2);
    let r13 = strsim::normalized_levenshtein(&t1, &t3);
    let r23 = strsim::normalized_levenshtein(&t2, &t3);

    r12.max(r13).max(r23) as f32
}

// ──────── internal helpers ────────

fn strip_diacritics(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ę' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
            'ú' | 'ù' | 'û' | 'ü' | 'ū' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            'ÿ' | 'ý' => 'y',
            'ß' => 's',
            other => other,
        })
        .collect()
}

fn remove_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {
                if depth == 0 {
                    out.push(c);
                }
            }
        }
    }
    out
}

fn strip_dash_suffix(s: &str) -> String {
    match s.find(" - ") {
        Some(idx) => s[..idx].to_string(),
        None => s.to_string(),
    }
}

fn strip_feat(s: &str) -> String {
    // Order matters: longest patterns first so " feat. " wins over " feat ".
    const PATTERNS: &[&str] = &[" featuring ", " feat. ", " feat ", " ft. ", " ft "];
    for p in PATTERNS {
        if let Some(idx) = s.find(p) {
            return s[..idx].to_string();
        }
    }
    s.to_string()
}

fn strip_punctuation(s: &str) -> String {
    const PUNCT: &[char] = &[
        ',', '.', '!', '?', '¿', '¡', '"', '\'', ';', ':', '/', '\\',
    ];
    s.chars().filter(|c| !PUNCT.contains(c)).collect()
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<&str>>().join(" ")
}

fn join_with_intersection(t1: &str, diff: &[&str]) -> String {
    if diff.is_empty() {
        return t1.to_string();
    }
    let diff_joined = diff.join(" ");
    if t1.is_empty() {
        diff_joined
    } else {
        format!("{} {}", t1, diff_joined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn mk_track(id: u64, title: &str, artist: &str, album_id: Option<&str>) -> CoreQueueTrack {
        CoreQueueTrack {
            id,
            title: title.to_string(),
            version: None,
            artist: artist.to_string(),
            album: String::new(),
            album_version: None,
            duration_secs: 0,
            artwork_url: None,
            hires: false,
            bit_depth: None,
            sample_rate: None,
            is_local: false,
            album_id: album_id.map(|s| s.to_string()),
            artist_id: None,
            streamable: true,
            source: None,
            parental_warning: false,
            source_item_id_hint: None,
            context_kind: None,
            context_id: None,
            isrc: None,
            recording_mbid: None,
        }
    }

    fn deterministic_rng() -> rand::rngs::StdRng {
        rand::rngs::StdRng::seed_from_u64(42)
    }

    // ──────── normalize_title ────────

    #[test]
    fn normalize_title_lowercases() {
        assert_eq!(normalize_title("Yesterday"), "yesterday");
    }

    #[test]
    fn normalize_title_strips_parens() {
        assert_eq!(normalize_title("Yesterday (Live)"), "yesterday");
        assert_eq!(normalize_title("Song (Acoustic Version)"), "song");
    }

    #[test]
    fn normalize_title_strips_brackets_and_braces() {
        assert_eq!(normalize_title("Track [Bonus]"), "track");
        assert_eq!(normalize_title("Tune {Demo}"), "tune");
    }

    #[test]
    fn normalize_title_strips_dash_suffix() {
        assert_eq!(normalize_title("Song - 2003 Remaster"), "song");
        assert_eq!(normalize_title("Tune - Live at Wembley"), "tune");
    }

    #[test]
    fn normalize_title_strips_feat() {
        assert_eq!(normalize_title("Song feat. Artist X"), "song");
        assert_eq!(normalize_title("Tune ft. X"), "tune");
        assert_eq!(normalize_title("Anthem featuring Y"), "anthem");
    }

    #[test]
    fn normalize_title_strips_diacritics() {
        assert_eq!(normalize_title("Café"), "cafe");
        assert_eq!(normalize_title("Niño"), "nino");
        assert_eq!(normalize_title("Über"), "uber");
    }

    #[test]
    fn normalize_title_strips_punctuation() {
        assert_eq!(normalize_title("Don't Stop!"), "dont stop");
        assert_eq!(normalize_title("¿Qué Pasa?"), "que pasa");
    }

    #[test]
    fn normalize_title_collapses_whitespace() {
        assert_eq!(normalize_title("  Hello   World  "), "hello world");
    }

    #[test]
    fn normalize_title_combined() {
        assert_eq!(
            normalize_title("¡Yesterday! (Live, Wembley) - 2003 Remaster feat. Friend"),
            "yesterday"
        );
    }

    // ──────── normalize_artist ────────

    #[test]
    fn normalize_artist_lowercases_and_trims() {
        assert_eq!(normalize_artist("  The Beatles  "), "the beatles");
    }

    #[test]
    fn normalize_artist_strips_diacritics() {
        assert_eq!(normalize_artist("Mägo de Oz"), "mago de oz");
    }

    #[test]
    fn normalize_artist_keeps_parens() {
        // "Foo (band)" must NOT collapse to "Foo" — that's title behavior, not artist.
        assert_eq!(normalize_artist("Foo (band)"), "foo (band)");
    }

    // ──────── token_set_ratio ────────

    #[test]
    fn token_set_ratio_identical_returns_one() {
        assert!((token_set_ratio("yesterday", "yesterday") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn token_set_ratio_subset_returns_one() {
        // RapidFuzz behavior: when one string's tokens are a subset of the
        // other, t1 == t2 (the smaller side), so max similarity is 1.0.
        let s = token_set_ratio("yesterday", "yesterday live wembley");
        assert!(s >= 0.95, "expected >= 0.95, got {}", s);
    }

    #[test]
    fn token_set_ratio_disjoint_returns_low() {
        let s = token_set_ratio("yesterday", "tomorrow");
        assert!(s < 0.50, "expected < 0.50, got {}", s);
    }

    #[test]
    fn token_set_ratio_overlap_passes_threshold() {
        // Three of four words shared after normalization.
        let s = token_set_ratio("song of the south", "song of the north");
        assert!(s >= SIMILARITY_THRESHOLD, "expected >= 0.80, got {}", s);
    }

    #[test]
    fn token_set_ratio_unrelated_fails_threshold() {
        let s = token_set_ratio("a totally different song", "yesterday");
        assert!(s < SIMILARITY_THRESHOLD, "expected < 0.80, got {}", s);
    }

    #[test]
    fn token_set_ratio_empty_inputs_safe() {
        // Both empty → defined as 1.0 (vacuously identical).
        assert!((token_set_ratio("", "") - 1.0).abs() < 1e-6);
        // One empty → 0.0 (no overlap).
        assert!(token_set_ratio("", "yesterday") < 1e-6);
    }

    // ──────── unique_track_count ────────

    #[test]
    fn unique_track_count_zero_for_empty() {
        assert_eq!(unique_track_count(&[]), 0);
    }

    #[test]
    fn unique_track_count_distinct_tracks() {
        let tracks = vec![
            mk_track(1, "Yesterday", "The Beatles", Some("a1")),
            mk_track(2, "Hey Jude", "The Beatles", Some("a1")),
            mk_track(3, "Let It Be", "The Beatles", Some("a1")),
        ];
        assert_eq!(unique_track_count(&tracks), 3);
    }

    #[test]
    fn unique_track_count_groups_versions() {
        let tracks = vec![
            mk_track(1, "Yesterday", "The Beatles", Some("a1")),
            mk_track(2, "Yesterday (Live)", "The Beatles", Some("a2")),
            mk_track(3, "Yesterday - 2003 Remaster", "The Beatles", Some("a3")),
            mk_track(4, "Hey Jude", "The Beatles", Some("a1")),
            mk_track(5, "Let It Be", "The Beatles", Some("a1")),
        ];
        // 3 versions of "Yesterday" collapse to 1, plus 2 distinct → 3 unique.
        assert_eq!(unique_track_count(&tracks), 3);
    }

    #[test]
    fn unique_track_count_respects_artist_buckets() {
        let tracks = vec![
            mk_track(1, "Yesterday", "The Beatles", Some("a1")),
            mk_track(2, "Yesterday", "Boyz II Men", Some("a2")),
        ];
        // Same title, different artists → not deduplicated.
        assert_eq!(unique_track_count(&tracks), 2);
    }

    #[test]
    fn unique_track_count_is_deterministic() {
        let tracks = vec![
            mk_track(1, "Yesterday", "The Beatles", Some("a1")),
            mk_track(2, "Yesterday (Live)", "The Beatles", Some("a2")),
            mk_track(3, "Hey Jude", "The Beatles", Some("a1")),
        ];
        let c1 = unique_track_count(&tracks);
        let c2 = unique_track_count(&tracks);
        let c3 = unique_track_count(&tracks);
        assert_eq!(c1, c2);
        assert_eq!(c2, c3);
        assert_eq!(c1, 2);
    }

    // ──────── dedup_by_similarity ────────

    #[test]
    fn dedup_empty_returns_empty() {
        let mut rng = deterministic_rng();
        assert!(dedup_by_similarity(vec![], &mut rng).is_empty());
    }

    #[test]
    fn dedup_single_track_passes_through() {
        let mut rng = deterministic_rng();
        let out = dedup_by_similarity(
            vec![mk_track(1, "Yesterday", "The Beatles", Some("a1"))],
            &mut rng,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, 1);
    }

    #[test]
    fn dedup_keeps_distinct_titles() {
        let mut rng = deterministic_rng();
        let tracks = vec![
            mk_track(1, "Yesterday", "The Beatles", Some("a1")),
            mk_track(2, "Hey Jude", "The Beatles", Some("a1")),
            mk_track(3, "Let It Be", "The Beatles", Some("a1")),
            mk_track(4, "Help!", "The Beatles", Some("a1")),
            mk_track(5, "Imagine", "The Beatles", Some("a1")),
        ];
        let out = dedup_by_similarity(tracks, &mut rng);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn dedup_collapses_versions() {
        let mut rng = deterministic_rng();
        let tracks = vec![
            mk_track(1, "Yesterday", "The Beatles", Some("a1")),
            mk_track(2, "Yesterday (Live)", "The Beatles", Some("a2")),
            mk_track(3, "Yesterday - 2003 Remaster", "The Beatles", Some("a3")),
        ];
        let out = dedup_by_similarity(tracks, &mut rng);
        assert_eq!(out.len(), 1);
        // The survivor must be one of the three input ids.
        assert!([1u64, 2, 3].contains(&out[0].id));
    }

    #[test]
    fn dedup_respects_artist_buckets() {
        let mut rng = deterministic_rng();
        let tracks = vec![
            mk_track(1, "Yesterday", "The Beatles", Some("a1")),
            mk_track(2, "Yesterday", "Boyz II Men", Some("a2")),
        ];
        let out = dedup_by_similarity(tracks, &mut rng);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn dedup_random_winner_varies_across_seeds() {
        // Over many seeds, all three versions should be selected at least once.
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        for seed in 0..200u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let tracks = vec![
                mk_track(1, "Yesterday", "The Beatles", Some("a1")),
                mk_track(2, "Yesterday (Live)", "The Beatles", Some("a2")),
                mk_track(3, "Yesterday - 2003 Remaster", "The Beatles", Some("a3")),
            ];
            let out = dedup_by_similarity(tracks, &mut rng);
            assert_eq!(out.len(), 1);
            seen.insert(out[0].id);
        }
        assert_eq!(seen.len(), 3, "over 200 seeds, all 3 versions should win at least once; got {:?}", seen);
    }

    // ──────── hybrid_sample ────────

    #[test]
    fn hybrid_sample_empty_returns_empty() {
        let mut rng = deterministic_rng();
        assert!(hybrid_sample(vec![], 10, &mut rng).is_empty());
    }

    #[test]
    fn hybrid_sample_zero_requested_returns_empty() {
        let mut rng = deterministic_rng();
        let tracks = vec![mk_track(1, "A", "X", Some("a1"))];
        assert!(hybrid_sample(tracks, 0, &mut rng).is_empty());
    }

    #[test]
    fn hybrid_sample_single_album_caps_at_floor() {
        // 1 album, 50 tracks, requested = 20.
        // cap = max(2, ceil(20 * 0.3)) = max(2, 6) = 6.
        // With only 1 album, total quota = 6, so result.len() = 6.
        let mut rng = deterministic_rng();
        let tracks: Vec<_> = (0..50)
            .map(|i| mk_track(i, &format!("Track {i}"), "X", Some("a1")))
            .collect();
        let out = hybrid_sample(tracks, 20, &mut rng);
        assert_eq!(out.len(), 6);
    }

    #[test]
    fn hybrid_sample_returns_requested_when_distribution_allows() {
        // 10 albums × 20 tracks = 200; requested = 50.
        // cap = max(2, ceil(50 * 0.3)) = max(2, 15) = 15.
        // total quota = min(15, 20) * 10 = 150 > 50, so we hit `requested`.
        let mut rng = deterministic_rng();
        let mut tracks = Vec::new();
        for album in 0..10u64 {
            for track in 0..20u64 {
                tracks.push(mk_track(
                    album * 100 + track,
                    &format!("Track {album}-{track}"),
                    "X",
                    Some(&format!("a{album}")),
                ));
            }
        }
        let out = hybrid_sample(tracks, 50, &mut rng);
        assert_eq!(out.len(), 50);
    }

    #[test]
    fn hybrid_sample_respects_album_cap_statistical() {
        // 3 albums × 100 tracks each; requested = 20; cap = max(2, 6) = 6.
        // Across 200 seeds, no album ever exceeds 6 picks.
        for seed in 0..200u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let mut tracks = Vec::new();
            for album in 0..3u64 {
                for track in 0..100u64 {
                    tracks.push(mk_track(
                        album * 1000 + track,
                        &format!("T{album}-{track}"),
                        "X",
                        Some(&format!("alb{album}")),
                    ));
                }
            }
            let out = hybrid_sample(tracks, 20, &mut rng);

            let mut per_album: BTreeMap<String, usize> = BTreeMap::new();
            for t in &out {
                let key = t.album_id.clone().unwrap();
                *per_album.entry(key).or_default() += 1;
            }
            let max_per = per_album.values().copied().max().unwrap_or(0);
            assert!(
                max_per <= 6,
                "seed {seed}: max picks per album was {max_per}, expected <= 6"
            );
        }
    }

    #[test]
    fn hybrid_sample_distributes_across_albums() {
        // 5 albums × 50 tracks; requested = 15; cap = max(2, 5) = 5.
        // With round-robin, expect at least 3 albums represented.
        let mut rng = deterministic_rng();
        let mut tracks = Vec::new();
        for album in 0..5u64 {
            for track in 0..50u64 {
                tracks.push(mk_track(
                    album * 1000 + track,
                    &format!("T{album}-{track}"),
                    "X",
                    Some(&format!("alb{album}")),
                ));
            }
        }
        let out = hybrid_sample(tracks, 15, &mut rng);
        assert_eq!(out.len(), 15);
        let albums: BTreeSet<String> = out
            .iter()
            .map(|t| t.album_id.clone().unwrap())
            .collect();
        assert!(
            albums.len() >= 3,
            "expected >= 3 albums represented, got {} ({:?})",
            albums.len(),
            albums
        );
    }

    #[test]
    fn hybrid_sample_groups_unknown_album_id() {
        // Tracks without album_id share the synthetic "_unknown" bucket and
        // therefore share one cap, not one per track.
        let mut rng = deterministic_rng();
        let tracks: Vec<_> = (0..50)
            .map(|i| mk_track(i, &format!("Track {i}"), "X", None))
            .collect();
        let out = hybrid_sample(tracks, 20, &mut rng);
        // Same as the single-album case: cap = 6.
        assert_eq!(out.len(), 6);
    }

    #[test]
    fn dedup_preserves_input_order_of_survivors() {
        let mut rng = deterministic_rng();
        let tracks = vec![
            mk_track(10, "Hey Jude", "The Beatles", Some("a1")),
            mk_track(20, "Yesterday", "The Beatles", Some("a1")),
            mk_track(30, "Let It Be", "The Beatles", Some("a1")),
        ];
        let out = dedup_by_similarity(tracks, &mut rng);
        // None of these collapse, so all survive in original order.
        assert_eq!(out.iter().map(|t| t.id).collect::<Vec<_>>(), vec![10, 20, 30]);
    }
}
