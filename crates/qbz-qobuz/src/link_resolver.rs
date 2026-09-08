//! Qobuz link resolver
//!
//! Parses Qobuz URLs (both `qobuzapp://` scheme and `https://play.qobuz.com/`)
//! into typed navigation actions. Pure function, no I/O, no Tauri dependency.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A resolved Qobuz link — tells the frontend which view to navigate to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id")]
pub enum ResolvedLink {
    /// Navigate to album view. ID is a string (matches qbz-models Album.id).
    OpenAlbum(String),
    /// Navigate to track's album. ID is numeric.
    OpenTrack(u64),
    /// Navigate to artist view. ID is numeric.
    OpenArtist(u64),
    /// Navigate to playlist view. ID is numeric.
    OpenPlaylist(u64),
}

/// Errors that can occur when resolving a link.
#[derive(Debug, Clone, PartialEq, Error, Serialize, Deserialize)]
pub enum LinkResolverError {
    #[error("empty input")]
    EmptyInput,
    #[error("malformed URL")]
    MalformedUrl,
    #[error("unsupported scheme — expected qobuzapp:// or https://play.qobuz.com/")]
    UnsupportedScheme,
    #[error("unknown entity type: {0}")]
    UnknownEntityType(String),
    #[error("invalid ID: {0}")]
    InvalidId(String),
}

/// Resolve a Qobuz URL into a navigation action.
///
/// Accepted formats:
/// - `qobuzapp://album/<id>`
/// - `qobuzapp://track/<id>`
/// - `qobuzapp://artist/<id>`
/// - `qobuzapp://playlist/<id>`
/// - `https://play.qobuz.com/album/<id>`
/// - `http://play.qobuz.com/album/<id>` (auto-upgraded)
/// - Same patterns for track, artist, playlist
///
/// Query parameters, fragments, and trailing slashes are stripped.
pub fn resolve_link(url: &str) -> Result<ResolvedLink, LinkResolverError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(LinkResolverError::EmptyInput);
    }

    let (entity_type, raw_id) = if let Some(rest) = url.strip_prefix("qobuzapp://") {
        parse_path_segments(rest)?
    } else if let Some(rest) = strip_web_prefix(url) {
        parse_path_segments(rest)?
    } else {
        return Err(LinkResolverError::UnsupportedScheme);
    };

    build_resolved_link(&entity_type, &raw_id)
}

/// Strip `https://play.qobuz.com/` or `http://play.qobuz.com/` prefix.
/// Also accepts `https://open.qobuz.com/` variant.
fn strip_web_prefix(url: &str) -> Option<&str> {
    let lowered = url.to_ascii_lowercase();
    for prefix in &[
        "https://play.qobuz.com/",
        "http://play.qobuz.com/",
        "https://open.qobuz.com/",
        "http://open.qobuz.com/",
    ] {
        if lowered.starts_with(prefix) {
            return Some(&url[prefix.len()..]);
        }
    }
    None
}

/// Parse `<entity_type>/<id>` from a path, stripping query params and fragments.
fn parse_path_segments(path: &str) -> Result<(String, String), LinkResolverError> {
    // Strip query string and fragment
    let path = path.split('?').next().unwrap_or(path);
    let path = path.split('#').next().unwrap_or(path);
    // Strip trailing slashes
    let path = path.trim_end_matches('/');

    if path.is_empty() {
        return Err(LinkResolverError::MalformedUrl);
    }

    let mut parts = path.splitn(2, '/');
    let entity_type = parts.next().unwrap_or("").to_ascii_lowercase();
    let raw_id = parts.next().unwrap_or("").to_string();

    if entity_type.is_empty() {
        return Err(LinkResolverError::MalformedUrl);
    }
    if raw_id.is_empty() {
        return Err(LinkResolverError::MalformedUrl);
    }

    Ok((entity_type, raw_id))
}

/// Build a ResolvedLink from entity type and raw ID string.
fn build_resolved_link(entity_type: &str, raw_id: &str) -> Result<ResolvedLink, LinkResolverError> {
    match entity_type {
        "album" => {
            // Album IDs are strings (e.g., "0060254728933")
            if raw_id.is_empty() {
                return Err(LinkResolverError::InvalidId(raw_id.to_string()));
            }
            Ok(ResolvedLink::OpenAlbum(raw_id.to_string()))
        }
        "track" => {
            let id = raw_id
                .parse::<u64>()
                .map_err(|_| LinkResolverError::InvalidId(raw_id.to_string()))?;
            Ok(ResolvedLink::OpenTrack(id))
        }
        "artist" | "interpreter" => {
            let id = raw_id
                .parse::<u64>()
                .map_err(|_| LinkResolverError::InvalidId(raw_id.to_string()))?;
            Ok(ResolvedLink::OpenArtist(id))
        }
        "playlist" => {
            let id = raw_id
                .parse::<u64>()
                .map_err(|_| LinkResolverError::InvalidId(raw_id.to_string()))?;
            Ok(ResolvedLink::OpenPlaylist(id))
        }
        _ => Err(LinkResolverError::UnknownEntityType(
            entity_type.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Happy path: HTTPS URLs ──

    #[test]
    fn test_https_album() {
        let result = resolve_link("https://play.qobuz.com/album/0060254728933");
        assert_eq!(result, Ok(ResolvedLink::OpenAlbum("0060254728933".into())));
    }

    #[test]
    fn test_https_track() {
        let result = resolve_link("https://play.qobuz.com/track/12345678");
        assert_eq!(result, Ok(ResolvedLink::OpenTrack(12345678)));
    }

    #[test]
    fn test_https_artist() {
        let result = resolve_link("https://play.qobuz.com/artist/56789");
        assert_eq!(result, Ok(ResolvedLink::OpenArtist(56789)));
    }

    #[test]
    fn test_https_interpreter() {
        let result = resolve_link("https://play.qobuz.com/interpreter/56789");
        assert_eq!(result, Ok(ResolvedLink::OpenArtist(56789)));
    }

    #[test]
    fn test_https_playlist() {
        let result = resolve_link("https://play.qobuz.com/playlist/99887766");
        assert_eq!(result, Ok(ResolvedLink::OpenPlaylist(99887766)));
    }

    // ── Happy path: qobuzapp:// scheme ──

    #[test]
    fn test_scheme_album() {
        let result = resolve_link("qobuzapp://album/abc123def");
        assert_eq!(result, Ok(ResolvedLink::OpenAlbum("abc123def".into())));
    }

    #[test]
    fn test_scheme_track() {
        let result = resolve_link("qobuzapp://track/42");
        assert_eq!(result, Ok(ResolvedLink::OpenTrack(42)));
    }

    #[test]
    fn test_scheme_artist() {
        let result = resolve_link("qobuzapp://artist/100");
        assert_eq!(result, Ok(ResolvedLink::OpenArtist(100)));
    }

    #[test]
    fn test_scheme_playlist() {
        let result = resolve_link("qobuzapp://playlist/200");
        assert_eq!(result, Ok(ResolvedLink::OpenPlaylist(200)));
    }

    // ── Edge cases: trimming ──

    #[test]
    fn test_trailing_slash() {
        let result = resolve_link("https://play.qobuz.com/album/123/");
        assert_eq!(result, Ok(ResolvedLink::OpenAlbum("123".into())));
    }

    #[test]
    fn test_query_params_stripped() {
        let result = resolve_link("https://play.qobuz.com/album/123?ref=share&utm_source=web");
        assert_eq!(result, Ok(ResolvedLink::OpenAlbum("123".into())));
    }

    #[test]
    fn test_fragment_stripped() {
        let result = resolve_link("https://play.qobuz.com/album/123#tracklist");
        assert_eq!(result, Ok(ResolvedLink::OpenAlbum("123".into())));
    }

    #[test]
    fn test_whitespace_trimmed() {
        let result = resolve_link("  https://play.qobuz.com/album/123  ");
        assert_eq!(result, Ok(ResolvedLink::OpenAlbum("123".into())));
    }

    #[test]
    fn test_http_variant() {
        let result = resolve_link("http://play.qobuz.com/track/555");
        assert_eq!(result, Ok(ResolvedLink::OpenTrack(555)));
    }

    #[test]
    fn test_open_qobuz_variant() {
        let result = resolve_link("https://open.qobuz.com/album/999");
        assert_eq!(result, Ok(ResolvedLink::OpenAlbum("999".into())));
    }

    // ── Error cases ──

    #[test]
    fn test_empty_input() {
        assert_eq!(resolve_link(""), Err(LinkResolverError::EmptyInput));
    }

    #[test]
    fn test_whitespace_only() {
        assert_eq!(resolve_link("   "), Err(LinkResolverError::EmptyInput));
    }

    #[test]
    fn test_unsupported_scheme() {
        assert_eq!(
            resolve_link("https://www.google.com/album/123"),
            Err(LinkResolverError::UnsupportedScheme)
        );
    }

    #[test]
    fn test_random_text() {
        assert_eq!(
            resolve_link("not a url at all"),
            Err(LinkResolverError::UnsupportedScheme)
        );
    }

    #[test]
    fn test_unknown_entity() {
        assert_eq!(
            resolve_link("https://play.qobuz.com/label/123"),
            Err(LinkResolverError::UnknownEntityType("label".into()))
        );
    }

    #[test]
    fn test_invalid_track_id() {
        assert_eq!(
            resolve_link("https://play.qobuz.com/track/not-a-number"),
            Err(LinkResolverError::InvalidId("not-a-number".into()))
        );
    }

    #[test]
    fn test_missing_id() {
        assert_eq!(
            resolve_link("https://play.qobuz.com/album/"),
            Err(LinkResolverError::MalformedUrl)
        );
    }

    #[test]
    fn test_scheme_no_path() {
        assert_eq!(
            resolve_link("qobuzapp://"),
            Err(LinkResolverError::MalformedUrl)
        );
    }

    #[test]
    fn test_scheme_only_entity_no_id() {
        assert_eq!(
            resolve_link("qobuzapp://album"),
            Err(LinkResolverError::MalformedUrl)
        );
    }
}
