// crates/qbzd/src/cli/art.rs — the `qbzd art` verb (02 §2.3). Current-track
// cover art, resolved through the daemon's own GET /api/artwork/current (302 to
// the Qobuz CDN link stamped on the queue track). This is the shipped caller
// that anchors that route (§3.1.4): a redirect-none request reads the 302
// `Location` (bare `art` prints it, pipe into a viewer), and `--save PATH`
// follows it to download the image (notification icons). No Qobuz session is
// needed — the art URL is unauthenticated and already on the queue track.
use std::time::Duration;

use crate::cli::client::{resolve_host, resolve_token, CliError};
use crate::paths::ProfileRoots;

/// `qbzd art [--save PATH]`. Exit 0 · 3 · 6 (nothing playing / no art).
pub async fn art(host: Option<String>, save: Option<String>, roots: &ProfileRoots) -> i32 {
    let target = resolve_host(host);
    let token = resolve_token(&target, roots);
    let base = format!("http://{}", target.addr);

    // Redirect-none so we can read the 302 Location (the canonical current-art
    // URL) instead of transparently following it to the CDN.
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let mut req = client.get(format!("{base}/api/artwork/current"));
    if let Some(t) = &token {
        req = req.bearer_auth(t);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let err = if e.is_connect() || e.is_timeout() {
                CliError::Unreachable(target.addr.clone())
            } else {
                CliError::Runtime(format!("request failed: {e}"))
            };
            eprintln!("{err}");
            return err.exit_code();
        }
    };

    let status = resp.status().as_u16();
    if status == 404 {
        eprintln!("error: no artwork for the current track");
        eprintln!("  → is something playing?  qbzd now");
        return 6;
    }
    if status != 302 {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("error: daemon returned {status}: {}", body.trim());
        return 1;
    }
    let url = match resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
    {
        Some(u) => u.to_string(),
        None => {
            eprintln!("error: daemon redirected without a Location");
            return 1;
        }
    };

    match save {
        None => {
            println!("{url}");
            0
        }
        Some(path) => match download(&url, &path).await {
            Ok(()) => {
                println!("saved {path}");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
    }
}

async fn download(url: &str, path: &str) -> Result<(), String> {
    let resp = reqwest::get(url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("cover download failed: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    std::fs::write(path, &bytes).map_err(|e| e.to_string())
}
