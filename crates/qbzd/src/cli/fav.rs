// crates/qbzd/src/cli/fav.rs — the `qbzd fav list|add|remove` verbs (02 §2.3).
// The fav_type contract is SINGULAR (track|album|artist); the server hides the
// plural-read trap. `fav add track --current` favorites the now-playing track.
use serde_json::Value;

use crate::cli::browse::{collect_ids, render};
use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

/// `qbzd fav list [--type track|album|artist] [--ids] [--json]`.
pub async fn list(host: Option<String>, kind: Option<String>, ids: bool, json: bool, roots: &ProfileRoots) -> i32 {
    let t = kind.unwrap_or_else(|| "track".to_string());
    let path = format!("/api/favorites?type={}", urlencoding::encode(&t));
    let client = ApiClient::new(host, roots);
    match client.get(&path).await {
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&v).unwrap_or_default());
            } else if ids {
                for id in collect_ids(&v) {
                    println!("{id}");
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

/// `qbzd fav add <track|album|artist> <ID>` · `qbzd fav add track --current`.
pub async fn add(host: Option<String>, fav_type: String, id: Option<String>, current: bool, roots: &ProfileRoots) -> i32 {
    let mut body = serde_json::json!({ "fav_type": fav_type });
    if current {
        body["current"] = Value::Bool(true);
    } else {
        match id {
            Some(i) => body["item_id"] = Value::String(i),
            None => {
                eprintln!("error: fav add needs an id (or --current for a track)");
                eprintln!("  → usage: qbzd fav add track <ID> | qbzd fav add track --current");
                return 2;
            }
        }
    }
    post(host, roots, "/api/favorites/add", body, "favorited").await
}

/// `qbzd fav remove <track|album|artist> <ID>`.
pub async fn remove(host: Option<String>, fav_type: String, id: String, roots: &ProfileRoots) -> i32 {
    let body = serde_json::json!({ "fav_type": fav_type, "item_id": id });
    post(host, roots, "/api/favorites/remove", body, "unfavorited").await
}

// ============================ internals ============================

async fn post(host: Option<String>, roots: &ProfileRoots, path: &str, body: Value, verb: &str) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.post(path, body).await {
        Ok(v) => {
            let t = v.get("type").and_then(|x| x.as_str()).unwrap_or("item");
            let id = v.get("item_id").and_then(|x| x.as_str()).unwrap_or("");
            println!("{verb} {t} {id}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}
