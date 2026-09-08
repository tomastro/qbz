//! Smart Qobuz search: progressively simpler queries until a match is found.
//!
//! Ported verbatim from the `src-tauri` link resolver, with the search calls
//! routed through the frontend-agnostic `QobuzSearchBridge`. The 4th
//! `search_type: Option<&str>` arg of the original (always `None`) is dropped
//! from the bridge signature.

use crate::bridge::QobuzSearchBridge;
use crate::errors::MusicLinkError;

/// Result of resolving a cross-platform music link.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum MusicLinkResult {
    /// Successfully resolved to a Qobuz entity.
    Resolved {
        link: qbz_qobuz::ResolvedLink,
        provider: Option<String>,
    },
    /// The URL is a playlist — redirect to the Playlist Importer.
    PlaylistDetected { provider: String },
    /// The content exists on the source platform but is not available on Qobuz.
    NotOnQobuz { provider: Option<String> },
}

/// Search Qobuz with progressively simpler queries until a match is found.
///
/// Strategy:
/// 1. "title artist" (exact)
/// 2. "cleaned_title artist" (remove parenthetical/bracket suffixes)
/// 3. "artist" only with album search (broad)
pub(crate) async fn search_qobuz_smart(
    bridge: &dyn QobuzSearchBridge,
    title: &str,
    artist: &str,
    is_track: bool,
    provider_name: &Option<String>,
) -> Result<Option<MusicLinkResult>, MusicLinkError> {
    let full_query = if artist.is_empty() {
        title.to_string()
    } else {
        format!("{} {}", title, artist)
    };

    // Attempt 1: full query
    if is_track {
        let results = bridge
            .search_tracks(&full_query, 5, 0)
            .await
            .map_err(MusicLinkError::Internal)?;
        if let Some(track) = results.items.first() {
            log::info!(
                "Link resolver: found Qobuz track id={} (full query)",
                track.id
            );
            return Ok(Some(MusicLinkResult::Resolved {
                link: qbz_qobuz::ResolvedLink::OpenTrack(track.id),
                provider: provider_name.clone(),
            }));
        }
    }

    let results = bridge
        .search_albums(&full_query, 5, 0)
        .await
        .map_err(MusicLinkError::Internal)?;
    if let Some(album) = results.items.first() {
        log::info!(
            "Link resolver: found Qobuz album id={} (full query)",
            album.id
        );
        return Ok(Some(MusicLinkResult::Resolved {
            link: qbz_qobuz::ResolvedLink::OpenAlbum(album.id.clone()),
            provider: provider_name.clone(),
        }));
    }

    // Attempt 2: clean title (remove parenthetical/bracket suffixes like "Remastered", "Deluxe")
    let cleaned = clean_title(title);
    if cleaned != title && !cleaned.is_empty() {
        let clean_query = if artist.is_empty() {
            cleaned.clone()
        } else {
            format!("{} {}", cleaned, artist)
        };

        log::info!(
            "Link resolver: retrying with cleaned query '{}'",
            clean_query
        );
        let results = bridge
            .search_albums(&clean_query, 5, 0)
            .await
            .map_err(MusicLinkError::Internal)?;
        if let Some(album) = results.items.first() {
            log::info!(
                "Link resolver: found Qobuz album id={} (cleaned query)",
                album.id
            );
            return Ok(Some(MusicLinkResult::Resolved {
                link: qbz_qobuz::ResolvedLink::OpenAlbum(album.id.clone()),
                provider: provider_name.clone(),
            }));
        }
    }

    // Attempt 3: search by artist name only (broad)
    if !artist.is_empty() && artist != title {
        log::info!(
            "Link resolver: retrying with artist-only query '{}'",
            artist
        );
        let results = bridge
            .search_albums(artist, 10, 0)
            .await
            .map_err(MusicLinkError::Internal)?;
        let title_lower = title.to_ascii_lowercase();
        let cleaned_lower = clean_title(title).to_ascii_lowercase();
        for album in &results.items {
            let album_title_lower = album.title.to_ascii_lowercase();
            if album_title_lower.contains(&cleaned_lower)
                || cleaned_lower.contains(&album_title_lower)
                || album_title_lower.contains(&title_lower)
            {
                log::info!(
                    "Link resolver: found Qobuz album id={} (artist-only + title match)",
                    album.id
                );
                return Ok(Some(MusicLinkResult::Resolved {
                    link: qbz_qobuz::ResolvedLink::OpenAlbum(album.id.clone()),
                    provider: provider_name.clone(),
                }));
            }
        }
    }

    Ok(None)
}

/// Remove parenthetical/bracket suffixes from a title.
/// "Senjutsu (2021 Remaster)" → "Senjutsu"
/// "The Number of the Beast [Deluxe Edition]" → "The Number of the Beast"
pub(crate) fn clean_title(title: &str) -> String {
    let mut result = title.to_string();
    // Remove trailing (...) and [...]
    while let Some(pos) = result.rfind('(') {
        if result[pos..].contains(')') {
            result = result[..pos].trim_end().to_string();
        } else {
            break;
        }
    }
    while let Some(pos) = result.rfind('[') {
        if result[pos..].contains(']') {
            result = result[..pos].trim_end().to_string();
        } else {
            break;
        }
    }
    result.trim().to_string()
}
