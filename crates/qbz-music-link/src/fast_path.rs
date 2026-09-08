//! Direct platform metadata (bypass Odesli for speed).
//!
//! Ported verbatim from the `src-tauri` link resolver. For Tidal/Deezer this
//! calls the platform API directly; for Spotify it scrapes the embed page.
//! Apple Music has no direct API and falls through to Odesli.

use crate::detection::{spotify, MusicProvider};

/// Public Cloudflare-worker proxy base. NOT a secret — this is the same public
/// URL hardcoded in the `src-tauri` original. No API keys are embedded here.
const QBZ_PROXY_BASE: &str = "https://qbz-api-proxy.blitzkriegfc.workers.dev";

/// Try to get title+artist directly from the platform API.
/// Returns None if the platform isn't supported or the request fails.
pub(crate) async fn try_direct_platform_metadata(
    url: &str,
    provider: &MusicProvider,
    is_track: bool,
) -> Option<(String, String)> {
    match provider {
        MusicProvider::Deezer => try_deezer_metadata(url, is_track).await,
        MusicProvider::Spotify => try_spotify_metadata(url, is_track).await,
        MusicProvider::Tidal => try_tidal_metadata(url, is_track).await,
        MusicProvider::AppleMusic => None, // No direct API available
    }
}

/// Extract a numeric or alphanumeric ID after /track/ or /album/ in a URL.
fn extract_entity_id(url: &str, entity_type: &str) -> Option<String> {
    let pattern = format!("/{}/", entity_type);
    let idx = url.find(&pattern)?;
    let rest = &url[idx + pattern.len()..];
    let id = rest.split(['?', '/', '#']).next()?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Extract Spotify ID from URL or URI.
fn extract_spotify_entity_id(url: &str, entity_type: &str) -> Option<String> {
    // URI format: spotify:track:abc123
    let uri_pattern = format!("spotify:{}:", entity_type);
    if let Some(rest) = url.strip_prefix(&uri_pattern) {
        let id = rest.split(['?', '/']).next()?;
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    extract_entity_id(url, entity_type)
}

async fn try_deezer_metadata(url: &str, is_track: bool) -> Option<(String, String)> {
    let entity = if is_track { "track" } else { "album" };
    let id = extract_entity_id(url, entity).or_else(|| {
        if is_track {
            None
        } else {
            extract_entity_id(url, "track")
        }
    })?;
    let api_url = format!("https://api.deezer.com/{}/{}", entity, id);

    log::debug!("Link resolver: Deezer direct API: {}", api_url);
    let data: serde_json::Value = reqwest::get(&api_url).await.ok()?.json().await.ok()?;
    if data.get("error").is_some() {
        return None;
    }

    let title = data.get("title")?.as_str()?.to_string();
    let artist = data
        .get("artist")
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((title, artist))
}

async fn try_spotify_metadata(url: &str, is_track: bool) -> Option<(String, String)> {
    let entity = if is_track { "track" } else { "album" };
    let id = extract_spotify_entity_id(url, entity)?;

    log::debug!("Link resolver: Spotify embed scrape for {} {}", entity, id);
    spotify::fetch_embed_metadata(entity, &id).await
}

async fn try_tidal_metadata(url: &str, is_track: bool) -> Option<(String, String)> {
    let entity = if is_track { "track" } else { "album" };
    let id = extract_entity_id(url, entity)
        // Also try /browse/track/ pattern
        .or_else(|| extract_entity_id(url, &format!("browse/{}", entity)))?;
    let token = get_proxy_token("tidal").await?;
    let api_url = format!(
        "https://openapi.tidal.com/v2/{}s/{}?countryCode=US&include=artists",
        entity, id
    );

    log::debug!("Link resolver: Tidal direct API: {}", api_url);
    let data: serde_json::Value = reqwest::Client::new()
        .get(&api_url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let title = data
        .get("data")
        .and_then(|d| d.get("attributes"))
        .and_then(|a| a.get("title"))
        .and_then(|v| v.as_str())?
        .to_string();

    // Artist name is in the "included" array
    let artist = data
        .get("included")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|item| item.get("type").and_then(|v| v.as_str()) == Some("artists"))
        })
        .and_then(|item| item.get("attributes"))
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((title, artist))
}

/// Get an OAuth token from the QBZ proxy for the given platform.
async fn get_proxy_token(platform: &str) -> Option<String> {
    let url = format!("{}/{}/token", QBZ_PROXY_BASE, platform);
    let data: serde_json::Value = reqwest::Client::builder()
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert(
                reqwest::header::USER_AGENT,
                reqwest::header::HeaderValue::from_static("QBZ/1.0.0"),
            );
            h
        })
        .build()
        .ok()?
        .get(&url)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    data.get("access_token")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}
