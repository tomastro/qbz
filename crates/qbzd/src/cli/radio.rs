// crates/qbzd/src/cli/radio.rs — the `qbzd radio` verb (02 §2.3). Seed-and-go:
// `qbzd radio artist:<ID> | track:<ID> | album:<ID>` generates a Qobuz radio
// from the seed, replaces the queue with it, and starts playing — all in one
// POST /api/radio.
use serde_json::Value;

use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

pub async fn radio(host: Option<String>, seed: String, roots: &ProfileRoots) -> i32 {
    let body = match to_body(&seed) {
        Ok(b) => b,
        Err(msg) => {
            eprintln!("error: {msg}");
            eprintln!("  → usage: qbzd radio artist:<ID> | track:<ID> | album:<ID>");
            return 2;
        }
    };
    let client = ApiClient::new(host, roots);
    match client.post("/api/radio", body).await {
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

fn to_body(seed: &str) -> Result<Value, String> {
    let s = seed.trim();
    if let Some(id) = s.strip_prefix("artist:") {
        return num(id).map(|n| serde_json::json!({ "artist_id": n }));
    }
    if let Some(id) = s.strip_prefix("track:") {
        return num(id).map(|n| serde_json::json!({ "track_id": n }));
    }
    if let Some(id) = s.strip_prefix("album:") {
        if id.is_empty() {
            return Err("album id is empty".into());
        }
        return Ok(serde_json::json!({ "album_id": id }));
    }
    Err(format!("unrecognized seed '{s}'"))
}

fn num(s: &str) -> Result<u64, String> {
    s.parse::<u64>().map_err(|_| format!("'{s}' is not a numeric id"))
}

fn render(v: &Value) -> String {
    let queued = v.get("queued").and_then(|x| x.as_u64()).unwrap_or(0);
    let track = v.get("track");
    let title = track.and_then(|t| t.get("title")).and_then(|x| x.as_str()).unwrap_or("");
    let artist = track.and_then(|t| t.get("artist")).and_then(|x| x.as_str()).unwrap_or("");
    if title.is_empty() {
        format!("radio queued {queued} · playing")
    } else if artist.is_empty() {
        format!("radio queued {queued} · playing \"{title}\"")
    } else {
        format!("radio queued {queued} · playing \"{title}\" — {artist}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_body_maps_each_seed_kind() {
        assert_eq!(to_body("artist:44042").unwrap(), serde_json::json!({"artist_id": 44042}));
        assert_eq!(to_body("track:9").unwrap(), serde_json::json!({"track_id": 9}));
        assert_eq!(to_body("album:abc").unwrap(), serde_json::json!({"album_id": "abc"}));
        assert!(to_body("artist:xx").is_err());
        assert!(to_body("nope").is_err());
    }
}
