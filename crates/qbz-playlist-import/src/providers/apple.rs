//! Apple Music playlist import

use serde_json::Value;

use crate::errors::PlaylistImportError;
use crate::http::http;
use crate::models::{ImportPlaylist, ImportProvider, ImportTrack};

/// Detect if a URL is an Apple Music track, album, or playlist.
///
/// Apple Music URLs:
/// - Track: `music.apple.com/{storefront}/album/{name}/{id}?i={track_id}`
/// - Album: `music.apple.com/{storefront}/album/{name}/{id}` (no `?i=`)
/// - Playlist: `music.apple.com/{storefront}/playlist/{name}/{pl.xxx}`
/// - Song: `music.apple.com/{storefront}/song/{name}/{id}`
pub fn detect_resource(url: &str) -> Option<super::MusicResource> {
    if !url.contains("music.apple.com/") {
        return None;
    }

    // Playlist
    if parse_playlist_id(url).is_some() {
        return Some(super::MusicResource::Playlist {
            provider: super::MusicProvider::AppleMusic,
        });
    }

    // Song page (explicit song URL)
    if url.contains("/song/") {
        return Some(super::MusicResource::Track {
            provider: super::MusicProvider::AppleMusic,
            url: url.to_string(),
        });
    }

    // Album page — with ?i= parameter means specific track
    if url.contains("/album/") {
        if url.contains("?i=") || url.contains("&i=") {
            return Some(super::MusicResource::Track {
                provider: super::MusicProvider::AppleMusic,
                url: url.to_string(),
            });
        }
        return Some(super::MusicResource::Album {
            provider: super::MusicProvider::AppleMusic,
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

pub async fn fetch_playlist(
    storefront: &str,
    playlist_id: &str,
) -> Result<ImportPlaylist, PlaylistImportError> {
    let url = format!(
        "https://music.apple.com/{}/playlist/{}",
        storefront, playlist_id
    );
    let html = http()
        .get(&url)
        .send()
        .await
        .map_err(|e| PlaylistImportError::Http(e.to_string()))?
        .text()
        .await
        .map_err(|e| PlaylistImportError::Http(e.to_string()))?;

    let name =
        extract_meta(&html, "og:title").unwrap_or_else(|| "Apple Music Playlist".to_string());
    let description = extract_meta(&html, "og:description").filter(|v| !v.is_empty());

    let json_text = extract_script(&html, "serialized-server-data").ok_or_else(|| {
        PlaylistImportError::Parse("Apple Music serialized-server-data not found".to_string())
    })?;

    let data: Value =
        serde_json::from_str(&json_text).map_err(|e| PlaylistImportError::Parse(e.to_string()))?;

    let items = find_track_items(&data).ok_or_else(|| {
        PlaylistImportError::Parse("Apple Music track list not found".to_string())
    })?;

    let mut tracks = Vec::new();
    for item in items {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let artist = item
            .get("artistName")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let duration_ms = item.get("duration").and_then(|v| v.as_u64());
        let provider_id = item
            .get("contentDescriptor")
            .and_then(|v| v.get("identifiers"))
            .and_then(|v| v.get("storeAdamID"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let provider_url = item
            .get("contentDescriptor")
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let album = item
            .get("tertiaryLinks")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("title"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        tracks.push(ImportTrack {
            title,
            artist,
            album,
            duration_ms,
            isrc: None,
            provider_id,
            provider_url,
        });
    }

    Ok(ImportPlaylist {
        provider: ImportProvider::AppleMusic,
        provider_id: playlist_id.to_string(),
        name,
        description,
        tracks,
    })
}

fn extract_script(html: &str, id: &str) -> Option<String> {
    let marker = format!("id=\"{}\"", id);
    let start = html.find(&marker)?;
    let script_start = html[start..].find('>')? + start + 1;
    let script_end = html[script_start..].find("</script>")? + script_start;
    let raw = &html[script_start..script_end];
    Some(unescape_basic(raw))
}

fn find_track_items(data: &Value) -> Option<Vec<&Value>> {
    match data {
        Value::Object(map) => {
            if map.get("itemKind").and_then(|v| v.as_str()) == Some("trackLockup") {
                let items = map.get("items").and_then(|v| v.as_array())?;
                if !items.is_empty() {
                    return Some(items.iter().collect());
                }
            }

            for value in map.values() {
                if let Some(found) = find_track_items(value) {
                    return Some(found);
                }
            }
        }
        Value::Array(list) => {
            for value in list {
                if let Some(found) = find_track_items(value) {
                    return Some(found);
                }
            }
        }
        _ => {}
    }

    None
}

fn extract_meta(html: &str, property: &str) -> Option<String> {
    let needle = format!("property=\"{}\"", property);
    let start = html.find(&needle)?;
    let content_start = html[start..].find("content=\"")? + start + "content=\"".len();
    let content_end = html[content_start..].find('"')? + content_start;
    Some(unescape_basic(&html[content_start..content_end]))
}

fn unescape_basic(input: &str) -> String {
    input
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_playlist_id_table() {
        // Editorial pl. id
        assert_eq!(
            parse_playlist_id(
                "https://music.apple.com/us/playlist/top-100-global/pl.d25f5d1181894928af76c85c967f8f31"
            ),
            Some((
                "us".to_string(),
                "pl.d25f5d1181894928af76c85c967f8f31".to_string()
            ))
        );
        // User pl.u- id + query strip
        assert_eq!(
            parse_playlist_id("https://music.apple.com/mx/playlist/mias/pl.u-abc123?l=en"),
            Some(("mx".to_string(), "pl.u-abc123".to_string())),
        );
        // Album URL is not a playlist
        assert_eq!(
            parse_playlist_id("https://music.apple.com/us/album/abbey-road/1441164426"),
            None
        );
        // Too few path segments
        assert_eq!(
            parse_playlist_id("https://music.apple.com/us/playlist"),
            None
        );
        // Wrong host
        assert_eq!(
            parse_playlist_id("https://example.com/us/playlist/x/pl.123"),
            None
        );
    }

    #[test]
    fn detect_resource_song_album_track() {
        assert!(matches!(
            detect_resource("https://music.apple.com/us/song/hey-jude/1441164589"),
            Some(super::super::MusicResource::Track { .. })
        ));
        assert!(matches!(
            detect_resource("https://music.apple.com/us/album/abbey-road/1441164426?i=1441164589"),
            Some(super::super::MusicResource::Track { .. })
        ));
        assert!(matches!(
            detect_resource("https://music.apple.com/us/album/abbey-road/1441164426"),
            Some(super::super::MusicResource::Album { .. })
        ));
        assert!(matches!(
            detect_resource("https://music.apple.com/us/playlist/x/pl.123"),
            Some(super::super::MusicResource::Playlist { .. })
        ));
        assert_eq!(detect_resource("https://example.com/us/album/x/1"), None);
    }

    #[test]
    fn extract_script_unescapes_serialized_server_data() {
        let html = concat!(
            "<script type=\"application/json\" id=\"serialized-server-data\">",
            "[{&quot;itemKind&quot;:&quot;trackLockup&quot;}]",
            "</script>"
        );
        assert_eq!(
            extract_script(html, "serialized-server-data").as_deref(),
            Some("[{\"itemKind\":\"trackLockup\"}]")
        );
    }

    #[test]
    fn extract_meta_reads_og_tags() {
        let html = concat!(
            "<meta property=\"og:title\" content=\"My Playlist &amp; More\">",
            "<meta property=\"og:description\" content=\"\">"
        );
        assert_eq!(
            extract_meta(html, "og:title").as_deref(),
            Some("My Playlist & More")
        );
        assert_eq!(extract_meta(html, "og:description").as_deref(), Some(""));
        assert_eq!(extract_meta(html, "og:image"), None);
    }

    #[test]
    fn unescape_basic_entities() {
        assert_eq!(
            unescape_basic("&quot;a&quot; &#34;b&#34; &amp; &lt;c&gt;"),
            "\"a\" \"b\" & <c>"
        );
    }

    #[test]
    fn find_track_items_locates_track_lockup_anywhere() {
        let data: Value = serde_json::from_str(
            r#"[{"sections":[{"itemKind":"trackLockup","items":[
                {"title":"Hey Jude","artistName":"The Beatles","duration":431333},
                {"title":"Let It Be","artistName":"The Beatles","duration":243026}
            ]}]}]"#,
        )
        .unwrap();
        let items = find_track_items(&data).expect("found");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["title"].as_str(), Some("Hey Jude"));
        assert_eq!(items[1]["artistName"].as_str(), Some("The Beatles"));
    }

    #[test]
    fn find_track_items_ignores_empty_lockups() {
        let data: Value = serde_json::from_str(r#"{"itemKind":"trackLockup","items":[]}"#).unwrap();
        assert!(find_track_items(&data).is_none());
    }
}
