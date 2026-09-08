// crates/qbzd/src/cli/watch.rs — `qbzd watch`, the CLI caller for the
// `GET /api/events` SSE stream (CONSOLE ext). Opens a long-lived connection and
// prints events as they arrive; the shipped verb behind the P1 route (§3.1.4).
//
// Default output is newline-delimited JSON — one CoreEvent `data` payload per
// line, pipe-friendly (`qbzd watch | jq`). `--raw` passes the SSE frames
// through verbatim (event:/data:/comment lines). Unlike `ApiClient` this uses a
// bespoke reqwest client with NO read timeout (the stream is meant to stay
// open); only the connect attempt is bounded.
use std::io::Write;
use std::time::Duration;

use crate::cli::client::{resolve_host, resolve_token, CliError};
use crate::paths::ProfileRoots;

pub async fn watch(host: Option<String>, raw: bool, roots: &ProfileRoots) -> i32 {
    let target = resolve_host(host);
    let token = resolve_token(&target, roots);
    let base = format!("http://{}", target.addr);

    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let mut builder = client
        .get(format!("{base}/api/events"))
        .header(reqwest::header::ACCEPT, "text/event-stream");
    if let Some(t) = &token {
        builder = builder.bearer_auth(t);
    }

    let resp = match builder.send().await {
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

    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        let hint = if code == 401 || code == 403 {
            " — check QBZD_TOKEN / the daemon [server] token"
        } else {
            ""
        };
        eprintln!("error: daemon returned {code}{hint}: {}", body.trim());
        return 1;
    }

    let mut resp = resp;
    let stdout = std::io::stdout();
    let mut pending = String::new();
    loop {
        match resp.chunk().await {
            Ok(Some(bytes)) => {
                if raw {
                    let mut lock = stdout.lock();
                    let _ = lock.write_all(&bytes);
                    let _ = lock.flush();
                    continue;
                }
                pending.push_str(&String::from_utf8_lossy(&bytes));
                // Emit each COMPLETE line's `data:` payload; keep the tail.
                while let Some(nl) = pending.find('\n') {
                    let line: String = pending.drain(..=nl).collect();
                    if let Some(data) = line.trim_end().strip_prefix("data:") {
                        let mut lock = stdout.lock();
                        let _ = writeln!(lock, "{}", data.trim());
                        let _ = lock.flush();
                    }
                    // event: / comment (`:`) / blank lines are dropped in parsed mode
                }
            }
            // The daemon closed the stream (shutting down): treat as unreachable.
            Ok(None) => {
                eprintln!("{}", CliError::Unreachable(target.addr.clone()));
                return CliError::Unreachable(target.addr).exit_code();
            }
            Err(e) => {
                eprintln!("error: event stream read failed: {e}");
                return 1;
            }
        }
    }
}
