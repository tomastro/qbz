// crates/qbzd/src/cli/play.rs — the `qbzd play [CONTENT]` verb (02 §2.3).
//
// Bare `qbzd play` resumes / cold-starts the current queue (the shipped P0
// route POST /api/playback/play, delegated to `transport::play` so there is
// one resume implementation). With a content argument it plays that content
// via POST /api/play: `track:ID` | `album:ID` | `artist:ID` | `playlist:ID` |
// a Qobuz URL | a bare numeric track id. Exit codes come from the frozen table
// via `CliError`; a malformed selector is a local usage error (exit 2).
use serde_json::Value;

use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

pub async fn play(host: Option<String>, content: Option<String>, roots: &ProfileRoots) -> i32 {
    let content = match content {
        // Bare `play` = resume, the shipped behaviour (one implementation).
        None => return crate::cli::transport::play(host, roots).await,
        Some(c) => c,
    };

    let body = match to_body(&content) {
        Ok(b) => b,
        Err(msg) => {
            eprintln!("error: {msg}");
            eprintln!(
                "  → try: qbzd play album:<ID> | track:<ID> | artist:<ID> | playlist:<ID> | <qobuz-url>"
            );
            return 2;
        }
    };

    let client = ApiClient::new(host, roots);
    match client.post("/api/play", body).await {
        Ok(v) => {
            println!("{}", render(&v));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

// ============================ internals ============================

/// Map a content token to the POST /api/play body. A URL passes through (the
/// daemon resolves it); `kind:ID` prefixes map to the typed id fields; a bare
/// number is a track id.
fn to_body(content: &str) -> Result<Value, String> {
    let c = content.trim();
    if c.starts_with("http://") || c.starts_with("https://") {
        return Ok(serde_json::json!({ "url": c }));
    }
    if let Some(id) = c.strip_prefix("track:") {
        return parse_u64(id).map(|n| serde_json::json!({ "track_id": n }));
    }
    if let Some(id) = c.strip_prefix("album:") {
        if id.is_empty() {
            return Err("album id is empty".into());
        }
        return Ok(serde_json::json!({ "album_id": id }));
    }
    if let Some(id) = c.strip_prefix("artist:") {
        return parse_u64(id).map(|n| serde_json::json!({ "artist_id": n }));
    }
    if let Some(id) = c.strip_prefix("playlist:") {
        return parse_u64(id).map(|n| serde_json::json!({ "playlist_id": n }));
    }
    if let Ok(n) = c.parse::<u64>() {
        return Ok(serde_json::json!({ "track_id": n }));
    }
    Err(format!("unrecognized content '{c}'"))
}

fn parse_u64(s: &str) -> Result<u64, String> {
    s.parse::<u64>()
        .map_err(|_| format!("'{s}' is not a numeric id"))
}

/// `queued N · playing "Title" — Artist`, degrading gracefully if the response
/// omits track fields.
fn render(v: &Value) -> String {
    let queued = v.get("queued").and_then(|x| x.as_u64()).unwrap_or(0);
    let track = v.get("track");
    let title = track
        .and_then(|t| t.get("title"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let artist = track
        .and_then(|t| t.get("artist"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if title.is_empty() {
        format!("queued {queued} · playing")
    } else if artist.is_empty() {
        format!("queued {queued} · playing \"{title}\"")
    } else {
        format!("queued {queued} · playing \"{title}\" — {artist}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_body_maps_prefixes_and_urls() {
        assert_eq!(to_body("track:42").unwrap(), serde_json::json!({"track_id": 42}));
        assert_eq!(to_body("album:0060254728933").unwrap(), serde_json::json!({"album_id": "0060254728933"}));
        assert_eq!(to_body("artist:9").unwrap(), serde_json::json!({"artist_id": 9}));
        assert_eq!(to_body("playlist:7").unwrap(), serde_json::json!({"playlist_id": 7}));
        assert_eq!(
            to_body("https://open.qobuz.com/album/abc").unwrap(),
            serde_json::json!({"url": "https://open.qobuz.com/album/abc"})
        );
    }

    #[test]
    fn to_body_bare_number_is_a_track_id() {
        assert_eq!(to_body("176544871").unwrap(), serde_json::json!({"track_id": 176544871u64}));
    }

    #[test]
    fn to_body_rejects_non_numeric_prefixed_ids_and_garbage() {
        assert!(to_body("track:abc").is_err());
        assert!(to_body("artist:").is_err());
        assert!(to_body("album:").is_err());
        assert!(to_body("nonsense").is_err());
    }

    #[test]
    fn render_reads_queued_and_track() {
        let v = serde_json::json!({"queued": 9, "started": true,
            "track": {"title": "So What", "artist": "Miles Davis"}});
        assert_eq!(render(&v), "queued 9 · playing \"So What\" — Miles Davis");
        let bare = serde_json::json!({"queued": 1, "track": {"title": "X"}});
        assert_eq!(render(&bare), "queued 1 · playing \"X\"");
    }
}
