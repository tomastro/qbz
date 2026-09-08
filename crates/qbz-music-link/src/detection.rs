//! Music-URL detection and provider definitions.
//!
//! Ported from `src-tauri/src/playlist_import/providers/{mod,spotify,apple,
//! tidal,deezer}.rs`. Only the URL-detection logic and Spotify's embed-metadata
//! scrape are ported here — the heavy playlist-import machinery (`fetch_playlist`,
//! token plumbing, importer models) is intentionally left in `src-tauri`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which streaming platform a music link belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MusicProvider {
    Spotify,
    AppleMusic,
    Tidal,
    Deezer,
}

/// The kind of resource a music URL points to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MusicResource {
    /// A native Qobuz URL — resolve directly.
    Qobuz,
    /// A single track on a third-party platform.
    Track {
        provider: MusicProvider,
        url: String,
    },
    /// An album on a third-party platform.
    Album {
        provider: MusicProvider,
        url: String,
    },
    /// A playlist — should be redirected to the Playlist Importer.
    Playlist { provider: MusicProvider },
    /// A song.link / album.link / odesli.co URL — resolve via Odesli API.
    SongLink { url: String },
}

/// Detect what kind of music resource a URL points to.
///
/// Returns `None` for URLs that don't match any supported platform.
pub fn detect_music_resource(url: &str) -> Option<MusicResource> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // 1. Qobuz — resolve_link() handles this natively
    if qbz_qobuz::resolve_link(url).is_ok() {
        return Some(MusicResource::Qobuz);
    }

    // 2. song.link / album.link / odesli.co URLs
    let lower = url.to_ascii_lowercase();
    if lower.contains("song.link/") || lower.contains("album.link/") || lower.contains("odesli.co/")
    {
        return Some(MusicResource::SongLink {
            url: url.to_string(),
        });
    }

    // 3. Per-provider detection (track/album/playlist)
    if let Some(resource) = spotify::detect_resource(url) {
        return Some(resource);
    }
    if let Some(resource) = apple::detect_resource(url) {
        return Some(resource);
    }
    if let Some(resource) = tidal::detect_resource(url) {
        return Some(resource);
    }
    if let Some(resource) = deezer::detect_resource(url) {
        return Some(resource);
    }

    None
}

// ── Spotify ──

pub mod spotify {
    use super::*;

    /// Detect if a URL is a Spotify track, album, or playlist.
    pub fn detect_resource(url: &str) -> Option<MusicResource> {
        let lower = url.to_ascii_lowercase();
        if !lower.contains("spotify.com/") && !lower.starts_with("spotify:") {
            return None;
        }

        // Playlist check first (so parse_playlist_id takes priority for playlists)
        if parse_playlist_id(url).is_some() {
            return Some(MusicResource::Playlist {
                provider: MusicProvider::Spotify,
            });
        }

        // Track: open.spotify.com/track/<id> or spotify:track:<id>
        if lower.contains("/track/") || lower.contains(":track:") {
            return Some(MusicResource::Track {
                provider: MusicProvider::Spotify,
                url: url.to_string(),
            });
        }

        // Album: open.spotify.com/album/<id> or spotify:album:<id>
        if lower.contains("/album/") || lower.contains(":album:") {
            return Some(MusicResource::Album {
                provider: MusicProvider::Spotify,
                url: url.to_string(),
            });
        }

        None
    }

    pub fn parse_playlist_id(url: &str) -> Option<String> {
        if let Some(rest) = url.strip_prefix("spotify:playlist:") {
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }

        let patterns = [
            "open.spotify.com/playlist/",
            "open.spotify.com/embed/playlist/",
        ];
        for pattern in patterns {
            if let Some(idx) = url.find(pattern) {
                let mut part = &url[idx + pattern.len()..];
                if let Some(end) = part.find('?') {
                    part = &part[..end];
                }
                if !part.is_empty() {
                    return Some(part.to_string());
                }
            }
        }

        None
    }

    /// Fetch track or album metadata from Spotify embed page.
    /// Returns (title, artist) if successful.
    pub async fn fetch_embed_metadata(
        entity_type: &str,
        entity_id: &str,
    ) -> Option<(String, String)> {
        let url = format!(
            "https://open.spotify.com/embed/{}/{}",
            entity_type, entity_id
        );
        let html = reqwest::get(&url).await.ok()?.text().await.ok()?;
        let json_text = extract_script(&html, "__NEXT_DATA__")?;
        let data: Value = serde_json::from_str(&json_text).ok()?;

        let entity = data
            .get("props")?
            .get("pageProps")?
            .get("state")?
            .get("data")?
            .get("entity")?;

        let title = entity
            .get("title")
            .or_else(|| entity.get("name"))
            .and_then(|v| v.as_str())?
            .to_string();

        // Tracks have "artists" array, albums have "subtitle" string
        let artist = entity
            .get("artists")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a.get("name").and_then(|v| v.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .or_else(|| {
                entity
                    .get("subtitle")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        if title.is_empty() {
            return None;
        }

        Some((title, artist))
    }

    fn extract_script(html: &str, id: &str) -> Option<String> {
        let marker = format!("id=\"{}\"", id);
        let start = html.find(&marker)?;
        let script_start = html[start..].find('>')? + start + 1;
        let script_end = html[script_start..].find("</script>")? + script_start;
        Some(html[script_start..script_end].to_string())
    }
}

// ── Apple Music ──

pub mod apple {
    use super::*;

    /// Detect if a URL is an Apple Music track, album, or playlist.
    pub fn detect_resource(url: &str) -> Option<MusicResource> {
        if !url.contains("music.apple.com/") {
            return None;
        }

        // Playlist
        if parse_playlist_id(url).is_some() {
            return Some(MusicResource::Playlist {
                provider: MusicProvider::AppleMusic,
            });
        }

        // Song page (explicit song URL)
        if url.contains("/song/") {
            return Some(MusicResource::Track {
                provider: MusicProvider::AppleMusic,
                url: url.to_string(),
            });
        }

        // Album page — with ?i= parameter means specific track
        if url.contains("/album/") {
            if url.contains("?i=") || url.contains("&i=") {
                return Some(MusicResource::Track {
                    provider: MusicProvider::AppleMusic,
                    url: url.to_string(),
                });
            }
            return Some(MusicResource::Album {
                provider: MusicProvider::AppleMusic,
                url: url.to_string(),
            });
        }

        None
    }

    pub fn parse_playlist_id(url: &str) -> Option<(String, String)> {
        if !url.contains("music.apple.com/") {
            return None;
        }

        let parts: Vec<&str> = url.split('/').collect();
        if parts.len() < 6 {
            return None;
        }

        let storefront = parts.get(3)?.to_string();
        let playlist_id = parts.last()?.split('?').next()?.to_string();

        if playlist_id.starts_with("pl.") || playlist_id.starts_with("pl.u-") {
            Some((storefront, playlist_id))
        } else {
            None
        }
    }
}

// ── Tidal ──

pub mod tidal {
    use super::*;

    /// Detect if a URL is a Tidal track, album, or playlist.
    pub fn detect_resource(url: &str) -> Option<MusicResource> {
        if !url.contains("tidal.com") {
            return None;
        }

        // Playlist
        if parse_playlist_id(url).is_some() {
            return Some(MusicResource::Playlist {
                provider: MusicProvider::Tidal,
            });
        }

        let lower = url.to_ascii_lowercase();

        // Track
        if lower.contains("/track/") || lower.contains("/browse/track/") {
            return Some(MusicResource::Track {
                provider: MusicProvider::Tidal,
                url: url.to_string(),
            });
        }

        // Album
        if lower.contains("/album/") || lower.contains("/browse/album/") {
            return Some(MusicResource::Album {
                provider: MusicProvider::Tidal,
                url: url.to_string(),
            });
        }

        None
    }

    pub fn parse_playlist_id(url: &str) -> Option<String> {
        if !url.contains("tidal.com") {
            return None;
        }

        let patterns = ["/browse/playlist/", "/playlist/"];
        for pattern in patterns {
            if let Some(idx) = url.find(pattern) {
                let mut part = &url[idx + pattern.len()..];
                if let Some(end) = part.find('?') {
                    part = &part[..end];
                }
                if !part.is_empty() {
                    return Some(part.to_string());
                }
            }
        }

        None
    }
}

// ── Deezer ──

pub mod deezer {
    use super::*;

    /// Detect if a URL is a Deezer track, album, or playlist.
    pub fn detect_resource(url: &str) -> Option<MusicResource> {
        if !url.contains("deezer.com") {
            return None;
        }

        // Playlist
        if parse_playlist_id(url).is_some() {
            return Some(MusicResource::Playlist {
                provider: MusicProvider::Deezer,
            });
        }

        let parts: Vec<&str> = url.split('/').collect();
        for (idx, part) in parts.iter().enumerate() {
            match *part {
                "track" => {
                    if parts.get(idx + 1).map(|s| !s.is_empty()).unwrap_or(false) {
                        return Some(MusicResource::Track {
                            provider: MusicProvider::Deezer,
                            url: url.to_string(),
                        });
                    }
                }
                "album" => {
                    if parts.get(idx + 1).map(|s| !s.is_empty()).unwrap_or(false) {
                        return Some(MusicResource::Album {
                            provider: MusicProvider::Deezer,
                            url: url.to_string(),
                        });
                    }
                }
                _ => {}
            }
        }

        None
    }

    pub fn parse_playlist_id(url: &str) -> Option<String> {
        if !url.contains("deezer.com") {
            return None;
        }

        let parts: Vec<&str> = url.split('/').collect();
        for (idx, part) in parts.iter().enumerate() {
            if *part == "playlist" {
                let id = parts.get(idx + 1)?.split('?').next()?;
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }

        None
    }
}
