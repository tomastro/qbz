//! Frontend-agnostic cross-platform music-link resolver.
//!
//! Extracted from `src-tauri/src/commands_v2/link_resolver.rs` so any frontend
//! (Tauri, Slint, TUI/CLI) can resolve music links without depending on
//! `src-tauri` (ADR-006). Accepts URLs from Qobuz, Spotify, Apple Music, Tidal,
//! Deezer, song.link, and album.link. For non-Qobuz tracks/albums it identifies
//! the content (direct platform API fast-path, else the Odesli API) and searches
//! Qobuz by title+artist to find the equivalent. For playlists it returns
//! `PlaylistDetected` so the frontend can redirect to its importer.
//!
//! The Qobuz search itself is decoupled via the [`QobuzSearchBridge`] trait —
//! the frontend implements it over its own core and passes a `&dyn` to
//! [`resolve_music_link`].

mod bridge;
mod detection;
mod errors;
mod fast_path;
mod odesli;
mod qobuz_search;

// ── Public API surface ──

pub use bridge::QobuzSearchBridge;
pub use detection::{detect_music_resource, MusicProvider, MusicResource};
pub use errors::MusicLinkError;
pub use odesli::{ContentType, ShareError, SongLinkClient};
pub use qobuz_search::MusicLinkResult;

// Re-export the native Qobuz parser so frontends can do native parsing too,
// without taking a direct `qbz-qobuz` dependency just for this.
pub use qbz_qobuz::{resolve_link, LinkResolverError, ResolvedLink};

use detection::MusicProvider as Provider;

/// Resolve a cross-platform music link to a Qobuz navigation action.
///
/// Accepts URLs from Qobuz, Spotify, Apple Music, Tidal, Deezer, song.link, and
/// album.link. For non-Qobuz tracks/albums, uses the Odesli API (or a direct
/// platform fast-path) to identify the content, then searches Qobuz by
/// title+artist to find the equivalent album. For playlists, returns
/// `PlaylistDetected` so the frontend can redirect to the importer.
pub async fn resolve_music_link(
    url: &str,
    songlink: &SongLinkClient,
    bridge: &dyn QobuzSearchBridge,
) -> Result<MusicLinkResult, MusicLinkError> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err(MusicLinkError::EmptyUrl);
    }

    // 1. Try Qobuz native resolve first (sync, no network)
    if let Ok(resolved) = qbz_qobuz::resolve_link(&url) {
        return Ok(MusicLinkResult::Resolved {
            link: resolved,
            provider: None,
        });
    }

    // 2. Detect what kind of resource this is
    let resource = detect_music_resource(&url).ok_or(MusicLinkError::Unsupported)?;

    match resource {
        MusicResource::Qobuz => {
            // Already handled above, but just in case
            let resolved = qbz_qobuz::resolve_link(&url)
                .map_err(|e| MusicLinkError::Internal(e.to_string()))?;
            Ok(MusicLinkResult::Resolved {
                link: resolved,
                provider: None,
            })
        }

        MusicResource::Playlist { provider } => Ok(MusicLinkResult::PlaylistDetected {
            provider: format!("{:?}", provider),
        }),

        MusicResource::Track {
            provider,
            url: source_url,
        } => {
            resolve_via_odesli_and_search(songlink, &source_url, Some(&provider), true, bridge).await
        }

        MusicResource::Album {
            provider,
            url: source_url,
        } => {
            resolve_via_odesli_and_search(songlink, &source_url, Some(&provider), false, bridge)
                .await
        }

        MusicResource::SongLink { url: source_url } => {
            // song.link URLs: try to detect track vs album from the URL format
            let is_track_hint = source_url.contains("song.link/");
            resolve_via_odesli_and_search(songlink, &source_url, None, is_track_hint, bridge).await
        }
    }
}

/// Identify a cross-platform music URL and search Qobuz for the equivalent.
///
/// Fast path: for Tidal/Deezer calls the platform API directly; for Spotify
/// scrapes the embed page to get title+artist. Fallback: uses Odesli API (~2-3s).
/// Then searches Qobuz with progressively simpler queries.
async fn resolve_via_odesli_and_search(
    songlink: &SongLinkClient,
    url: &str,
    provider: Option<&Provider>,
    is_track: bool,
    bridge: &dyn QobuzSearchBridge,
) -> Result<MusicLinkResult, MusicLinkError> {
    let provider_name = provider.map(|p| format!("{:?}", p));

    // 1. Get title + artist: try direct platform API first (fast), fall back to Odesli
    let (title, artist) = if let Some(prov) = provider {
        match fast_path::try_direct_platform_metadata(url, prov, is_track).await {
            Some(meta) => {
                log::info!(
                    "Link resolver: direct API resolved '{}' by '{}'",
                    meta.0,
                    meta.1
                );
                meta
            }
            None => {
                log::info!("Link resolver: direct API failed, falling back to Odesli");
                fetch_metadata_via_odesli(songlink, url).await?
            }
        }
    } else {
        // No provider (song.link URLs) — use Odesli
        fetch_metadata_via_odesli(songlink, url).await?
    };

    if title.is_empty() {
        return Ok(MusicLinkResult::NotOnQobuz {
            provider: provider_name,
        });
    }

    // 2. Search Qobuz with progressively simpler queries
    if let Some(result) =
        qobuz_search::search_qobuz_smart(bridge, &title, &artist, is_track, &provider_name).await?
    {
        return Ok(result);
    }

    log::info!(
        "Link resolver: '{}' by '{}' not found on Qobuz",
        title,
        artist
    );
    Ok(MusicLinkResult::NotOnQobuz {
        provider: provider_name,
    })
}

/// Fetch metadata from Odesli API (with one retry for transient errors).
async fn fetch_metadata_via_odesli(
    songlink: &SongLinkClient,
    url: &str,
) -> Result<(String, String), MusicLinkError> {
    let response = match songlink.get_by_url(url, ContentType::Track).await {
        Ok(r) => r,
        Err(first_err) => {
            log::warn!(
                "Link resolver: Odesli first attempt failed: {}, retrying...",
                first_err
            );
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            songlink
                .get_by_url(url, ContentType::Track)
                .await
                .map_err(|e| MusicLinkError::Internal(format!("Odesli API error: {}", e)))?
        }
    };

    let title = response.title.unwrap_or_default().trim().to_string();
    let artist = response.artist.unwrap_or_default().trim().to_string();
    Ok((title, artist))
}
