// crates/qbzd/src/cli/lyrics.rs — the `qbzd lyrics [TRACK_ID]` verb (02 §2.3).
// Bare = the current track. Default prints plain text lines; `--synced` prefixes
// each line with its `[mm:ss.cc]` timestamp (when the doc is time-synced);
// `--json` is the raw {track_id, synced, lines} payload.
use serde_json::Value;

use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

pub async fn lyrics(host: Option<String>, track_id: Option<u64>, synced: bool, json: bool, roots: &ProfileRoots) -> i32 {
    let id = track_id.map(|n| n.to_string()).unwrap_or_else(|| "current".to_string());
    let client = ApiClient::new(host, roots);
    match client.get(&format!("/api/lyrics?id={id}")).await {
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&v).unwrap_or_default());
            } else {
                print!("{}", render(&v, synced));
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

fn render(v: &Value, want_synced: bool) -> String {
    let is_synced = v.get("synced").and_then(|s| s.as_bool()).unwrap_or(false);
    let lines = v.get("lines").and_then(|l| l.as_array());
    match lines {
        Some(a) if !a.is_empty() => {
            let mut out = String::new();
            for line in a {
                let text = line.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if want_synced && is_synced {
                    match line.get("time_ms").and_then(|t| t.as_i64()) {
                        Some(ms) => out.push_str(&format!("[{}] {text}\n", fmt_ts(ms))),
                        None => out.push_str(&format!("{text}\n")),
                    }
                } else {
                    out.push_str(&format!("{text}\n"));
                }
            }
            out
        }
        _ => "no lyrics\n".to_string(),
    }
}

/// `mm:ss.cc` (centiseconds) from milliseconds — the LRC timestamp form.
fn fmt_ts(ms: i64) -> String {
    let ms = ms.max(0);
    let secs = ms / 1000;
    format!("{:02}:{:02}.{:02}", secs / 60, secs % 60, (ms % 1000) / 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_ts_formats_lrc_timestamps() {
        assert_eq!(fmt_ts(12_400), "00:12.40");
        assert_eq!(fmt_ts(75_020), "01:15.02");
        assert_eq!(fmt_ts(0), "00:00.00");
    }

    #[test]
    fn render_plain_and_synced() {
        let synced = serde_json::json!({"synced": true, "lines": [
            {"time_ms": 12400, "text": "You're everything"},
            {"time_ms": 19850, "text": "From the start"}
        ]});
        assert_eq!(render(&synced, true), "[00:12.40] You're everything\n[00:19.85] From the start\n");
        // without --synced, timestamps are dropped.
        assert_eq!(render(&synced, false), "You're everything\nFrom the start\n");
        // plain doc ignores --synced.
        let plain = serde_json::json!({"synced": false, "lines": [{"text": "Line one"}]});
        assert_eq!(render(&plain, true), "Line one\n");
        assert_eq!(render(&serde_json::json!({"lines": []}), false), "no lyrics\n");
    }
}
