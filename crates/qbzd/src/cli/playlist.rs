// crates/qbzd/src/cli/playlist.rs — the `qbzd playlist list|show` verbs
// (02 §2.3). Reads only in this slice. `playlist show --ids` emits the
// playlist's track ids (pipe into `queue add -`).
use std::io::Read;

use serde_json::Value;

use crate::cli::browse::{collect_ids, render};
use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

/// `qbzd playlist list [--json]`.
pub async fn list(host: Option<String>, json: bool, roots: &ProfileRoots) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.get("/api/playlists").await {
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&v).unwrap_or_default());
            } else {
                print!("{}", render_list(&v));
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// `qbzd playlist show <ID> [--ids] [--json]`.
pub async fn show(host: Option<String>, id: u64, ids: bool, json: bool, roots: &ProfileRoots) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.get(&format!("/api/playlist?id={id}")).await {
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&v).unwrap_or_default());
            } else if ids {
                for tid in collect_ids(&v) {
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

/// `qbzd playlist create <NAME> [--desc D] [--public]`.
pub async fn create(host: Option<String>, name: String, desc: Option<String>, public: bool, roots: &ProfileRoots) -> i32 {
    let mut body = serde_json::json!({ "name": name, "public": public });
    if let Some(d) = desc {
        body["description"] = Value::String(d);
    }
    post(host, roots, "/api/playlist/create", body, |v| {
        let pl = v.get("playlist");
        let id = pl.and_then(|p| p.get("id")).and_then(|x| x.as_u64()).unwrap_or(0);
        let nm = pl.and_then(|p| p.get("name")).and_then(|x| x.as_str()).unwrap_or("");
        format!("created playlist {id} \"{nm}\"")
    })
    .await
}

/// `qbzd playlist edit <ID> [--name N] [--desc D] [--public|--private]`.
#[allow(clippy::too_many_arguments)]
pub async fn edit(host: Option<String>, id: u64, name: Option<String>, desc: Option<String>, public: bool, private: bool, roots: &ProfileRoots) -> i32 {
    if public && private {
        eprintln!("error: --public and --private are mutually exclusive");
        return 2;
    }
    let mut body = serde_json::json!({ "id": id });
    if let Some(n) = name {
        body["name"] = Value::String(n);
    }
    if let Some(d) = desc {
        body["description"] = Value::String(d);
    }
    if public {
        body["public"] = Value::Bool(true);
    } else if private {
        body["public"] = Value::Bool(false);
    }
    post(host, roots, "/api/playlist/update", body, |_| "playlist updated".to_string()).await
}

/// `qbzd playlist rm <ID> --yes`.
pub async fn rm(host: Option<String>, id: u64, yes: bool, roots: &ProfileRoots) -> i32 {
    if !yes {
        eprintln!("error: refusing to delete without --yes");
        eprintln!("  → qbzd playlist rm {id} --yes");
        return 2;
    }
    post(host, roots, "/api/playlist/delete", serde_json::json!({ "id": id }), move |_| {
        format!("deleted playlist {id}")
    })
    .await
}

/// `qbzd playlist add <ID> <TRACK_IDS...|->`.
pub async fn add(host: Option<String>, id: u64, track_ids: Vec<String>, roots: &ProfileRoots) -> i32 {
    let ids = match resolve_ids(track_ids) {
        Ok(i) => i,
        Err(m) => {
            eprintln!("error: {m}");
            return 2;
        }
    };
    let body = serde_json::json!({ "id": id, "track_ids": ids });
    post(host, roots, "/api/playlist/tracks/add", body, |v| {
        let n = v.get("added").and_then(|x| x.as_u64()).unwrap_or(0);
        format!("added {n} track(s)")
    })
    .await
}

/// `qbzd playlist remove <ID> <TRACK_IDS...>` (plain track ids; the daemon
/// resolves them to per-playlist row ids).
pub async fn remove(host: Option<String>, id: u64, track_ids: Vec<String>, roots: &ProfileRoots) -> i32 {
    let ids = match resolve_ids(track_ids) {
        Ok(i) => i,
        Err(m) => {
            eprintln!("error: {m}");
            return 2;
        }
    };
    let body = serde_json::json!({ "id": id, "track_ids": ids });
    post(host, roots, "/api/playlist/tracks/remove", body, |v| {
        let n = v.get("removed").and_then(|x| x.as_u64()).unwrap_or(0);
        format!("removed {n} track(s)")
    })
    .await
}

// ============================ internals ============================

async fn post<F: Fn(&Value) -> String>(host: Option<String>, roots: &ProfileRoots, path: &str, body: Value, render_ok: F) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.post(path, body).await {
        Ok(v) => {
            println!("{}", render_ok(&v));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// Track ids from positional args; a single `-` reads them from stdin
/// (whitespace-separated). Every id must be numeric.
fn resolve_ids(args: Vec<String>) -> Result<Vec<u64>, String> {
    let raw: Vec<String> = if args.len() == 1 && args[0] == "-" {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        buf.split_whitespace().map(|s| s.to_string()).collect()
    } else {
        args
    };
    if raw.is_empty() {
        return Err("no track ids given".into());
    }
    let mut ids = Vec::with_capacity(raw.len());
    for s in raw {
        let n: u64 = s.parse().map_err(|_| format!("'{s}' is not a numeric track id"))?;
        ids.push(n);
    }
    Ok(ids)
}

/// The collection view: `id  Name (N tracks)` per playlist. The array is under
/// `playlists` (not an items/tracks key), so it has its own small renderer.
fn render_list(v: &Value) -> String {
    match v.get("playlists").and_then(|p| p.as_array()) {
        Some(a) if !a.is_empty() => {
            let mut out = String::new();
            for pl in a {
                let id = pl.get("id").and_then(|x| x.as_u64()).map(|n| n.to_string()).unwrap_or_default();
                let name = pl.get("name").and_then(|x| x.as_str()).unwrap_or("(untitled)");
                let count = pl.get("tracks_count").and_then(|x| x.as_u64()).unwrap_or(0);
                out.push_str(&format!("{id}  {name} ({count} tracks)\n"));
            }
            out
        }
        _ => "no playlists\n".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_list_shows_id_name_count() {
        let v = serde_json::json!({"playlists": [
            {"id": 987, "name": "Fusion", "tracks_count": 42},
            {"id": 12, "name": "Chill"}
        ]});
        let out = render_list(&v);
        assert!(out.contains("987  Fusion (42 tracks)"), "{out}");
        assert!(out.contains("12  Chill (0 tracks)"), "{out}");
    }

    #[test]
    fn render_list_empty() {
        assert_eq!(render_list(&serde_json::json!({"playlists": []})), "no playlists\n");
    }
}
