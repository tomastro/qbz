// crates/qbzd/src/cli/search.rs — the `qbzd search` verb (02-cli-and-api.md
// §2.3). A stateless renderer over one `GET /api/search` (§1.1): one verb, one
// request. Three output modes — human top-hits table (default), `--ids` (ids
// one-per-line, the composition currency for `... | qbzd queue add -`), and
// `--json` (the raw api_version-stamped payload). Exit codes come from the
// frozen table via `CliError` (§1.3): 0 · 3 unreachable · 4 needs_auth · 1 else.
use serde_json::Value;

use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

/// `qbzd search <QUERY> [--type all|albums|tracks|artists|playlists]
/// [--limit N] [--offset N] [--ids] [--json]`.
#[allow(clippy::too_many_arguments)]
pub async fn search(
    host: Option<String>,
    query: String,
    stype: String,
    limit: u32,
    offset: u32,
    ids: bool,
    json: bool,
    roots: &ProfileRoots,
) -> i32 {
    let client = ApiClient::new(host, roots);
    let path = format!(
        "/api/search?q={}&type={}&limit={}&offset={}",
        urlencoding::encode(&query),
        urlencoding::encode(&stype),
        limit,
        offset,
    );
    let payload = match client.get(&path).await {
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

// ============================ rendering ============================

/// The category order used by both the human table and `--ids`. `tracks` leads
/// so `qbzd search "..." --type tracks --ids | qbzd queue add -` is the obvious
/// pipeline; the order is stable so scripts can rely on it.
const CATEGORIES: [(&str, &str); 4] = [
    ("tracks", "TRACKS"),
    ("albums", "ALBUMS"),
    ("artists", "ARTISTS"),
    ("playlists", "PLAYLISTS"),
];

/// Human top-hits table. Defensive against missing optional fields — a category
/// with no results is skipped; an item with no title/artist degrades gracefully
/// rather than erroring (the `--json` payload is the exact contract; this is a
/// convenience view).
fn render(p: &Value) -> String {
    let mut out = String::new();
    for (key, label) in CATEGORIES {
        let page = match p.get(key) {
            Some(v) if v.is_object() => v,
            _ => continue,
        };
        let items = match page.get("items").and_then(|v| v.as_array()) {
            Some(a) if !a.is_empty() => a,
            _ => continue,
        };
        let total = page
            .get("total")
            .and_then(|v| v.as_u64())
            .unwrap_or(items.len() as u64);
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{label} ({total})\n"));
        for it in items {
            let id = id_str(it.get("id"));
            let title = it
                .get("title")
                .or_else(|| it.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("(untitled)");
            match secondary_name(it) {
                Some(s) => out.push_str(&format!("  {id}  {s} — {title}\n")),
                None => out.push_str(&format!("  {id}  {title}\n")),
            }
        }
    }
    if out.is_empty() {
        out.push_str("no results\n");
    }
    out
}

/// The ids of every returned item, in `CATEGORIES` order — the composition
/// currency. For a typed search only one category is populated; for `--type
/// all` this emits all ids (album ids are strings, track/artist ids numbers —
/// realistic pipelines use a typed search, but mixing is not an error).
fn collect_ids(p: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for (key, _label) in CATEGORIES {
        if let Some(items) = p
            .get(key)
            .and_then(|v| v.get("items"))
            .and_then(|v| v.as_array())
        {
            for it in items {
                let id = id_str(it.get("id"));
                if !id.is_empty() {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

/// A best-effort "artist" line: an album/track carries `artist.name` or
/// `performer.name`; an artist/playlist has neither (its `name` is the title).
fn secondary_name(it: &Value) -> Option<&str> {
    it.get("artist")
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            it.get("performer")
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
        })
}

/// Stringify an id `Value` without quotes: a string id (album) prints bare, a
/// numeric id (track/artist/playlist) prints as its integer. Missing/other →
/// empty (skipped by the callers).
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

    fn payload() -> Value {
        serde_json::json!({
            "query": "feather", "type": "all", "limit": 20, "offset": 0,
            "albums": {"items": [
                {"id": "c9vd8vvvrbpkc", "title": "Light as a Feather",
                 "artist": {"name": "Chick Corea"}}
            ], "total": 27, "offset": 0, "limit": 20},
            "tracks": {"items": [
                {"id": 176544871, "title": "Spain", "performer": {"name": "Chick Corea"}}
            ], "total": 5, "offset": 0, "limit": 20},
            "artists": Value::Null,
            "playlists": Value::Null
        })
    }

    #[test]
    fn collect_ids_leads_with_tracks_then_albums() {
        // CATEGORIES order: tracks first (queue-pipe target), then albums.
        assert_eq!(collect_ids(&payload()), vec!["176544871", "c9vd8vvvrbpkc"]);
    }

    #[test]
    fn id_str_handles_string_and_numeric_ids_without_quotes() {
        assert_eq!(
            id_str(Some(&serde_json::json!("c9vd8vvvrbpkc"))),
            "c9vd8vvvrbpkc"
        );
        assert_eq!(id_str(Some(&serde_json::json!(176544871u64))), "176544871");
        assert_eq!(id_str(Some(&Value::Null)), "");
        assert_eq!(id_str(None), "");
    }

    #[test]
    fn render_shows_present_categories_with_ids_and_names() {
        let out = render(&payload());
        assert!(out.contains("TRACKS (5)"), "{out}");
        assert!(out.contains("  176544871  Chick Corea — Spain"), "{out}");
        assert!(out.contains("ALBUMS (27)"), "{out}");
        assert!(
            out.contains("  c9vd8vvvrbpkc  Chick Corea — Light as a Feather"),
            "{out}"
        );
        // null categories are skipped entirely.
        assert!(!out.contains("ARTISTS"), "{out}");
        assert!(!out.contains("PLAYLISTS"), "{out}");
    }

    #[test]
    fn render_empty_payload_says_no_results() {
        let empty = serde_json::json!({"albums": Value::Null, "tracks": Value::Null});
        assert_eq!(render(&empty), "no results\n");
    }
}
