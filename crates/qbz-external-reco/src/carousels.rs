//! Per-row candidate generation, blending, filtering, Qobuz validation, rotation.
//!
//! The documented Last.fm "artist discovery" recipe (api-evangelist/lastfm
//! arazzo workflow): top artists -> artist.getSimilar -> top albums. There is no
//! recommendation endpoint, so recommendations are replicated from similarity.

use std::collections::HashSet;
use std::sync::Mutex;

use futures_util::stream::{self, StreamExt};
use qbz_models::Album;

use crate::cache::RecoCache;
use crate::matching::normalize;
use crate::types::{
    AlbumCandidate, AlbumReco, ArtistCandidate, ArtistReco, ExtHistory, RecoSource, TrackCandidate,
    TrackReco,
};
use crate::validate::{
    build_album_reco, is_full_album, is_slop, validate_album, validate_artist, validate_track,
};
use crate::{RecoCatalog, RecoInputs};

const DISPLAY_CAP: usize = 20;
/// Public alias for the paint layer (it composes the visible rows after
/// filtering/dedup, so it needs the canonical per-rail visible cap).
pub const ARTIST_DISPLAY_CAP: usize = DISPLAY_CAP;
const PLAYLIST_CAP: usize = 30;
const VALIDATE_CONCURRENCY: usize = 6;
const ARTIST_SEEDS: usize = 6;
const SIMILAR_PER_SEED: u32 = 12;
const KNOWN_ARTISTS_PER_BUILD: usize = 50;

type Cache<'a> = Option<&'a Mutex<RecoCache>>;

fn track_key(artist: &str, title: &str) -> String {
    format!("{}|{}", normalize(artist), normalize(title))
}
fn album_key(artist: &str, album: &str) -> String {
    format!("{}|{}", normalize(artist), normalize(album))
}

fn rotate_take<T>(mut pool: Vec<T>, seed: u64, take: usize) -> Vec<T> {
    if pool.is_empty() {
        return pool;
    }
    let off = (seed as usize) % pool.len();
    pool.rotate_left(off);
    pool.truncate(take);
    pool
}

// ── Validation pools (concurrent, blend-ordered, deduped) ───────────────────

async fn validate_artist_pool(
    catalog: &dyn RecoCatalog,
    cache: Cache<'_>,
    cands: Vec<ArtistCandidate>,
) -> Vec<ArtistReco> {
    let resolved: Vec<Option<ArtistReco>> = stream::iter(
        cands.into_iter().map(|cand| async move { validate_artist(catalog, cache, &cand).await }),
    )
    .buffered(VALIDATE_CONCURRENCY)
    .collect()
    .await;
    let mut seen = HashSet::new();
    resolved
        .into_iter()
        .flatten()
        .filter(|r| seen.insert(r.qobuz_artist_id))
        .collect()
}

async fn validate_album_pool(
    catalog: &dyn RecoCatalog,
    cache: Cache<'_>,
    cands: Vec<AlbumCandidate>,
) -> Vec<AlbumReco> {
    let resolved: Vec<Option<AlbumReco>> = stream::iter(
        cands.into_iter().map(|cand| async move { validate_album(catalog, cache, &cand).await }),
    )
    .buffered(VALIDATE_CONCURRENCY)
    .collect()
    .await;
    let mut seen = HashSet::new();
    resolved
        .into_iter()
        .flatten()
        .filter(|r| seen.insert(r.qobuz_album_id.clone()))
        .collect()
}

async fn validate_track_pool(
    catalog: &dyn RecoCatalog,
    mb: &qbz_integrations::MusicBrainzClient,
    cache: Cache<'_>,
    cands: Vec<TrackCandidate>,
    skip_negative: bool,
    skip_mb: bool,
) -> Vec<TrackReco> {
    let resolved: Vec<Option<TrackReco>> = stream::iter(cands.into_iter().map(|cand| async move {
        validate_track(catalog, mb, cache, &cand, skip_negative, skip_mb).await
    }))
    .buffered(VALIDATE_CONCURRENCY)
    .collect()
    .await;
    let mut seen = HashSet::new();
    resolved
        .into_iter()
        .flatten()
        .filter(|r| seen.insert(r.qobuz_track_id))
        .collect()
}

// ── Shared external history ─────────────────────────────────────────────────

pub async fn gather_history(inputs: &RecoInputs<'_>) -> ExtHistory {
    let mut artist_names = HashSet::new();
    let mut track_keys = HashSet::new();
    let mut album_keys = HashSet::new();

    if let Some(lf) = &inputs.lastfm {
        let (tops, recents, albums) = tokio::join!(
            lf.client.get_top_artists(&lf.username, "overall", 300),
            lf.client.get_recent_tracks(&lf.username, 200, 1),
            lf.client.get_user_top_albums(&lf.username, "overall", 300, 1),
        );
        for a in tops.unwrap_or_default() {
            artist_names.insert(normalize(&a.name));
        }
        for t in recents.unwrap_or_default() {
            artist_names.insert(normalize(&t.artist));
            track_keys.insert(track_key(&t.artist, &t.name));
        }
        for al in albums.unwrap_or_default() {
            artist_names.insert(normalize(&al.artist));
            album_keys.insert(album_key(&al.artist, &al.name));
        }
    }
    if let Some(lb) = &inputs.listenbrainz {
        let listens = lb.client.get_recent_listens(&lb.username, 1000).await.unwrap_or_default();
        for l in listens {
            artist_names.insert(normalize(&l.artist_name));
            track_keys.insert(track_key(&l.artist_name, &l.track_name));
        }
    }
    ExtHistory {
        artist_names,
        track_keys,
        album_keys,
    }
}

// ── Recommended Artists — common (overall top) vs recent (1-month top) ──────
//
// Split so a recent one-off binge (e.g. a soundtrack) can't pollute the core
// taste row, and so the user gets more, better-targeted rows. Round-robin per
// seed inside each row so no single seed floods the carousel.

const PER_SEED_CAP: usize = 8;

/// Interleave per-seed candidate lists round-robin (fair representation).
fn round_robin<T>(groups: Vec<Vec<T>>) -> Vec<T> {
    let mut iters: Vec<std::vec::IntoIter<T>> = groups.into_iter().map(|g| g.into_iter()).collect();
    let mut out = Vec::new();
    loop {
        let mut any = false;
        for it in iters.iter_mut() {
            if let Some(x) = it.next() {
                out.push(x);
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    out
}

async fn similar_artist_row(
    inputs: &RecoInputs<'_>,
    history: &ExtHistory,
    period: &str,
) -> Vec<ArtistReco> {
    let Some(lf) = &inputs.lastfm else {
        return Vec::new();
    };
    let seeds: Vec<String> = lf
        .client
        .get_top_artists(&lf.username, period, 12)
        .await
        .unwrap_or_default()
        .into_iter()
        .take(ARTIST_SEEDS)
        .map(|a| a.name)
        .collect();
    let seeds_norm: HashSet<String> = seeds.iter().map(|s| normalize(s)).collect();

    let sim_results: Vec<(String, Vec<qbz_integrations::lastfm::LastFmSimilarArtist>)> =
        stream::iter(seeds.into_iter().map(|seed| {
            let lf = lf;
            async move {
                let sims = lf
                    .client
                    .get_similar_artists(&seed, SIMILAR_PER_SEED)
                    .await
                    .unwrap_or_default();
                (seed, sims)
            }
        }))
        .buffered(4)
        .collect()
        .await;

    // One candidate list per seed, deduped globally (first seed wins), capped so
    // one seed cannot dominate; then round-robin interleaved.
    let mut seen_global: HashSet<String> = HashSet::new();
    let mut groups: Vec<Vec<ArtistCandidate>> = Vec::new();
    for (seed, sims) in sim_results {
        let mut list: Vec<ArtistCandidate> = Vec::new();
        for s in sims {
            let nk = normalize(&s.name);
            if nk.is_empty()
                || history.artist_names.contains(&nk)
                || seeds_norm.contains(&nk)
                || !seen_global.insert(nk)
            {
                continue;
            }
            list.push(ArtistCandidate {
                name: s.name,
                source: RecoSource::LastFm,
                score: s.match_score as f32,
                subtitle: format!("Similar to {}", seed),
            });
            if list.len() >= PER_SEED_CAP {
                break;
            }
        }
        groups.push(list);
    }
    let candidates: Vec<ArtistCandidate> = round_robin(groups).into_iter().take(45).collect();
    let pool = validate_artist_pool(inputs.catalog, inputs.cache, candidates).await;
    // Rotate for daily variety but do NOT truncate at DISPLAY_CAP: the paint
    // layer composes the visible rows AFTER followed/blacklisted/dismissed
    // filtering + cross-rail dedup (compose_artist_rails) and keeps the
    // remaining validated candidates as backfill overflow — they ride the
    // results blob, so replacements need no extra network.
    let take = pool.len();
    rotate_take(pool, inputs.rotation_seed, take)
}

/// "More like the artists you love" — your COMMON taste (overall top).
pub async fn build_rec_artists_common(
    inputs: &RecoInputs<'_>,
    history: &ExtHistory,
) -> Vec<ArtistReco> {
    similar_artist_row(inputs, history, "overall").await
}

/// "Based on what you've been into lately" — your RECENT taste (1-month top).
pub async fn build_rec_artists_recent(
    inputs: &RecoInputs<'_>,
    history: &ExtHistory,
) -> Vec<ArtistReco> {
    similar_artist_row(inputs, history, "1month").await
}

// ── Display composition (filter + cross-rail dedup + backfill split) ────────

/// One artist rail after display composition: the visible rows (the first
/// `cap` survivors) plus the retained overflow (the remaining survivors, in
/// pool order), which the paint layer keeps for live backfill after a
/// "not interested" dismissal.
#[derive(Debug, Clone, Default)]
pub struct ArtistRailComposition {
    pub visible: Vec<ArtistReco>,
    pub overflow: Vec<ArtistReco>,
}

/// Compose the two Recommended-Artist rails for display:
///
/// - drop every candidate whose Qobuz id is in `excluded` (the frontend folds
///   followed / blacklisted / dismissed ids into that set; fail-soft = pass
///   an empty set),
/// - dedup WITHIN and ACROSS rails: an id shows in at most one rail — the
///   COMMON rail is composed first and wins — and overflow ids are claimed
///   too, so a common-overflow id cannot resurface in recent's visible,
/// - the first `cap` survivors per rail are visible; the rest is overflow.
pub fn compose_artist_rails(
    common_pool: Vec<ArtistReco>,
    recent_pool: Vec<ArtistReco>,
    excluded: &HashSet<u64>,
    cap: usize,
) -> (ArtistRailComposition, ArtistRailComposition) {
    let mut seen: HashSet<u64> = HashSet::new();
    let common = compose_one_rail(common_pool, excluded, &mut seen, cap);
    let recent = compose_one_rail(recent_pool, excluded, &mut seen, cap);
    (common, recent)
}

fn compose_one_rail(
    pool: Vec<ArtistReco>,
    excluded: &HashSet<u64>,
    seen: &mut HashSet<u64>,
    cap: usize,
) -> ArtistRailComposition {
    let mut out = ArtistRailComposition::default();
    for reco in pool {
        if excluded.contains(&reco.qobuz_artist_id) || !seen.insert(reco.qobuz_artist_id) {
            continue;
        }
        if out.visible.len() < cap {
            out.visible.push(reco);
        } else {
            out.overflow.push(reco);
        }
    }
    out
}

// ── Recommended Albums (Last.fm: your artists' top albums, not scrobbled) ───

pub async fn build_rec_albums(inputs: &RecoInputs<'_>, history: &ExtHistory) -> Vec<AlbumReco> {
    let Some(lf) = &inputs.lastfm else {
        return Vec::new();
    };
    // Lifetime top artists (not 1-month) for VOLUME: Recommended Albums shows
    // one album per artist, so we need many distinct artists to clear >=20
    // after Qobuz-catalog validation (the recent-taste rows cover 1-month).
    let artists: Vec<String> = lf
        .client
        .get_top_artists(&lf.username, "overall", 60)
        .await
        .unwrap_or_default()
        .into_iter()
        .take(KNOWN_ARTISTS_PER_BUILD)
        .map(|a| a.name)
        .collect();

    let per_artist: Vec<(String, Vec<qbz_integrations::lastfm::LastFmAlbum>)> =
        stream::iter(artists.into_iter().map(|name| {
            let lf = lf;
            async move {
                let albums = lf.client.get_artist_top_albums(&name, 6).await.unwrap_or_default();
                (name, albums)
            }
        }))
        .buffered(4)
        .collect()
        .await;

    let mut seen: HashSet<String> = HashSet::new();
    let mut candidates: Vec<AlbumCandidate> = Vec::new();
    for (artist, albums) in per_artist {
        for al in albums {
            let k = album_key(&al.artist, &al.name);
            if history.album_keys.contains(&k) || is_slop(&al.artist, &al.name) || !seen.insert(k) {
                continue;
            }
            candidates.push(AlbumCandidate {
                artist: al.artist.clone(),
                title: al.name,
                upc: None,
                source: RecoSource::LastFm,
                score: al.playcount as f32,
                subtitle: format!("From {} — you haven't heard this one", artist),
            });
            // One album per artist (owner request): take this artist's top
            // not-yet-heard album and move on, so the row spans >=20 artists.
            break;
        }
    }
    candidates.truncate(60);
    let pool = validate_album_pool(inputs.catalog, inputs.cache, candidates).await;
    rotate_take(pool, inputs.rotation_seed, DISPLAY_CAP)
}

// ── Seeded similar albums (album page: similar to THIS album) ───────────────

/// "Albums similar to this one" for the album page. There is no Last.fm
/// album-similarity endpoint, so we replicate it from artist similarity:
/// `seed_artist` -> artist.getSimilar -> one top (not-slop, not-excluded) album
/// per similar artist -> resolve to the Qobuz catalog. `exclude_pairs` are the
/// (artist, title) of albums already shown by the Qobuz `/album/suggest` row,
/// so the two carousels don't overlap. Empty when Last.fm is not connected.
pub async fn build_similar_albums_seeded(
    inputs: &RecoInputs<'_>,
    seed_artist: &str,
    exclude_pairs: &[(String, String)],
) -> Vec<AlbumReco> {
    let Some(lf) = &inputs.lastfm else {
        return Vec::new();
    };
    if seed_artist.trim().is_empty() {
        return Vec::new();
    }
    let exclude_keys: HashSet<String> =
        exclude_pairs.iter().map(|(a, t)| album_key(a, t)).collect();
    let seed_key = normalize(seed_artist);

    let sims = lf
        .client
        .get_similar_artists(seed_artist, 30)
        .await
        .unwrap_or_default();

    let per_artist: Vec<(String, Vec<qbz_integrations::lastfm::LastFmAlbum>)> =
        stream::iter(sims.into_iter().map(|s| {
            let lf = lf;
            async move {
                let albums = lf.client.get_artist_top_albums(&s.name, 6).await.unwrap_or_default();
                (s.name, albums)
            }
        }))
        .buffered(4)
        .collect()
        .await;

    let mut seen: HashSet<String> = HashSet::new();
    let mut candidates: Vec<AlbumCandidate> = Vec::new();
    for (artist, albums) in per_artist {
        // Skip a similar artist that IS the seed (self-similarity edge case).
        if normalize(&artist) == seed_key {
            continue;
        }
        for al in albums {
            let k = album_key(&al.artist, &al.name);
            if exclude_keys.contains(&k) || is_slop(&al.artist, &al.name) || !seen.insert(k) {
                continue;
            }
            candidates.push(AlbumCandidate {
                artist: al.artist.clone(),
                title: al.name,
                upc: None,
                source: RecoSource::LastFm,
                score: al.playcount as f32,
                subtitle: format!("Similar to {artist}"),
            });
            // One album per similar artist, so the row spans many artists.
            break;
        }
    }
    candidates.truncate(40);
    let pool = validate_album_pool(inputs.catalog, inputs.cache, candidates).await;
    rotate_take(pool, inputs.rotation_seed, DISPLAY_CAP)
}

// ── Fresh Releases (ListenBrainz, from artists you follow) ──────────────────

pub async fn build_fresh_releases(inputs: &RecoInputs<'_>) -> Vec<AlbumReco> {
    let Some(lb) = &inputs.listenbrainz else {
        return Vec::new();
    };
    let releases = lb.client.get_fresh_releases(&lb.username, 30).await.unwrap_or_default();
    let candidates: Vec<AlbumCandidate> = releases
        .into_iter()
        .filter(|r| {
            !r.release_name.is_empty()
                && !r.artist_credit_name.is_empty()
                && !is_slop(&r.artist_credit_name, &r.release_name)
        })
        .take(50)
        .map(|r| AlbumCandidate {
            artist: r.artist_credit_name,
            title: r.release_name,
            upc: None,
            source: RecoSource::ListenBrainz,
            score: 0.0,
            subtitle: r
                .release_date
                .map(|d| format!("New release · {}", d))
                .unwrap_or_else(|| "New release".to_string()),
        })
        .collect();
    let pool = validate_album_pool(inputs.catalog, inputs.cache, candidates).await;
    rotate_take(pool, inputs.rotation_seed, DISPLAY_CAP)
}

// ── Weekly playlists (ListenBrainz curated: exploration / jams) ─────────────
//
// These have their OWN ListenBrainz cadence: a brand-new playlist (new mbid +
// date) every Monday. They are cached per-week, keyed by the playlist mbid, and
// are DELIBERATELY decoupled from the shared 48h results blob — bundling them in
// it is what made them vanish (a transient empty build got cached for 48h, and
// the 7d per-track negative cache compounded it across rebuilds). See cache.rs.

/// Last resort when the current week can't be built (no playlist returned, or a
/// transient empty resolve): the most recent successfully-cached week, so the
/// row shows something instead of disappearing.
fn cached_weekly_fallback(cache: Cache<'_>, source_patch: &str) -> Vec<TrackReco> {
    cache
        .and_then(|c| c.lock().ok().and_then(|g| g.get_latest_weekly_for_patch(source_patch)))
        .and_then(|json| serde_json::from_str::<Vec<TrackReco>>(&json).ok())
        .unwrap_or_default()
}

pub async fn build_weekly(inputs: &RecoInputs<'_>, source_patch: &str) -> Vec<TrackReco> {
    let Some(lb) = &inputs.listenbrainz else {
        log::info!("[reco] weekly '{source_patch}': ListenBrainz not connected — skipping");
        return Vec::new();
    };

    // Discover the current week's playlist for this patch (one cheap call).
    let playlists = lb
        .client
        .get_created_for_playlists(&lb.username, 50)
        .await
        .unwrap_or_default();
    let matching = playlists
        .iter()
        .filter(|p| p.source_patch.as_deref() == Some(source_patch))
        .count();
    log::info!(
        "[reco] weekly '{source_patch}': {} created-for playlists from ListenBrainz, {matching} match the patch",
        playlists.len()
    );
    // Newest playlist matching the source_patch (created_at desc).
    let chosen = playlists
        .into_iter()
        .filter(|p| p.source_patch.as_deref() == Some(source_patch))
        .max_by(|a, b| a.created_at.cmp(&b.created_at));
    let Some(meta) = chosen else {
        log::warn!(
            "[reco] weekly '{source_patch}': ListenBrainz returned no matching playlist \
             (rate-limit / not generated yet) — serving last cached week"
        );
        return cached_weekly_fallback(inputs.cache, source_patch);
    };
    let week = meta.created_at.as_deref().unwrap_or("?");

    // Week-keyed cache: the mbid changes every Monday, so a new week is a natural
    // miss and the current week is served instantly (no Qobuz/MusicBrainz round-trips).
    let cache_key = format!("{}:{}", source_patch, meta.playlist_mbid);
    if let Some(c) = inputs.cache {
        if let Some(json) = c.lock().ok().and_then(|g| g.get_weekly(&cache_key)) {
            if let Ok(tracks) = serde_json::from_str::<Vec<TrackReco>>(&json) {
                if !tracks.is_empty() {
                    log::info!(
                        "[reco] weekly '{source_patch}': cache hit — {} tracks (week {week})",
                        tracks.len()
                    );
                    return tracks;
                }
            }
        }
    }

    // Cache miss for this week: fetch + resolve to Qobuz.
    let raw = lb
        .client
        .get_playlist_tracks(&meta.playlist_mbid)
        .await
        .unwrap_or_default();
    let candidates: Vec<TrackCandidate> = raw
        .into_iter()
        .filter(|t| !t.title.is_empty() && !t.artist_name.is_empty())
        .map(|t| TrackCandidate {
            artist: t.artist_name,
            title: t.title,
            album: t.release_name,
            duration_ms: None,
            isrc: None,
            recording_mbid: t.recording_mbid,
            source: RecoSource::ListenBrainz,
            score: 0.0,
        })
        .collect();
    let cand_count = candidates.len();
    log::info!(
        "[reco] weekly '{source_patch}': fetched {cand_count} tracks (week {week}, mbid {}); resolving to Qobuz…",
        meta.playlist_mbid
    );

    // skip_negative=true: a transient throttle on these tracks must NOT stick as
    // a 7-day negative (that locked the rows empty across rebuilds).
    // skip_mb=true: bypass the serial 1.1s/req MusicBrainz ISRC lookup (~110s
    // for 100 tracks) and resolve via fuzzy Qobuz search — fast and reliable
    // for these mainstream playlists. This is the fix for "the row never paints".
    let pool = validate_track_pool(
        inputs.catalog,
        inputs.musicbrainz,
        inputs.cache,
        candidates,
        true,
        true,
    )
    .await;
    let resolved: Vec<TrackReco> = pool.into_iter().take(PLAYLIST_CAP).collect();
    log::info!(
        "[reco] weekly '{source_patch}': resolved {} / {cand_count} tracks to Qobuz (week {week}, mbid {})",
        resolved.len(),
        meta.playlist_mbid
    );

    if !resolved.is_empty() {
        // Persist the resolved set for this week (only when non-empty).
        if let Some(c) = inputs.cache {
            if let (Ok(g), Ok(json)) = (c.lock(), serde_json::to_string(&resolved)) {
                g.put_weekly(&cache_key, source_patch, &json);
            }
        }
        return resolved;
    }

    // Resolved empty this build (transient) — show last cached week, not nothing.
    log::warn!(
        "[reco] weekly '{source_patch}': resolved 0 tracks this build — serving last cached week"
    );
    cached_weekly_fallback(inputs.cache, source_patch)
}

// ── Deep-cut albums from artists you know (Qobuz catalog, not heard) ────────

pub async fn build_deep_cut_albums(inputs: &RecoInputs<'_>) -> Vec<AlbumReco> {
    if inputs.local.known_artist_ids.is_empty() {
        return Vec::new();
    }
    let mut ids: Vec<u64> = inputs.local.known_artist_ids.iter().copied().collect();
    ids.sort_unstable();
    let ids = rotate_take(ids, inputs.rotation_seed, KNOWN_ARTISTS_PER_BUILD);

    let per_artist: Vec<Vec<Album>> = stream::iter(ids.into_iter().map(|id| {
        let catalog = inputs.catalog;
        async move { catalog.artist_albums(id, 12).await }
    }))
    .buffered(4)
    .collect()
    .await;

    let mut seen: HashSet<String> = HashSet::new();
    let mut pool: Vec<AlbumReco> = Vec::new();
    for albums in per_artist {
        for album in albums.into_iter().skip(2) {
            if album.id.is_empty()
                || !is_full_album(&album)
                || is_slop(&album.artist.name, &album.title)
                || inputs.local.played_album_ids.contains(&album.id)
                || !seen.insert(album.id.clone())
            {
                continue;
            }
            let subtitle = format!("Deep cut · {}", album.artist.name);
            pool.push(build_album_reco(&album, subtitle, RecoSource::Internal));
        }
    }
    rotate_take(pool, inputs.rotation_seed, DISPLAY_CAP)
}

// ── Cold-start editorial (top albums + artists) ─────────────────────────────

pub async fn build_editorial(inputs: &RecoInputs<'_>) -> (Vec<AlbumReco>, Vec<ArtistReco>) {
    let (most_streamed, new_releases) = tokio::join!(
        inputs.catalog.featured_albums("most-streamed", 20),
        inputs.catalog.featured_albums("new-releases", 20),
    );

    let mut seen_albums: HashSet<String> = HashSet::new();
    let mut top_albums: Vec<AlbumReco> = Vec::new();
    for album in most_streamed.iter().chain(new_releases.iter()) {
        if !album.id.is_empty() && seen_albums.insert(album.id.clone()) {
            top_albums.push(build_album_reco(album, String::new(), RecoSource::Editorial));
        }
    }
    top_albums.truncate(20);

    let mut seen_artists: HashSet<u64> = HashSet::new();
    let mut artist_ids: Vec<(u64, String)> = Vec::new();
    for album in most_streamed.iter().chain(new_releases.iter()) {
        let id = album.artist.id;
        if id != 0 && seen_artists.insert(id) {
            artist_ids.push((id, album.artist.name.clone()));
        }
        if artist_ids.len() >= 12 {
            break;
        }
    }
    let top_artists: Vec<ArtistReco> = stream::iter(artist_ids.into_iter().map(|(id, name)| {
        let catalog = inputs.catalog;
        async move {
            let image_url = catalog
                .get_artist(id)
                .await
                .and_then(|a| a.image.and_then(|i| i.best().cloned()))
                .unwrap_or_default();
            ArtistReco {
                qobuz_artist_id: id,
                name,
                image_url,
                subtitle: String::new(),
                source: RecoSource::Editorial,
            }
        }
    }))
    .buffered(4)
    .collect()
    .await;

    (top_albums, top_artists)
}


// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn reco(id: u64) -> ArtistReco {
        ArtistReco {
            qobuz_artist_id: id,
            name: format!("Artist {id}"),
            image_url: String::new(),
            subtitle: String::new(),
            source: RecoSource::LastFm,
        }
    }

    fn ids(recos: &[ArtistReco]) -> Vec<u64> {
        recos.iter().map(|r| r.qobuz_artist_id).collect()
    }

    #[test]
    fn compose_excludes_ids_from_visible_and_overflow() {
        let common = vec![reco(1), reco(2), reco(3), reco(4)];
        let recent = vec![reco(5), reco(6)];
        let excluded = HashSet::from([2, 4, 5]);
        let (c, r) = compose_artist_rails(common, recent, &excluded, 2);
        assert_eq!(ids(&c.visible), vec![1, 3]);
        assert!(c.overflow.is_empty(), "excluded ids leave no overflow");
        assert_eq!(ids(&r.visible), vec![6]);
    }

    #[test]
    fn compose_dedups_cross_rail_common_wins() {
        let common = vec![reco(1), reco(2)];
        let recent = vec![reco(2), reco(3)];
        let (c, r) = compose_artist_rails(common, recent, &HashSet::new(), 20);
        assert_eq!(ids(&c.visible), vec![1, 2]);
        assert_eq!(ids(&r.visible), vec![3], "id 2 appears only in common");
    }

    #[test]
    fn compose_dedups_within_rail() {
        let common = vec![reco(1), reco(1), reco(2)];
        let (c, _r) = compose_artist_rails(common, Vec::new(), &HashSet::new(), 20);
        assert_eq!(ids(&c.visible), vec![1, 2]);
    }

    #[test]
    fn compose_splits_visible_and_overflow_in_order() {
        let common: Vec<ArtistReco> = (1..=5).map(reco).collect();
        let excluded = HashSet::from([1]);
        let (c, _r) = compose_artist_rails(common, Vec::new(), &excluded, 3);
        // The exclusion punches a hole in the first window; the next
        // candidates move up into the visible rows, the rest is overflow —
        // both in pool order (the backfill contract).
        assert_eq!(ids(&c.visible), vec![2, 3, 4]);
        assert_eq!(ids(&c.overflow), vec![5]);
    }

    #[test]
    fn compose_cross_rail_dedup_also_covers_overflow() {
        // An id sitting in common's OVERFLOW must not resurface in recent's
        // visible rows (it is still claimed for potential backfill).
        let common: Vec<ArtistReco> = (1..=3).map(reco).collect();
        let recent = vec![reco(3), reco(4)];
        let (c, r) = compose_artist_rails(common, recent, &HashSet::new(), 2);
        assert_eq!(ids(&c.visible), vec![1, 2]);
        assert_eq!(ids(&c.overflow), vec![3]);
        assert_eq!(ids(&r.visible), vec![4], "id 3 is claimed by common");
        assert!(r.overflow.is_empty());
    }
}
