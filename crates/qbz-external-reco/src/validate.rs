//! Resolve a recommendation candidate to a real Qobuz entity (track/artist/album).
//!
//! Tracks: ISRC -> MusicBrainz `inc=isrcs` -> Qobuz, else fuzzy text.
//! Artists: Qobuz artist-search + normalized-name match.
//! Albums: UPC match if known, else fuzzy text (title*0.6 + artist*0.4).
//! Every outcome (positive AND negative) is cached.

use std::sync::Mutex;

use qbz_integrations::MusicBrainzClient;
use qbz_models::{Album, Track};

use crate::cache::{CacheLookup, RecoCache};
use crate::matching::{normalize, select_best_match, similarity, MatchInput, MIN_SCORE};
use crate::types::{
    AlbumCandidate, AlbumReco, ArtistCandidate, ArtistReco, RecoSource, TrackCandidate, TrackReco,
};
use crate::RecoCatalog;

type Cache<'a> = Option<&'a Mutex<RecoCache>>;

const ALBUM_MIN_SCORE: f32 = 0.6;

/// Minimum track count to treat a release as a full album when Qobuz did not
/// label its `release_type` (singles/EPs are short).
const MIN_ALBUM_TRACKS: u32 = 5;

/// Keep only proper full albums — drop singles, EPs, boxsets, compilations.
/// Qobuz's `release_type` is the source of truth ("album" | "single" |
/// "boxset" | "compilation"); when it is absent, fall back to the track count.
pub fn is_full_album(album: &Album) -> bool {
    match album.release_type.as_deref() {
        Some(rt) => rt.eq_ignore_ascii_case("album"),
        None => album.tracks_count.or(album.track_count).unwrap_or(0) >= MIN_ALBUM_TRACKS,
    }
}

/// Second layer against karaoke / tribute / "made famous by" AI-slop that can
/// still wear a full-album shape. Matched on the artist OR title, case-folded.
pub fn is_slop(artist: &str, title: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "karaoke",
        "tribute to",
        "tribute band",
        "made famous by",
        "made popular by",
        "as made famous",
        "as made popular",
        "originally performed",
        "in the style of",
        "instrumental version",
    ];
    let a = artist.to_lowercase();
    let t = title.to_lowercase();
    NEEDLES.iter().any(|n| a.contains(n) || t.contains(n))
}

/// Build the reco only if the resolved Qobuz album is a real full album and not
/// karaoke/tribute slop; otherwise discard the candidate (cached as negative).
fn album_if_full(a: &Album, cand: &AlbumCandidate) -> Option<AlbumReco> {
    if is_full_album(a) && !is_slop(&a.artist.name, &a.title) {
        Some(build_album_reco(a, cand.subtitle.clone(), cand.source))
    } else {
        None
    }
}

// ── Tracks ─────────────────────────────────────────────────────────────────

fn track_cache_key(c: &TrackCandidate) -> String {
    if let Some(isrc) = c.isrc.as_deref().filter(|s| !s.is_empty()) {
        format!("t:isrc:{}", isrc.to_uppercase())
    } else if let Some(mbid) = c.recording_mbid.as_deref().filter(|s| !s.is_empty()) {
        format!("t:mbid:{}", mbid)
    } else {
        format!("t:name:{}|{}", normalize(&c.artist), normalize(&c.title))
    }
}

fn build_track_reco(track: &Track, source: RecoSource) -> TrackReco {
    TrackReco {
        qobuz_track_id: track.id,
        title: track.title.clone(),
        artist: track
            .performer
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default(),
        artwork_url: track
            .album
            .as_ref()
            .and_then(|al| al.image.best().cloned())
            .unwrap_or_default(),
        source,
    }
}

/// ISRC -> the streamable Qobuz track carrying it, or `None`.
///
/// Public because it is the FIRST thing the "find available version" flow tries
/// on a track Qobuz pulled from the catalogue: a dead row keeps its `isrc` (the
/// 2026-08-17 capture of album `0886443985094` confirms it), and licensing churn
/// most often re-publishes the SAME recording under a new track id with the same
/// ISRC. A hit there is an exact relink, and it skips the fuzzy text search and
/// the human judgement entirely.
///
/// Qobuz has no ISRC endpoint — this is a free-text search FOR the ISRC string
/// plus an exact, case-folded verification of the returned `isrc`, which is why
/// it can answer `None` for an ISRC that does exist. Treat a miss as "no exact
/// relink", never as "the recording is gone".
pub async fn find_by_isrc(catalog: &dyn RecoCatalog, isrc: &str) -> Option<Track> {
    let results = catalog.search_tracks(isrc, 5).await;
    results.into_iter().find(|t| {
        // `is_streamable()`, not the raw field: a search result that simply did
        // not report availability is still a valid relink target, and this
        // lookup is the cheapest, most certain path there is (same ISRC = same
        // recording under a new id). Only an explicit false is rejected.
        t.is_streamable()
            && t.isrc
                .as_deref()
                .map(|c| c.eq_ignore_ascii_case(isrc))
                .unwrap_or(false)
    })
}

async fn resolve_track_live(
    catalog: &dyn RecoCatalog,
    mb: &MusicBrainzClient,
    cand: &TrackCandidate,
    skip_mb: bool,
) -> Option<TrackReco> {
    if let Some(isrc) = cand.isrc.as_deref().filter(|s| !s.is_empty()) {
        if let Some(track) = find_by_isrc(catalog, isrc).await {
            return Some(build_track_reco(&track, cand.source));
        }
    }
    // The MusicBrainz recording->ISRC lookup is gated behind a SERIAL 1.1s/req
    // rate limiter; for a 50-track weekly playlist (×2 rows) that is ~110s of
    // pure waiting, so the row never paints in practice. `skip_mb` bypasses it
    // and relies on the fuzzy Qobuz search below — reliable for the mainstream
    // tracks these playlists contain. (This is THE reason the Weekly rows did
    // not appear while Fresh Releases — album-based, no MusicBrainz — did.)
    if !skip_mb {
        if let Some(mbid) = cand.recording_mbid.as_deref().filter(|s| !s.is_empty()) {
            let isrcs = mb.get_recording_isrcs(mbid).await.unwrap_or_default();
            for isrc in isrcs {
                if let Some(track) = find_by_isrc(catalog, &isrc).await {
                    return Some(build_track_reco(&track, cand.source));
                }
            }
        }
    }
    let query = format!("{} {}", cand.artist, cand.title);
    let candidates = catalog.search_tracks(query.trim(), 20).await;
    let input = MatchInput {
        artist: &cand.artist,
        title: &cand.title,
        album: cand.album.as_deref(),
        duration_ms: cand.duration_ms,
        isrc: cand.isrc.as_deref(),
    };
    let (best, score) = select_best_match(&input, &candidates);
    match best {
        Some(track) if score >= MIN_SCORE => Some(build_track_reco(track, cand.source)),
        _ => None,
    }
}

/// Resolve a track candidate to a Qobuz track.
///
/// `skip_negative`: when true, the per-track NEGATIVE cache is ignored on BOTH
/// read and write. This is for ListenBrainz weekly playlists, whose resolution
/// runs under heavy concurrent first-build load: a transient throttle/timeout
/// returns `None`, and persisting that as a 7-day negative would lock the track
/// (and so the whole weekly row) out long after the hiccup passed — the exact
/// mechanism that made Weekly Exploration/Jams "vanish". POSITIVE hits are still
/// cached (cheap re-resolution), and the resolved set is cached per-week by the
/// caller.
pub async fn validate_track(
    catalog: &dyn RecoCatalog,
    mb: &MusicBrainzClient,
    cache: Cache<'_>,
    cand: &TrackCandidate,
    skip_negative: bool,
    skip_mb: bool,
) -> Option<TrackReco> {
    let key = track_cache_key(cand);
    if let Some(c) = cache {
        if let Ok(guard) = c.lock() {
            match guard.get(&key) {
                CacheLookup::Found(json) => {
                    if let Ok(mut reco) = serde_json::from_str::<TrackReco>(&json) {
                        reco.source = cand.source;
                        return Some(reco);
                    }
                }
                // A cached negative is authoritative only when we trust negatives.
                CacheLookup::Negative if !skip_negative => return None,
                _ => {}
            }
        }
    }
    let reco = resolve_track_live(catalog, mb, cand, skip_mb).await;
    if let Some(c) = cache {
        if let Ok(guard) = c.lock() {
            match &reco {
                Some(r) => guard.put(&key, "track", Some(&serde_json::to_string(r).unwrap_or_default())),
                // Only persist a negative when negatives are trusted (NOT for
                // weeklies — a transient miss must not stick for 7 days).
                None if !skip_negative => guard.put(&key, "track", None),
                None => {}
            }
        }
    }
    reco
}

// ── Artists ────────────────────────────────────────────────────────────────

pub async fn validate_artist(
    catalog: &dyn RecoCatalog,
    cache: Cache<'_>,
    cand: &ArtistCandidate,
) -> Option<ArtistReco> {
    let key = format!("a:{}", normalize(&cand.name));
    if let Some(c) = cache {
        if let Ok(guard) = c.lock() {
            match guard.get(&key) {
                CacheLookup::Found(json) => {
                    if let Ok(mut reco) = serde_json::from_str::<ArtistReco>(&json) {
                        reco.source = cand.source;
                        reco.subtitle = cand.subtitle.clone();
                        return Some(reco);
                    }
                }
                CacheLookup::Negative => return None,
                CacheLookup::Miss => {}
            }
        }
    }

    let target = normalize(&cand.name);
    let artists = catalog.search_artists(&cand.name, 8).await;
    let reco = artists
        .iter()
        .find(|a| normalize(&a.name) == target)
        .or_else(|| artists.first())
        .filter(|a| a.id != 0)
        .map(|a| ArtistReco {
            qobuz_artist_id: a.id,
            name: a.name.clone(),
            image_url: a.image.as_ref().and_then(|i| i.best().cloned()).unwrap_or_default(),
            subtitle: cand.subtitle.clone(),
            source: cand.source,
        });

    if let Some(c) = cache {
        if let Ok(guard) = c.lock() {
            match &reco {
                Some(r) => guard.put(&key, "artist", Some(&serde_json::to_string(r).unwrap_or_default())),
                None => guard.put(&key, "artist", None),
            }
        }
    }
    reco
}

// ── Albums ─────────────────────────────────────────────────────────────────

fn album_cache_key(c: &AlbumCandidate) -> String {
    if let Some(upc) = c.upc.as_deref().filter(|s| !s.is_empty()) {
        format!("alb:upc:{}", upc)
    } else {
        format!("alb:{}|{}", normalize(&c.artist), normalize(&c.title))
    }
}

pub fn build_album_reco(album: &Album, subtitle: String, source: RecoSource) -> AlbumReco {
    let year = album
        .release_date_original
        .as_deref()
        .and_then(|s| s.get(..4).map(|y| y.to_string()))
        .unwrap_or_default();
    let quality_tier = match album.maximum_bit_depth {
        Some(d) if d >= 24 => "hires",
        Some(_) => "cd",
        None => "",
    }
    .to_string();
    let quality_label = match (album.maximum_bit_depth, album.maximum_sampling_rate) {
        (Some(bd), Some(sr)) => format!("{}-bit / {} kHz", bd, sr),
        _ => String::new(),
    };
    AlbumReco {
        qobuz_album_id: album.id.clone(),
        title: album.title.clone(),
        artist: album.artist.name.clone(),
        artist_id: album.artist.id.to_string(),
        year,
        quality_tier,
        quality_label,
        artwork_url: album.image.best().cloned().unwrap_or_default(),
        subtitle,
        genre: album
            .genre
            .as_ref()
            .map(|g| g.name.clone())
            .unwrap_or_default(),
        source,
    }
}

async fn resolve_album_live(catalog: &dyn RecoCatalog, cand: &AlbumCandidate) -> Option<AlbumReco> {
    if let Some(upc) = cand.upc.as_deref().filter(|s| !s.is_empty()) {
        let albums = catalog.search_albums(upc, 5).await;
        if let Some(a) = albums
            .iter()
            .find(|a| a.upc.as_deref().map(|u| u.eq_ignore_ascii_case(upc)).unwrap_or(false))
        {
            // The UPC pins the exact release; if it's a single/slop, discard the
            // candidate rather than fuzzy-hunting for a different album.
            return album_if_full(a, cand);
        }
    }
    let query = format!("{} {}", cand.artist, cand.title);
    let albums = catalog.search_albums(query.trim(), 10).await;
    let mut best: Option<&Album> = None;
    let mut best_score = 0.0f32;
    for a in &albums {
        let title_s = similarity(&cand.title, &a.title);
        let artist_s = similarity(&cand.artist, &a.artist.name);
        let score = title_s * 0.6 + artist_s * 0.4;
        if score > best_score {
            best_score = score;
            best = Some(a);
        }
    }
    match best {
        Some(a) if best_score >= ALBUM_MIN_SCORE => album_if_full(a, cand),
        _ => None,
    }
}

pub async fn validate_album(
    catalog: &dyn RecoCatalog,
    cache: Cache<'_>,
    cand: &AlbumCandidate,
) -> Option<AlbumReco> {
    let key = album_cache_key(cand);
    if let Some(c) = cache {
        if let Ok(guard) = c.lock() {
            match guard.get(&key) {
                CacheLookup::Found(json) => {
                    if let Ok(mut reco) = serde_json::from_str::<AlbumReco>(&json) {
                        reco.source = cand.source;
                        reco.subtitle = cand.subtitle.clone();
                        return Some(reco);
                    }
                }
                CacheLookup::Negative => return None,
                CacheLookup::Miss => {}
            }
        }
    }
    let reco = resolve_album_live(catalog, cand).await;
    if let Some(c) = cache {
        if let Ok(guard) = c.lock() {
            match &reco {
                Some(r) => guard.put(&key, "album", Some(&serde_json::to_string(r).unwrap_or_default())),
                None => guard.put(&key, "album", None),
            }
        }
    }
    reco
}
