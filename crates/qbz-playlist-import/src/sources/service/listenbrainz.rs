//! ListenBrainz — a playlist by MBID, and the "created for you" list.
//!
//! Two public endpoints, no auth required:
//!
//! | what | endpoint |
//! |---|---|
//! | one playlist | `GET /1/playlist/<mbid>` |
//! | "created for you" | `GET /1/user/<user>/playlists/createdfor?count=N` |
//!
//! Both return JSPF. The interesting detail is that JSPF `duration` IS
//! MILLISECONDS — same as XSPF, which JSPF is the JSON serialization of — so it
//! goes through unmultiplied.
//!
//! MBID -> ISRC enrichment is an explicit NON-GOAL. It would unlock the
//! matcher's score-1.0 fast path, but it costs one MusicBrainz request per
//! track against a 1 req/s rate limit and a new dependency. Backlog, not here.

use serde_json::Value;

use super::LB_USER_AGENT;
use crate::errors::PlaylistImportError;
use crate::http::http;
use crate::models::{ImportPlaylist, ImportProvider, ImportTrack};

const API: &str = "https://api.listenbrainz.org";
/// How many "created for you" entries to offer. The endpoint's own default is
/// 25; the list is a dropdown, not a page.
const CREATED_FOR_COUNT: usize = 25;

/// What a pasted string means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LbTarget {
    /// A playlist MBID (pasted bare or lifted out of a listenbrainz.org URL).
    Mbid(String),
    /// A username — the "created for you" list is fetched for it.
    User(String),
}

/// One entry of the "created for you" dropdown.
#[derive(Debug, Clone)]
pub struct LbPlaylistOption {
    pub mbid: String,
    pub title: String,
}

/// Cheap, synchronous, and safe to run on every keystroke.
///
/// MBID FIRST: a URL is checked before a bare username, because
/// `listenbrainz.org/playlist/<mbid>` also contains something that looks like a
/// path segment a loose username rule would grab.
pub fn detect(input: &str) -> Option<LbTarget> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains("listenbrainz.org") {
        // .../playlist/<mbid>  (trailing slash and query tolerated)
        if let Some(rest) = s.split("/playlist/").nth(1) {
            let mbid = rest
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if is_mbid(&mbid) {
                return Some(LbTarget::Mbid(mbid));
            }
        }
        // .../user/<name>
        if let Some(rest) = s.split("/user/").nth(1) {
            let user = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
            if is_username(user) {
                return Some(LbTarget::User(user.to_string()));
            }
        }
        return None;
    }
    if is_mbid(s) {
        return Some(LbTarget::Mbid(s.to_string()));
    }
    if is_username(s) {
        return Some(LbTarget::User(s.to_string()));
    }
    None
}

/// 8-4-4-4-12 hex.
fn is_mbid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Conservative: no spaces, no slashes, no dots — enough to reject a stray URL
/// or a sentence without pretending to know the service's real rules.
fn is_username(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The "created for you" dropdown's contents.
pub async fn list_created_for(
    user: &str,
    token: Option<&str>,
) -> Result<Vec<LbPlaylistOption>, PlaylistImportError> {
    let url = format!("{API}/1/user/{user}/playlists/createdfor?count={CREATED_FOR_COUNT}");
    let doc = get_json(&url, token).await?;
    let mut out = Vec::new();
    for entry in doc["playlists"].as_array().into_iter().flatten() {
        let pl = &entry["playlist"];
        let Some(id) = pl["identifier"].as_str() else {
            continue;
        };
        // `identifier` is a URI: .../playlist/<mbid>
        let mbid = id.rsplit('/').next().unwrap_or("").to_string();
        if !is_mbid(&mbid) {
            continue;
        }
        out.push(LbPlaylistOption {
            mbid,
            title: pl["title"].as_str().unwrap_or("Playlist").to_string(),
        });
    }
    Ok(out)
}

/// One playlist, by MBID.
pub async fn fetch(mbid: &str, token: Option<&str>) -> Result<ImportPlaylist, PlaylistImportError> {
    let doc = get_json(&format!("{API}/1/playlist/{mbid}"), token).await?;
    let pl = &doc["playlist"];
    let tracks: Vec<ImportTrack> = pl["track"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(map_track)
        .collect();
    if tracks.is_empty() {
        return Err(PlaylistImportError::EmptyPlaylist);
    }
    Ok(ImportPlaylist {
        provider: ImportProvider::ListenBrainz,
        provider_id: mbid.to_string(),
        name: pl["title"].as_str().unwrap_or("ListenBrainz playlist").to_string(),
        description: pl["annotation"]
            .as_str()
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty()),
        tracks,
    })
}

/// JSPF track -> `ImportTrack`. Defensive: a shape change drops the row
/// instead of failing the import.
fn map_track(t: &Value) -> Option<ImportTrack> {
    let title = t["title"].as_str().unwrap_or("").trim().to_string();
    if title.is_empty() {
        return None;
    }
    // `identifier` is the recording MBID as a URI; it goes to `provider_id` and
    // NEVER to `isrc` — JSPF carries no ISRC, and an MBID in that field would
    // misfire the matcher's exact-ISRC short circuit.
    let recording_mbid = t["identifier"]
        .as_str()
        .or_else(|| t["identifier"][0].as_str())
        .and_then(|s| s.rsplit('/').next())
        .filter(|s| is_mbid(s))
        .map(str::to_string);
    Some(ImportTrack {
        title,
        artist: t["creator"].as_str().unwrap_or("").trim().to_string(),
        album: t["album"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        // JSPF duration is MILLISECONDS (it is XSPF in JSON) — no x1000.
        duration_ms: t["duration"].as_u64().filter(|d| *d > 0),
        isrc: None,
        provider_url: recording_mbid
            .as_deref()
            .map(|m| format!("https://listenbrainz.org/track/{m}")),
        provider_id: recording_mbid,
    })
}

async fn get_json(url: &str, token: Option<&str>) -> Result<Value, PlaylistImportError> {
    let mut req = http().get(url).header(reqwest::header::USER_AGENT, LB_USER_AGENT);
    if let Some(t) = token.map(str::trim).filter(|t| !t.is_empty()) {
        req = req.header(reqwest::header::AUTHORIZATION, format!("Token {t}"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| PlaylistImportError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(PlaylistImportError::Http(format!(
            "ListenBrainz returned {}",
            resp.status()
        )));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| PlaylistImportError::Parse(format!("ListenBrainz: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MBID: &str = "1a2b3c4d-5e6f-4a1b-8c9d-0e1f2a3b4c5d";

    #[test]
    fn detect_reads_urls_mbids_and_usernames() {
        assert_eq!(
            detect(&format!("https://listenbrainz.org/playlist/{MBID}")),
            Some(LbTarget::Mbid(MBID.into()))
        );
        // Trailing slash and query survive.
        assert_eq!(
            detect(&format!("https://listenbrainz.org/playlist/{MBID}/?x=1")),
            Some(LbTarget::Mbid(MBID.into()))
        );
        assert_eq!(
            detect("https://listenbrainz.org/user/rob"),
            Some(LbTarget::User("rob".into()))
        );
        assert_eq!(detect(MBID), Some(LbTarget::Mbid(MBID.into())));
        assert_eq!(detect("  rob_2  "), Some(LbTarget::User("rob_2".into())));
        assert_eq!(detect(""), None);
        assert_eq!(detect("not a name"), None);
        assert_eq!(detect("https://example.com/x"), None);
    }

    #[test]
    fn jspf_duration_is_milliseconds() {
        let t: Value = serde_json::from_str(
            r#"{"title":"Song","creator":"Band","album":"Al","duration":214000}"#,
        )
        .unwrap();
        let m = map_track(&t).unwrap();
        assert_eq!(m.duration_ms, Some(214_000));
        assert_eq!(m.artist, "Band");
        assert_eq!(m.album.as_deref(), Some("Al"));
    }

    #[test]
    fn the_recording_mbid_lands_in_provider_id_never_in_isrc() {
        let t: Value = serde_json::from_str(&format!(
            r#"{{"title":"Song","creator":"B","identifier":"https://musicbrainz.org/recording/{MBID}"}}"#
        ))
        .unwrap();
        let m = map_track(&t).unwrap();
        assert_eq!(m.provider_id.as_deref(), Some(MBID));
        assert_eq!(m.isrc, None);
    }

    #[test]
    fn a_titleless_track_is_dropped_rather_than_failing_the_import() {
        let t: Value = serde_json::from_str(r#"{"creator":"B"}"#).unwrap();
        assert!(map_track(&t).is_none());
    }
}
