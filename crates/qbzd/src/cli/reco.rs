// crates/qbzd/src/cli/reco.rs — the `qbzd reco playlist <ID>` verb (CONSOLE
// extension). Suggested Songs for a playlist (no listening history needed).
// `--ids` emits the suggested track ids one-per-line (pipe into `queue add -`
// or `playlist add`).
use serde_json::Value;

use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

/// `qbzd reco playlist <ID> [--limit N] [--ids] [--json]`.
pub async fn playlist(host: Option<String>, id: u64, limit: Option<u32>, ids: bool, json: bool, roots: &ProfileRoots) -> i32 {
    let mut body = serde_json::json!({ "playlist_id": id });
    if let Some(n) = limit {
        body["limit"] = serde_json::json!(n);
    }
    let client = ApiClient::new(host, roots);
    match client.post("/api/reco/playlist", body).await {
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&v).unwrap_or_default());
            } else if ids {
                for tid in track_ids(&v) {
                    println!("{tid}");
                }
            } else {
                print!("{}", render(&v));
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

// ============================ internals ============================

/// SuggestedTrack carries `track_id` (not `id`), so `--ids` reads that field.
fn track_ids(v: &Value) -> Vec<String> {
    v.get("tracks")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.get("track_id").and_then(|x| x.as_u64()).map(|n| n.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn render(v: &Value) -> String {
    let tracks = v.get("tracks").and_then(|t| t.as_array());
    match tracks {
        Some(a) if !a.is_empty() => {
            let mut out = String::new();
            for t in a {
                let id = t.get("track_id").and_then(|x| x.as_u64()).unwrap_or(0);
                let title = t.get("title").and_then(|x| x.as_str()).unwrap_or("");
                let artist = t.get("artist_name").and_then(|x| x.as_str()).unwrap_or("");
                out.push_str(&format!("{id}  {artist} — {title}\n"));
            }
            let similar = v.get("similar_artists_count").and_then(|x| x.as_u64()).unwrap_or(0);
            out.push_str(&format!("{} suggestions from {similar} similar artists\n", a.len()));
            out
        }
        _ => "no suggestions (try a playlist with more resolvable artists)\n".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_ids_reads_track_id_field() {
        let v = serde_json::json!({"tracks": [
            {"track_id": 111, "title": "A", "artist_name": "X"},
            {"track_id": 222, "title": "B", "artist_name": "Y"}
        ]});
        assert_eq!(track_ids(&v), vec!["111", "222"]);
    }

    #[test]
    fn render_lists_suggestions_or_says_none() {
        let v = serde_json::json!({"tracks": [{"track_id": 5, "title": "So What", "artist_name": "Miles Davis"}], "similar_artists_count": 3});
        let out = render(&v);
        assert!(out.contains("5  Miles Davis — So What"), "{out}");
        assert!(out.contains("1 suggestions from 3 similar artists"), "{out}");
        assert_eq!(render(&serde_json::json!({"tracks": []})), "no suggestions (try a playlist with more resolvable artists)\n");
    }
}
