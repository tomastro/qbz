//! Orchestrates playlist import

use std::sync::Arc;

use qbz_qobuz::QobuzClient;

use crate::errors::PlaylistImportError;
use crate::match_qobuz::match_tracks;
use crate::models::{ImportPlaylist, ImportProgress, ImportSummary};
use crate::providers::{detect_provider, fetch_playlist};
use crate::sink::{ImportEvent, ImportPhase, ImportProgressSink};

const ADD_CHUNK_SIZE: usize = 50;
const QOBUZ_PLAYLIST_TRACK_LIMIT: usize = 2000;

pub async fn preview_public_playlist(url: &str) -> Result<ImportPlaylist, PlaylistImportError> {
    let provider = detect_provider(url)?;
    fetch_playlist(provider).await
}

/// COMPAT SHIM — the pre-expansion signature, byte-for-byte. Every external
/// and test caller keeps working; it is now just "resolve the URL, then import
/// what came back".
pub async fn import_public_playlist(
    url: &str,
    client: &QobuzClient,
    name_override: Option<&str>,
    is_public: bool,
    progress: Arc<dyn ImportProgressSink>,
) -> Result<ImportSummary, PlaylistImportError> {
    let playlist = preview_public_playlist(url).await?;
    import_prepared_playlist(playlist, client, name_override, is_public, progress).await
}

/// The IMPORT half, SOURCE-AGNOSTIC: match against the Qobuz catalog, create
/// the playlist (splitting past 2000 tracks) and add the matches.
///
/// It takes an already-resolved [`ImportPlaylist`] rather than a URL, which is
/// what lets a file, a JSON blob or a service feed the same pipeline. The body
/// below is the pre-expansion `import_public_playlist` unchanged from the
/// matching phase down.
///
/// TAKING THE PLAYLIST BY VALUE IS THE POINT. The Tauri original re-fetched
/// from the URL at execute time, and this port inherited that: it scraped the
/// same page twice per import. Worse, the pattern cannot express the new
/// sources at all — the `rfd` bytes of a picked file are gone by then and the
/// dialog cannot silently reopen. The caller snapshots the previewed playlist
/// and hands it over; the double-scrape dies with it.
pub async fn import_prepared_playlist(
    playlist: ImportPlaylist,
    client: &QobuzClient,
    name_override: Option<&str>,
    is_public: bool,
    progress: Arc<dyn ImportProgressSink>,
) -> Result<ImportSummary, PlaylistImportError> {
    // Phase: matching
    progress.emit(ImportEvent::Phase(ImportPhase::Matching));
    let matches = match_tracks(client, &playlist.tracks, Arc::clone(&progress)).await?;

    let mut matched_track_ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in &matches {
        if let Some(id) = entry.qobuz_track_id {
            if seen.insert(id) {
                matched_track_ids.push(id);
            }
        }
    }

    let matched_count = matched_track_ids.len() as u32;
    let total_tracks = playlist.tracks.len() as u32;
    // THREE DISJOINT NUMBERS, and they sum to `total_tracks`.
    //
    // `skipped` is counted from the matches themselves — rows that found
    // nothing — instead of being inferred as `total - matched`. The inferred
    // form folded duplicates into failures, and on a source that repeats
    // (a Last.fm radio draw, a JSON export with the same track twice) that made
    // the summary actively wrong: 469 rows, 453 matches, 198 distinct, and the
    // user was told 271 tracks had been skipped.
    let skipped_tracks = matches
        .iter()
        .filter(|m| m.qobuz_track_id.is_none())
        .count() as u32;
    let duplicate_tracks = total_tracks
        .saturating_sub(matched_count)
        .saturating_sub(skipped_tracks);

    let mut qobuz_playlist_ids = Vec::new();

    if !matched_track_ids.is_empty() {
        let base_name = name_override.unwrap_or(&playlist.name);
        let description = playlist
            .description
            .clone()
            .or_else(|| Some(format!("Imported from {}", playlist.provider.as_str())));

        // Split into parts if more than QOBUZ_PLAYLIST_TRACK_LIMIT tracks
        let parts: Vec<&[u64]> = matched_track_ids
            .chunks(QOBUZ_PLAYLIST_TRACK_LIMIT)
            .collect();
        let total_parts = parts.len();

        for (part_idx, part_tracks) in parts.iter().enumerate() {
            // Phase: creating (per part)
            progress.emit(ImportEvent::Phase(ImportPhase::Creating));

            let playlist_name = if total_parts == 1 {
                base_name.to_string()
            } else {
                format!("{} (Part {})", base_name, part_idx + 1)
            };

            let part_desc = if total_parts == 1 {
                description.clone()
            } else {
                Some(format!(
                    "Part {} of {} — {}",
                    part_idx + 1,
                    total_parts,
                    description.as_deref().unwrap_or("")
                ))
            };

            let created = client
                .create_playlist(&playlist_name, part_desc.as_deref(), is_public)
                .await
                .map_err(|e| PlaylistImportError::Qobuz(e.to_string()))?;

            qobuz_playlist_ids.push(created.id);

            // Phase: adding
            progress.emit(ImportEvent::Phase(ImportPhase::Adding));

            let chunks: Vec<&[u64]> = part_tracks.chunks(ADD_CHUNK_SIZE).collect();
            let total_chunks = chunks.len() as u32;

            for (i, chunk) in chunks.iter().enumerate() {
                client
                    .add_tracks_to_playlist(created.id, chunk)
                    .await
                    .map_err(|e| PlaylistImportError::Qobuz(e.to_string()))?;

                progress.emit(ImportEvent::Progress(ImportProgress {
                    phase: "adding".to_string(),
                    current: (i as u32) + 1,
                    total: total_chunks,
                    matched_so_far: matched_count,
                    current_track: if total_parts > 1 {
                        Some(format!("Part {}/{}", part_idx + 1, total_parts))
                    } else {
                        None
                    },
                }));
            }
        }
    }

    let parts_created = qobuz_playlist_ids.len() as u32;

    Ok(ImportSummary {
        provider: playlist.provider,
        // Deliberate fix vs the Tauri original (owner decision): the summary
        // reports the name the playlist was actually created under — the
        // rename when one was given — not the original source name.
        playlist_name: match name_override {
            Some(name) => name.to_string(),
            None => playlist.name,
        },
        total_tracks,
        matched_tracks: matched_count,
        skipped_tracks,
        duplicate_tracks,
        qobuz_playlist_ids,
        parts_created,
        matches,
    })
}

#[cfg(test)]
mod tests {
    use crate::models::{ImportTrack, TrackMatch};

    fn m(id: Option<u64>) -> TrackMatch {
        TrackMatch {
            source: ImportTrack {
                title: "t".into(),
                artist: "a".into(),
                album: None,
                duration_ms: None,
                isrc: None,
                provider_id: None,
                provider_url: None,
            },
            qobuz_track_id: id,
            qobuz_title: None,
            qobuz_artist: None,
            score: 1.0,
        }
    }

    /// The arithmetic `import_prepared_playlist` performs on its match list,
    /// isolated from the network. THE INVARIANT: matched + skipped + duplicates
    /// == total, and each means exactly one thing.
    ///
    /// The shape is the real Last.fm radio import that exposed the bug — many
    /// rows, most matching, heavy repetition — scaled down.
    #[test]
    fn the_three_counts_are_disjoint_and_sum_to_the_total() {
        // 6 rows: ids 1,1,2,2,2 and one miss.
        let matches = vec![
            m(Some(1)),
            m(Some(1)),
            m(Some(2)),
            m(Some(2)),
            m(Some(2)),
            m(None),
        ];
        let total = matches.len() as u32;

        let mut seen = std::collections::HashSet::new();
        let unique = matches
            .iter()
            .filter_map(|e| e.qobuz_track_id)
            .filter(|id| seen.insert(*id))
            .count() as u32;
        let skipped = matches
            .iter()
            .filter(|e| e.qobuz_track_id.is_none())
            .count() as u32;
        let duplicates = total.saturating_sub(unique).saturating_sub(skipped);

        assert_eq!(unique, 2, "two distinct tracks reach the playlist");
        assert_eq!(skipped, 1, "one row matched nothing");
        assert_eq!(duplicates, 3, "three rows repeated an already-added track");
        assert_eq!(unique + skipped + duplicates, total);
        // The OLD formula would have called all four non-unique rows skipped.
        assert_ne!(total - unique, skipped);
    }
}
