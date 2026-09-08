//! Tidal playlist import (OpenAPI v2)

use std::time::Duration;

use serde_json::Value;
use tokio::time::sleep;

use crate::errors::PlaylistImportError;
use crate::http::http;
use crate::models::{ImportPlaylist, ImportProvider, ImportTrack};

const RATE_LIMIT_DELAY_MS: u64 = 200; // Delay between API calls to avoid 429

const TIDAL_API_BASE: &str = "https://openapi.tidal.com/v2";
const DEFAULT_COUNTRY_CODE: &str = "US";

/// Detect if a URL is a Tidal track, album, or playlist.
///
/// Tidal URLs:
/// - Track: `tidal.com/browse/track/{id}` or `tidal.com/track/{id}`
/// - Album: `tidal.com/browse/album/{id}` or `tidal.com/album/{id}`
/// - Playlist: `tidal.com/browse/playlist/{id}` or `tidal.com/playlist/{id}`
pub fn detect_resource(url: &str) -> Option<super::MusicResource> {
    if !url.contains("tidal.com") {
        return None;
    }

    // Playlist
    if parse_playlist_id(url).is_some() {
        return Some(super::MusicResource::Playlist {
            provider: super::MusicProvider::Tidal,
        });
    }

    let lower = url.to_ascii_lowercase();

    // Track
    if lower.contains("/track/") || lower.contains("/browse/track/") {
        return Some(super::MusicResource::Track {
            provider: super::MusicProvider::Tidal,
            url: url.to_string(),
        });
    }

    // Album
    if lower.contains("/album/") || lower.contains("/browse/album/") {
        return Some(super::MusicResource::Album {
            provider: super::MusicProvider::Tidal,
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

/// Fetch a Tidal playlist.
///
/// `country_code` replaces the Tauri original's `TIDAL_COUNTRY_CODE` env read
/// — `None` keeps the same "US" default; callers wanting the env behavior
/// read it at their edge and pass `Some(..)`.
pub async fn fetch_playlist(
    playlist_id: &str,
    country_code: Option<&str>,
) -> Result<ImportPlaylist, PlaylistImportError> {
    let token = get_app_token().await?;
    let country_code = country_code.unwrap_or(DEFAULT_COUNTRY_CODE).to_string();

    let client = http();
    let meta_url = format!("{}/playlists/{}", TIDAL_API_BASE, playlist_id);
    let resp = client
        .get(&meta_url)
        .header("Authorization", format!("Bearer {}", token))
        .query(&[("countryCode", &country_code)])
        .send()
        .await
        .map_err(|e| PlaylistImportError::Http(e.to_string()))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| PlaylistImportError::Parse(e.to_string()))?;

    if !status.is_success() {
        return Err(PlaylistImportError::Http(format!(
            "Tidal playlist fetch failed: {} - {}",
            status, body
        )));
    }

    let meta: Value = serde_json::from_str(&body)
        .map_err(|e| PlaylistImportError::Parse(format!("Invalid playlist JSON: {}", e)))?;

    let name = meta
        .get("data")
        .and_then(|v| v.get("attributes"))
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("Tidal Playlist")
        .to_string();
    let description = meta
        .get("data")
        .and_then(|v| v.get("attributes"))
        .and_then(|v| v.get("description"))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .filter(|v| !v.is_empty());

    let track_ids = fetch_track_ids(client, &token, playlist_id, &country_code).await?;
    let tracks = fetch_tracks_by_ids(client, &token, &track_ids, &country_code).await?;

    Ok(ImportPlaylist {
        provider: ImportProvider::Tidal,
        provider_id: playlist_id.to_string(),
        name,
        description,
        tracks,
    })
}

async fn fetch_track_ids(
    client: &reqwest::Client,
    token: &str,
    playlist_id: &str,
    country_code: &str,
) -> Result<Vec<String>, PlaylistImportError> {
    let mut ids = Vec::new();
    let mut next_path = format!("/playlists/{}/relationships/items?limit=100", playlist_id);

    loop {
        let url = format!("{}{}", TIDAL_API_BASE, next_path);
        sleep(Duration::from_millis(RATE_LIMIT_DELAY_MS)).await;

        let mut request = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token));
        if !next_path.contains("countryCode=") {
            request = request.query(&[("countryCode", country_code)]);
        }

        let resp = request
            .send()
            .await
            .map_err(|e| PlaylistImportError::Http(e.to_string()))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| PlaylistImportError::Parse(e.to_string()))?;

        if !status.is_success() {
            return Err(PlaylistImportError::Http(format!(
                "Tidal track IDs fetch failed: {} - {}",
                status, body
            )));
        }

        let response: Value = serde_json::from_str(&body)
            .map_err(|e| PlaylistImportError::Parse(format!("Invalid track IDs JSON: {}", e)))?;

        if let Some(data) = response.get("data").and_then(|v| v.as_array()) {
            for item in data {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    ids.push(id.to_string());
                }
            }
        }

        let next = response
            .get("links")
            .and_then(|v| v.get("next"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        match next {
            Some(path) if !path.is_empty() => {
                next_path = path;
            }
            _ => break,
        }
    }

    Ok(ids)
}

async fn fetch_tracks_by_ids(
    client: &reqwest::Client,
    token: &str,
    track_ids: &[String],
    country_code: &str,
) -> Result<Vec<ImportTrack>, PlaylistImportError> {
    let mut tracks = Vec::new();
    let mut chunk_start = 0usize;
    let chunk_size = 20usize; // Tidal API limit

    while chunk_start < track_ids.len() {
        let end = (chunk_start + chunk_size).min(track_ids.len());
        let chunk = &track_ids[chunk_start..end];

        let url = format!("{}/tracks", TIDAL_API_BASE);
        sleep(Duration::from_millis(RATE_LIMIT_DELAY_MS)).await;

        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .query(&[
                ("filter[id]", chunk.join(",")),
                ("include", "artists,albums".to_string()),
                ("countryCode", country_code.to_string()),
            ])
            .send()
            .await
            .map_err(|e| PlaylistImportError::Http(e.to_string()))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| PlaylistImportError::Parse(e.to_string()))?;

        if !status.is_success() {
            return Err(PlaylistImportError::Http(format!(
                "Tidal tracks fetch failed: {} - {}",
                status, body
            )));
        }

        let response: Value = serde_json::from_str(&body)
            .map_err(|e| PlaylistImportError::Parse(format!("Invalid tracks JSON: {}", e)))?;

        let included = response
            .get("included")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut artist_map = std::collections::HashMap::new();
        let mut album_map = std::collections::HashMap::new();

        for item in included {
            if let Some(item_type) = item.get("type").and_then(|v| v.as_str()) {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    continue;
                }

                match item_type {
                    "artists" => {
                        if let Some(name) = item
                            .get("attributes")
                            .and_then(|v| v.get("name"))
                            .and_then(|v| v.as_str())
                        {
                            artist_map.insert(id.to_string(), name.to_string());
                        }
                    }
                    "albums" => {
                        if let Some(name) = item
                            .get("attributes")
                            .and_then(|v| v.get("title"))
                            .and_then(|v| v.as_str())
                        {
                            album_map.insert(id.to_string(), name.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        if let Some(data) = response.get("data").and_then(|v| v.as_array()) {
            for item in data {
                let title = item
                    .get("attributes")
                    .and_then(|v| v.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let isrc = item
                    .get("attributes")
                    .and_then(|v| v.get("isrc"))
                    .and_then(|v| v.as_str())
                    .map(|v| v.to_string());
                let duration_ms = item
                    .get("attributes")
                    .and_then(|v| v.get("duration"))
                    .and_then(|v| v.as_str())
                    .and_then(|v| parse_duration_ms(v));

                let artist = item
                    .get("relationships")
                    .and_then(|v| v.get("artists"))
                    .and_then(|v| v.get("data"))
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
                    .and_then(|id| artist_map.get(id))
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_string());

                let album = item
                    .get("relationships")
                    .and_then(|v| v.get("albums"))
                    .and_then(|v| v.get("data"))
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
                    .and_then(|id| album_map.get(id))
                    .cloned();

                let provider_id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|v| v.to_string());

                tracks.push(ImportTrack {
                    title,
                    artist,
                    album,
                    duration_ms,
                    isrc,
                    provider_id,
                    provider_url: None,
                });
            }
        }

        chunk_start = end;
    }

    Ok(tracks)
}

fn parse_duration_ms(value: &str) -> Option<u64> {
    if !value.starts_with('P') {
        return None;
    }

    let mut seconds = 0u64;
    let mut parsed_any = false;
    let mut num = String::new();
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            num.push(ch);
            continue;
        }

        if num.is_empty() {
            continue;
        }

        let value_num: u64 = num.parse().ok()?;
        parsed_any = true;
        match ch {
            'H' => seconds += value_num * 3600,
            'M' => seconds += value_num * 60,
            'S' => seconds += value_num,
            _ => {}
        }
        num.clear();
    }

    if !parsed_any {
        None
    } else {
        Some(seconds * 1000)
    }
}

async fn get_app_token() -> Result<String, PlaylistImportError> {
    // Proxy handles credentials
    let url = format!("{}/tidal/token", crate::QBZ_PROXY_BASE);

    let response: Value = http()
        .get(&url)
        .header(reqwest::header::USER_AGENT, crate::http::USER_AGENT)
        .send()
        .await
        .map_err(|e| PlaylistImportError::Http(e.to_string()))?
        .json()
        .await
        .map_err(|e| PlaylistImportError::Parse(e.to_string()))?;

    response
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or_else(|| PlaylistImportError::Parse("Tidal proxy missing access_token".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_playlist_id_table() {
        let cases: &[(&str, Option<&str>)] = &[
            (
                "https://tidal.com/browse/playlist/1b418bb8-90a7-4f87-901d-707993838346",
                Some("1b418bb8-90a7-4f87-901d-707993838346"),
            ),
            (
                "https://listen.tidal.com/playlist/1b418bb8-90a7-4f87-901d-707993838346",
                Some("1b418bb8-90a7-4f87-901d-707993838346"),
            ),
            ("https://tidal.com/playlist/abc?u=1", Some("abc")),
            ("https://tidal.com/browse/album/123", None),
            ("https://tidal.com/playlist/", None),
            ("https://example.com/playlist/abc", None),
        ];

        for (url, expected) in cases {
            assert_eq!(parse_playlist_id(url).as_deref(), *expected, "url: {}", url);
        }
    }

    #[test]
    fn detect_resource_track_album_playlist() {
        assert!(matches!(
            detect_resource("https://tidal.com/browse/playlist/abc"),
            Some(super::super::MusicResource::Playlist { .. })
        ));
        assert!(matches!(
            detect_resource("https://tidal.com/browse/track/12345"),
            Some(super::super::MusicResource::Track { .. })
        ));
        assert!(matches!(
            detect_resource("https://tidal.com/album/67890"),
            Some(super::super::MusicResource::Album { .. })
        ));
        assert_eq!(detect_resource("https://example.com/track/1"), None);
    }

    #[test]
    fn parse_duration_ms_iso8601_forms() {
        assert_eq!(parse_duration_ms("PT3M21S"), Some(201_000));
        assert_eq!(parse_duration_ms("PT1H2M3S"), Some(3_723_000));
        assert_eq!(parse_duration_ms("PT45S"), Some(45_000));
        // No leading P
        assert_eq!(parse_duration_ms("3M21S"), None);
        // No digit/unit pairs at all
        assert_eq!(parse_duration_ms("PT"), None);
        assert_eq!(parse_duration_ms(""), None);
    }
}
