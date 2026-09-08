// crates/qbzd/src/cli/mode.rs — the `qbzd shuffle` and `qbzd repeat` verbs
// (02 §2.2 CONSOLE additions). Each is one POST to a playback route; state
// also surfaces in `qbzd now`/`status`.
use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

/// `qbzd shuffle [on|off|toggle]` (bare = toggle).
pub async fn shuffle(host: Option<String>, mode: Option<String>, roots: &ProfileRoots) -> i32 {
    let m = mode.unwrap_or_else(|| "toggle".to_string());
    let client = ApiClient::new(host, roots);
    match client.post("/api/playback/shuffle", serde_json::json!({ "mode": m })).await {
        Ok(v) => {
            let on = v.get("shuffle").and_then(|x| x.as_bool()).unwrap_or(false);
            println!("shuffle {}", if on { "on" } else { "off" });
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// `qbzd repeat <off|all|one>`.
pub async fn repeat(host: Option<String>, mode: String, roots: &ProfileRoots) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.post("/api/playback/repeat", serde_json::json!({ "mode": mode })).await {
        Ok(v) => {
            let m = v.get("repeat").and_then(|x| x.as_str()).unwrap_or("off");
            println!("repeat {m}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}
