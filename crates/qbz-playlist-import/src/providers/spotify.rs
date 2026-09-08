//! Spotify playlist import
//!
//! As of 2026-03-06, Spotify API access via client_credentials is no longer available.
//! All playlist imports now use the embed page scraping fallback.
//! Embed is limited to ~50 tracks and provides no ISRC or album data.

use serde_json::Value;

use crate::errors::PlaylistImportError;
use crate::http::http;
use crate::models::{ImportPlaylist, ImportProvider, ImportTrack};

/// Detect if a URL is a Spotify track, album, or playlist.
pub fn detect_resource(url: &str) -> Option<super::MusicResource> {
    let lower = url.to_ascii_lowercase();
    if !lower.contains("spotify.com/") && !lower.starts_with("spotify:") {
        return None;
    }

    // Playlist check first (so parse_playlist_id takes priority for playlists)
    if parse_playlist_id(url).is_some() {
        return Some(super::MusicResource::Playlist {
            provider: super::MusicProvider::Spotify,
        });
    }

    // Track: open.spotify.com/track/<id> or spotify:track:<id>
    if lower.contains("/track/") || lower.contains(":track:") {
        return Some(super::MusicResource::Track {
            provider: super::MusicProvider::Spotify,
            url: url.to_string(),
        });
    }

    // Album: open.spotify.com/album/<id> or spotify:album:<id>
    if lower.contains("/album/") || lower.contains(":album:") {
        return Some(super::MusicResource::Album {
            provider: super::MusicProvider::Spotify,
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
pub async fn fetch_embed_metadata(entity_type: &str, entity_id: &str) -> Option<(String, String)> {
    let url = format!(
        "https://open.spotify.com/embed/{}/{}",
        entity_type, entity_id
    );
    let html = http().get(&url).send().await.ok()?.text().await.ok()?;
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

pub async fn fetch_playlist(playlist_id: &str) -> Result<ImportPlaylist, PlaylistImportError> {
    log::info!(
        "Spotify: fetching playlist {} via embed (API no longer available)",
        playlist_id
    );
    fetch_playlist_from_embed(playlist_id).await
}

async fn fetch_playlist_from_embed(
    playlist_id: &str,
) -> Result<ImportPlaylist, PlaylistImportError> {
    let url = format!("https://open.spotify.com/embed/playlist/{}", playlist_id);
    let html = http()
        .get(&url)
        .send()
        .await
        .map_err(|e| PlaylistImportError::Http(e.to_string()))?
        .text()
        .await
        .map_err(|e| PlaylistImportError::Http(e.to_string()))?;

    let json_text = extract_script(&html, "__NEXT_DATA__").ok_or_else(|| {
        PlaylistImportError::Parse("Spotify embed missing __NEXT_DATA__".to_string())
    })?;

    let data: Value =
        serde_json::from_str(&json_text).map_err(|e| PlaylistImportError::Parse(e.to_string()))?;

    let entity = data
        .get("props")
        .and_then(|v| v.get("pageProps"))
        .and_then(|v| v.get("state"))
        .and_then(|v| v.get("data"))
        .and_then(|v| v.get("entity"))
        .ok_or_else(|| PlaylistImportError::Parse("Spotify embed missing entity".to_string()))?;

    let name = entity
        .get("title")
        .or_else(|| entity.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("Spotify Playlist")
        .to_string();

    let mut tracks = Vec::new();
    let track_list = entity
        .get("trackList")
        .and_then(|v| v.as_array())
        .ok_or_else(|| PlaylistImportError::Parse("Spotify embed missing trackList".to_string()))?;

    for track in track_list {
        let title = track
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let artist = track
            .get("subtitle")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let duration_ms = track.get("duration").and_then(|v| v.as_u64());
        let uri = track.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        let provider_id = uri
            .split(':')
            .last()
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string());
        let provider_url = provider_id
            .as_ref()
            .map(|id| format!("https://open.spotify.com/track/{}", id));

        tracks.push(ImportTrack {
            title,
            artist,
            album: None,
            duration_ms,
            isrc: None,
            provider_id,
            provider_url,
        });
    }

    log::info!(
        "Spotify: embed returned {} tracks for '{}' (embed limit is ~50, no ISRC/album data)",
        tracks.len(),
        name
    );

    Ok(ImportPlaylist {
        provider: ImportProvider::Spotify,
        provider_id: playlist_id.to_string(),
        name,
        description: None,
        tracks,
    })
}

fn extract_script(html: &str, id: &str) -> Option<String> {
    let marker = format!("id=\"{}\"", id);
    let start = html.find(&marker)?;
    let script_start = html[start..].find('>')? + start + 1;
    let script_end = html[script_start..].find("</script>")? + script_start;
    Some(html[script_start..script_end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_playlist_id_table() {
        let cases: &[(&str, Option<&str>)] = &[
            (
                "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M",
                Some("37i9dQZF1DXcBWIGoYBM5M"),
            ),
            (
                "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M",
                Some("37i9dQZF1DXcBWIGoYBM5M"),
            ),
            (
                "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=xyz&utm=1",
                Some("37i9dQZF1DXcBWIGoYBM5M"),
            ),
            (
                "https://open.spotify.com/embed/playlist/37i9dQZF1DXcBWIGoYBM5M",
                Some("37i9dQZF1DXcBWIGoYBM5M"),
            ),
            ("spotify:playlist:", None),
            ("https://open.spotify.com/playlist/", None),
            ("https://open.spotify.com/track/abc", None),
            ("https://example.com/", None),
        ];

        for (url, expected) in cases {
            assert_eq!(parse_playlist_id(url).as_deref(), *expected, "url: {}", url);
        }
    }

    #[test]
    fn detect_resource_track_album_playlist() {
        assert_eq!(
            detect_resource("https://open.spotify.com/playlist/abc"),
            Some(super::super::MusicResource::Playlist {
                provider: super::super::MusicProvider::Spotify
            })
        );
        assert!(matches!(
            detect_resource("https://open.spotify.com/track/abc"),
            Some(super::super::MusicResource::Track { .. })
        ));
        assert!(matches!(
            detect_resource("spotify:album:abc"),
            Some(super::super::MusicResource::Album { .. })
        ));
        assert_eq!(detect_resource("https://example.com/track/abc"), None);
    }

    #[test]
    fn extract_script_pulls_next_data_payload() {
        let html = concat!(
            "<html><head></head><body>",
            "<script id=\"__NEXT_DATA__\" type=\"application/json\">",
            "{\"props\":{\"pageProps\":{\"state\":{\"data\":{\"entity\":{\"title\":\"My Mix\"}}}}}}",
            "</script></body></html>"
        );
        let json_text = extract_script(html, "__NEXT_DATA__").expect("script found");
        let data: Value = serde_json::from_str(&json_text).expect("valid JSON");
        let title = data["props"]["pageProps"]["state"]["data"]["entity"]["title"]
            .as_str()
            .unwrap();
        assert_eq!(title, "My Mix");
    }

    #[test]
    fn extract_script_missing_id_is_none() {
        assert_eq!(extract_script("<html></html>", "__NEXT_DATA__"), None);
    }
}
