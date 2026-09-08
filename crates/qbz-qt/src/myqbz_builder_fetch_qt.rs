//! Discography Builder — the THREE source fetchers, split out of
//! `myqbz_builder_qt.rs` (which crossed the ~1200-line mandatory-split line of
//! the domain recipe, §T3). The classifier, the grouping, the selection maths
//! and the save flow stay in the parent module on purpose: they are one
//! algorithm and splitting mid-algorithm buys nothing (spec 02 §3).
//!
//! Both fetchers produce `myqbz_builder_qt::Candidate` and reuse the parent's
//! classifier (`normalize_title` / `title_is_compilation` / `classify_release` /
//! `quality_score`) — there is no second copy of any of it here.
//!
//! Ordering is load-bearing: Qobuz runs FIRST because it resolves the artist
//! NAME, and the local/Plex query filters on that name. A Qobuz failure still
//! allows the local half if a name is known; a DB failure degrades to empty and
//! the Qobuz half still works.

use std::sync::Arc;

use qbz_app::shell::AppRuntime;
use qbz_core::LoggingAdapter;
use qbz_library::album_grouping::AlbumGroupMode;

use crate::myqbz_builder_qt::{
    classify_release, normalize_title, quality_score, title_is_compilation, Candidate,
};

/// Fetch the artist's Qobuz releases. Returns `(candidates, artist_name,
/// avatar_url)`. Mirrors `fetchQobuzAlbums` — name + avatar are a side effect.
/// The avatar url is the MEDIUM portrait variant (the header renders a 72px
/// circle), 1:1 with `myqbz_builder.rs:316-323`.
pub async fn fetch_qobuz(
    runtime: &Arc<AppRuntime<LoggingAdapter>>,
    artist_id: &str,
) -> Result<(Vec<Candidate>, String, String), String> {
    let id: u64 = artist_id
        .parse()
        .map_err(|_| format!("invalid artist id: {artist_id}"))?;
    let page = runtime
        .core()
        .get_artist_page(id, None)
        .await
        .map_err(|e| e.to_string())?;

    let artist_name = page.name.display.clone();
    let avatar_url = page
        .images
        .as_ref()
        .and_then(|imgs| imgs.portrait.as_ref())
        .map(|p| {
            format!(
                "https://static.qobuz.com/images/artists/covers/medium/{}.{}",
                p.hash, p.format
            )
        })
        .unwrap_or_default();

    let mut out: Vec<Candidate> = Vec::new();
    for group in page.releases.into_iter().flatten() {
        let group_type = group.release_type.clone();
        for r in group.items.into_iter() {
            let year = r
                .dates
                .as_ref()
                .and_then(|d| d.original.as_deref())
                .and_then(|s| s.get(..4))
                .and_then(|y| y.parse::<i32>().ok());
            let artwork = r
                .image
                .as_ref()
                .and_then(|img| img.large.clone().or_else(|| img.best().cloned()));
            let track_count = r.tracks_count.map(|n| n as i32);
            let bit = r.audio_info.as_ref().and_then(|a| a.maximum_bit_depth);
            let rate = r.audio_info.as_ref().and_then(|a| a.maximum_sampling_rate);
            let title = r.title.clone();
            let title_comp = title_is_compilation(&title);
            let release_type = classify_release(
                &title,
                track_count,
                r.release_type.as_deref(),
                Some(&group_type),
                title_comp,
            );
            let is_comp = release_type == "compilation";
            let group_key = format!(
                "{}|{}",
                normalize_title(&title),
                year.map(|y| y.to_string()).unwrap_or_default()
            );
            out.push(Candidate {
                group_key,
                source: "qobuz".to_string(),
                source_item_id: r.id.clone(),
                title,
                artist: artist_name.clone(),
                year,
                artwork_url: artwork,
                artwork_path: String::new(),
                track_count,
                max_bit_depth: bit,
                max_sample_rate: rate,
                format: "FLAC".to_string(),
                is_compilation: is_comp,
                release_type,
                quality_score: quality_score(bit, rate, "FLAC"),
            });
        }
    }
    Ok((out, artist_name, avatar_url))
}

/// Whether a local album's `artist` / `all_artists` matches `artist_name`
/// (case-insensitive exact match on artist OR any comma-split all_artists entry).
fn matches_artist(artist: &str, all_artists: &str, artist_name: &str) -> bool {
    let needle = artist_name.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    if artist.trim().to_lowercase() == needle {
        return true;
    }
    all_artists
        .split(',')
        .any(|a| a.trim().to_lowercase() == needle)
}

/// Fetch local + Plex albums by the artist via the unified metadata-grouped
/// page (Plex union included when enabled). BLOCKING (DB) — the caller wraps it
/// in `spawn_blocking`. Degrades to empty on DB failure (logged by `with_db`).
/// `artist_name` MUST be resolved first (the Qobuz fetch sets it) — an empty
/// name drops every match.
pub fn fetch_local_and_plex(artist_name: &str) -> Vec<Candidate> {
    if artist_name.trim().is_empty() {
        return Vec::new();
    }
    let plex_path = crate::local_plex::cache_db_path();
    // The shared remote mirror + the enabled sources, same gates the Albums
    // tab uses — the builder must see the SAME set the grid shows.
    let remote_path = crate::media_servers_qt::remote_cache_path();
    let remote_words = crate::media_servers_qt::configured_words();
    // Map INSIDE the `with_db` closure so `db.resolve_album_cover_fallback` is
    // reachable (mirrors the Albums grid): the cover PATH rides on the
    // candidate's `artwork_url`, so the saved collection item carries it and the
    // detail rows render the real cover instead of the disc placeholder. Plex
    // rows arrive with a non-empty `/library/...` thumb path; local rows carry
    // `a.artwork_path`, with the same cover.jpg/folder.jpg on-disk fallback the
    // grid uses so a DB row missing artwork_path still resolves a cover.
    crate::local_state::with_db(move |db| {
        let page = db.get_albums_metadata_page(
            0,
            1_000_000,
            None,
            "artist",
            "asc",
            /* include_qobuz_downloads */ true,
            /* exclude_network_folders */
            crate::offline_fwd::exclude_network_folders_now(),
            plex_path.as_deref(),
            remote_path.as_deref(),
            &remote_words,
            // My QBZ collections are artist-scoped CANDIDATES, not the Albums
            // view — keep the metadata grouping regardless of the user's Local
            // Library identity-mode pref.
            AlbumGroupMode::Metadata,
        )?;
        let out: Vec<Candidate> = page
            .albums
            .into_iter()
            .filter(|a| matches_artist(&a.artist, &a.all_artists, artist_name))
            .map(|a| {
                let source = if a.source == "plex" { "plex" } else { "local" }.to_string();
                let year = a.year.map(|y| y as i32);
                let track_count = if a.track_count > 0 {
                    Some(a.track_count as i32)
                } else {
                    None
                };
                let bit = a.bit_depth;
                // LocalAlbum.sample_rate is Hz; the candidate carries kHz.
                let rate_khz = if a.sample_rate >= 1000.0 {
                    Some(a.sample_rate / 1000.0)
                } else if a.sample_rate > 0.0 {
                    Some(a.sample_rate)
                } else {
                    None
                };
                // Cover path: the row's own artwork_path, else the on-disk
                // cover.jpg/folder.jpg fallback (local only — Plex rows already
                // carry a non-empty thumb path so the fallback no-ops).
                let artwork_url = a
                    .artwork_path
                    .clone()
                    .filter(|p| !p.is_empty())
                    .or_else(|| db.resolve_album_cover_fallback(&a.id));
                let format = a.format.to_string();
                let title = a.title.clone();
                let title_comp = title_is_compilation(&title);
                let release_type = classify_release(&title, track_count, None, None, title_comp);
                let is_comp = release_type == "compilation";
                let group_key = format!(
                    "{}|{}",
                    normalize_title(&title),
                    year.map(|y| y.to_string()).unwrap_or_default()
                );
                Candidate {
                    group_key,
                    source,
                    source_item_id: a.id,
                    title,
                    artist: a.artist,
                    year,
                    artwork_url,
                    artwork_path: String::new(),
                    track_count,
                    max_bit_depth: bit,
                    max_sample_rate: rate_khz,
                    quality_score: quality_score(bit, rate_khz, &format),
                    format,
                    is_compilation: is_comp,
                    release_type,
                }
            })
            .collect();
        Ok(out)
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::matches_artist;

    #[test]
    fn matches_artist_is_case_insensitive_over_both_columns() {
        assert!(matches_artist("Radiohead", "", "radiohead"));
        assert!(matches_artist(
            "Various",
            "Foo, Radiohead , Bar",
            "RADIOHEAD"
        ));
        assert!(!matches_artist("Radioheads", "", "radiohead"));
        assert!(!matches_artist("Radiohead", "", "  "));
    }
}
