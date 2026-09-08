// crates/qbzd/src/cli/browse.rs — the catalog READ verbs (02 §2.3):
// `qbzd album`, `qbzd artist`, `qbzd similar`, `qbzd suggest`. Each is a
// stateless renderer over one GET request. Three modes on every verb: default
// human list, `--ids` (ids one-per-line — the composition currency), `--json`
// (the raw payload). The human/`--ids` views walk the payload generically
// (items/tracks arrays); `--json` is the exact, complete contract.
use std::io::Read;

use serde_json::Value;

use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

/// `qbzd album <ALBUM_ID> [--suggest] [--ids] [--json]`.
pub async fn album(host: Option<String>, id: String, suggest: bool, ids: bool, json: bool, roots: &ProfileRoots) -> i32 {
    let path = format!("/api/album?id={}&suggest={}", urlencoding::encode(&id), if suggest { 1 } else { 0 });
    get_and_render(host, roots, &path, ids, json).await
}

/// `qbzd artist <ARTIST_ID> [--top|--albums] [--limit N] [--ids] [--json]`.
#[allow(clippy::too_many_arguments)]
pub async fn artist(host: Option<String>, id: u64, top: bool, albums: bool, limit: u32, ids: bool, json: bool, roots: &ProfileRoots) -> i32 {
    let view = if albums { "albums" } else if top { "top" } else { "page" };
    let path = format!("/api/artist?id={id}&view={view}&limit={limit}");
    get_and_render(host, roots, &path, ids, json).await
}

/// `qbzd similar <artist:ID | album:ID> [--limit N] [--ids] [--json]`.
pub async fn similar(host: Option<String>, selector: String, limit: u32, ids: bool, json: bool, roots: &ProfileRoots) -> i32 {
    let path = match to_similar_query(&selector, limit) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("error: {msg}");
            eprintln!("  → usage: qbzd similar artist:<ID> | album:<ID>");
            return 2;
        }
    };
    get_and_render(host, roots, &path, ids, json).await
}

/// `qbzd suggest [--seed <ID,ID> | --seed -] [--limit N] [--ids] [--json]`.
/// No `--seed` = the daemon seeds from the current queue. `--seed -` reads ids
/// one-per-line from stdin.
pub async fn suggest(host: Option<String>, seed: Option<String>, limit: u32, ids: bool, json: bool, roots: &ProfileRoots) -> i32 {
    let seed_param = match seed.as_deref() {
        Some("-") => Some(read_stdin_ids()),
        Some(s) => Some(s.to_string()),
        None => None,
    };
    let mut path = format!("/api/suggest?limit={limit}");
    if let Some(s) = seed_param.filter(|s| !s.is_empty()) {
        path.push_str(&format!("&seed={}", urlencoding::encode(&s)));
    }
    get_and_render(host, roots, &path, ids, json).await
}

// ============================ shared ============================

async fn get_and_render(host: Option<String>, roots: &ProfileRoots, path: &str, ids: bool, json: bool) -> i32 {
    let client = ApiClient::new(host, roots);
    let payload = match client.get(path).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return e.exit_code();
        }
    };
    if json {
        println!("{}", serde_json::to_string(&payload).unwrap_or_default());
    } else if ids {
        for id in collect_ids(&payload) {
            println!("{id}");
        }
    } else {
        print!("{}", render(&payload));
    }
    0
}

fn to_similar_query(selector: &str, limit: u32) -> Result<String, String> {
    let s = selector.trim();
    if let Some(id) = s.strip_prefix("artist:") {
        let id: u64 = id.parse().map_err(|_| format!("'{id}' is not a numeric artist id"))?;
        return Ok(format!("/api/similar?artist={id}&limit={limit}"));
    }
    if let Some(id) = s.strip_prefix("album:") {
        if id.is_empty() {
            return Err("album id is empty".into());
        }
        return Ok(format!("/api/similar?album={}&limit={limit}", urlencoding::encode(id)));
    }
    Err(format!("unrecognized selector '{s}'"))
}

fn read_stdin_ids() -> String {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    buf.split_whitespace().collect::<Vec<_>>().join(",")
}

/// Generic human list: every object carried in an `items`/`tracks` array,
/// rendered as `id  Artist — Title`. Robust across the album/artist/similar/
/// suggest payload shapes (the `--json` output is the exact contract). Shared
/// with `cli::fav` (favorites payloads are the same items-array shape).
pub(crate) fn render(p: &Value) -> String {
    let mut items: Vec<&Value> = Vec::new();
    walk(p, &mut items);
    if items.is_empty() {
        return "no results\n".to_string();
    }
    let mut out = String::new();
    for it in items {
        let id = id_str(it.get("id"));
        if id.is_empty() {
            continue;
        }
        let title = it
            .get("title")
            .or_else(|| it.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("(untitled)");
        match secondary(it) {
            Some(s) => out.push_str(&format!("{id}  {s} — {title}\n")),
            None => out.push_str(&format!("{id}  {title}\n")),
        }
    }
    if out.is_empty() {
        out.push_str("no results\n");
    }
    out
}

pub(crate) fn collect_ids(p: &Value) -> Vec<String> {
    let mut items: Vec<&Value> = Vec::new();
    walk(p, &mut items);
    items
        .iter()
        .map(|it| id_str(it.get("id")))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Collect objects held in `items`/`tracks` arrays anywhere in the payload.
/// Nested reference objects (a track's `artist`/`album`) are NOT under those
/// keys, so they are not over-collected.
fn walk<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if (k == "items" || k == "tracks") && val.is_array() {
                    if let Value::Array(arr) = val {
                        for e in arr {
                            if e.is_object() && e.get("id").is_some() {
                                out.push(e);
                            }
                        }
                    }
                }
                walk(val, out);
            }
        }
        Value::Array(arr) => {
            for e in arr {
                walk(e, out);
            }
        }
        _ => {}
    }
}

fn secondary(it: &Value) -> Option<&str> {
    it.get("artist")
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .or_else(|| it.get("performer").and_then(|a| a.get("name")).and_then(|v| v.as_str()))
}

fn id_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_similar_query_builds_artist_and_album_paths() {
        assert_eq!(to_similar_query("artist:123", 10).unwrap(), "/api/similar?artist=123&limit=10");
        assert_eq!(to_similar_query("album:abc", 5).unwrap(), "/api/similar?album=abc&limit=5");
        assert!(to_similar_query("artist:xy", 10).is_err());
        assert!(to_similar_query("nope", 10).is_err());
    }

    #[test]
    fn walk_collects_items_and_top_level_tracks_only() {
        // album shape: album.tracks.items ; plus a nested track.album (must NOT
        // be collected — it is not under an items/tracks array key).
        let album = serde_json::json!({
            "album": {"id": "A", "title": "Al",
                "tracks": {"items": [
                    {"id": 1, "title": "T1", "album": {"id": "A", "title": "Al"}},
                    {"id": 2, "title": "T2"}
                ]}}
        });
        assert_eq!(collect_ids(&album), vec!["1", "2"]);

        // suggest shape: top-level tracks array of Track.
        let suggest = serde_json::json!({"tracks": [{"id": 9, "title": "S"}]});
        assert_eq!(collect_ids(&suggest), vec!["9"]);
    }

    #[test]
    fn render_empty_says_no_results() {
        assert_eq!(render(&serde_json::json!({"album": Value::Null})), "no results\n");
    }
}
