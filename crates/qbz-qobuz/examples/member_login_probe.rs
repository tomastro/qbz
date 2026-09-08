//! Read-only live probe: what does Qobuz return for an account WITHOUT an
//! active subscription ("Qobuz member")?
//!
//! Answers Q2 of `qbz-nix-docs/offline-mode/tauri-review-2026-06-09/
//! 10-subscription-trial-offline-gating.md` §5.6 against the LIVE API and
//! prints REDACTED JSON so the shapes can be captured into
//! `qbz-nix-docs/qobuz-api/` before the member-mode parser is written:
//!
//!   1. `POST /user/login` raw payload after the OAuth exchange (does it
//!      200? what is in `credential.parameters` / `user.subscription`?)
//!   2. `favorite/getUserFavorites` totals + `getUserPlaylists` (reads?)
//!   3. `/track/getFileUrl` for a catalog track at CD + Hi-Res quality
//!      (`sample`? `restrictions`? which format comes back?)
//!   4. optionally the same for a PURCHASED track (`--purchased=<track_id>`)
//!
//! Sign-in is the same system-browser OAuth the app uses (Qobuz retired the
//! email+password login in 2026): the probe binds a loopback listener,
//! prints the sign-in URL (and tries to open it), captures the redirect,
//! exchanges the code. Alternatively paste an existing `user_auth_token`
//! (e.g. the web player's `X-User-Auth-Token` header) to skip the browser:
//!
//!   cargo run -p qbz-qobuz --example member_login_probe [--purchased=ID] [track_id...]
//!   QBZ_PROBE_TOKEN=... cargo run -p qbz-qobuz --example member_login_probe
//!
//! Nothing is written to disk and nothing is written to Qobuz: login + GETs.

use qbz_models::{Quality, UserSession};
use qbz_qobuz::QobuzClient;
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

const OAUTH_TIMEOUT: Duration = Duration::from_secs(180);

/// Devin Townsend - Vampira (Retinal Circus): a plain catalog track that the
/// lyrics probe already used, so the two captures line up.
const DEFAULT_TRACK_IDS: &[u64] = &[29006863];

/// Keys whose string values identify the account or are secrets. Redacted
/// in place before printing; structure and every other field stay verbatim.
const REDACT_KEYS: &[&str] = &[
    "user_auth_token",
    "email",
    "login",
    "firstname",
    "lastname",
    "display_name",
    "avatar",
    "publicId",
    "public_id",
    "session_id",
];

fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if REDACT_KEYS.contains(&key.as_str()) {
                    if let Value::String(s) = child {
                        if !s.is_empty() {
                            *s = format!("<redacted:{}>", s.len());
                        }
                    }
                } else if key == "url" {
                    // Signed CDN URL: keep scheme + host + path, drop the
                    // query (it carries the per-request token).
                    if let Value::String(s) = child {
                        if let Some((base, _)) = s.split_once('?') {
                            *s = format!("{base}?<redacted-query>");
                        }
                    }
                } else {
                    redact(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

fn print_json(label: &str, status: u16, body: &str) {
    println!("\n=== {label} (HTTP {status}) ===");
    match serde_json::from_str::<Value>(body) {
        Ok(mut json) => {
            redact(&mut json);
            println!(
                "{}",
                serde_json::to_string_pretty(&json).unwrap_or_default()
            );
        }
        Err(_) => println!("<non-JSON body, {} bytes>", body.len()),
    }
}

/// 48 hex chars from a splitmix64 stream seeded by wall-clock nanos + pid.
/// A probe run by hand does not need a CSPRNG; it needs a path the callback
/// must echo back so a stray request on the port is ignored.
fn gen_nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let mut state = nanos ^ ((std::process::id() as u64) << 32);
    let mut out = String::with_capacity(48);
    for _ in 0..3 {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        out.push_str(&format!("{z:016x}"));
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| percent_decode(v))
}

/// The authorization code ONLY when the request path carries the nonce
/// (`GET /<nonce>?...`); `code_autorisation` wins over `code`, as in the app.
fn parse_callback(request_line: &str, expected_nonce: &str) -> Option<String> {
    let target = request_line.split_whitespace().nth(1)?;
    let (path, query) = target.split_once('?')?;
    if path.trim_matches('/') != expected_nonce {
        return None;
    }
    query_param(query, "code_autorisation").or_else(|| query_param(query, "code"))
}

async fn capture_oauth_code(listener: TcpListener, expected_nonce: &str) -> Option<String> {
    loop {
        let (mut stream, _) = listener.accept().await.ok()?;
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).await.ok()?;
        let request = String::from_utf8_lossy(&buf[..n]);
        let code = parse_callback(request.lines().next().unwrap_or(""), expected_nonce);
        let body = if code.is_some() {
            "<html><body style=\"font-family:system-ui;text-align:center;padding:64px\">\
             <h2>Probe signed in</h2><p>You can close this tab.</p></body></html>"
        } else {
            "<html><body style=\"font-family:system-ui;text-align:center;padding:64px\">\
             <h2>Waiting for Qobuz...</h2></body></html>"
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body,
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;
        if code.is_some() {
            return code;
        }
    }
}

/// System-browser OAuth, the app's flow: loopback listener + nonce path,
/// sign-in URL, redirect capture, code → token exchange.
async fn oauth_token(client: &QobuzClient) -> Result<String, Box<dyn std::error::Error>> {
    let app_id = client.app_id().await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let nonce = gen_nonce();
    let redirect = format!("http%3A%2F%2Flocalhost%3A{port}%2F{nonce}");
    let url =
        format!("https://www.qobuz.com/signin/oauth?ext_app_id={app_id}&redirect_url={redirect}");
    eprintln!("[probe] sign in with the account to probe:\n{url}\n");
    // Best-effort: open it; if that fails the URL above is the fallback.
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    let _ = std::process::Command::new(opener)
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let code = tokio::time::timeout(OAUTH_TIMEOUT, capture_oauth_code(listener, &nonce))
        .await
        .map_err(|_| "OAuth login timed out (180 s)")?
        .ok_or("OAuth login cancelled or no code received")?;
    eprintln!("[probe] code captured, exchanging for a token");
    Ok(client.exchange_oauth_code(&code).await?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = log::set_logger(&LOGGER).map(|_| log::set_max_level(log::LevelFilter::Info));

    let mut purchased: Option<u64> = None;
    let mut ids: Vec<u64> = Vec::new();
    for arg in std::env::args().skip(1) {
        if let Some(id) = arg.strip_prefix("--purchased=") {
            purchased = Some(id.parse()?);
        } else if let Ok(id) = arg.parse() {
            ids.push(id);
        }
    }
    let track_ids: Vec<u64> = if ids.is_empty() {
        DEFAULT_TRACK_IDS.to_vec()
    } else {
        ids
    };

    let client = QobuzClient::new()?;
    let warm = client.init().await?;
    eprintln!("[probe] init done (warm bundle cache: {warm})");

    // 1. Token: pasted, or the app's OAuth flow. Then the raw session
    // fetch, captured BEFORE the parser gets a chance to reject it.
    let token = match std::env::var("QBZ_PROBE_TOKEN") {
        Ok(t) if !t.trim().is_empty() => {
            eprintln!("[probe] using QBZ_PROBE_TOKEN");
            t.trim().to_string()
        }
        _ => oauth_token(&client).await?,
    };
    let (status, body) = client.login_with_token_raw(&token).await?;
    print_json("POST /user/login (X-User-Auth-Token)", status, &body);
    if status != 200 {
        eprintln!("[probe] login did not return 200; nothing else to probe");
        return Ok(());
    }
    let json: Value = serde_json::from_str(&body)?;
    let user = json.get("user").ok_or("no `user` in login payload")?;
    let user_id = user
        .get("id")
        .and_then(Value::as_u64)
        .ok_or("no `user.id` in login payload")?;
    let session_token = json
        .get("user_auth_token")
        .and_then(Value::as_str)
        .unwrap_or(&token)
        .to_string();
    let parameters = user.get("credential").and_then(|c| c.get("parameters"));
    let has_parameters = parameters
        .and_then(Value::as_object)
        .map(|o| !o.is_empty())
        .unwrap_or(false);
    println!(
        "\n[verdict] user_id={user_id} credential.parameters present+non-empty: {has_parameters} \
         -> today's parser would say: {}",
        if has_parameters {
            "eligible"
        } else {
            "IneligibleUser"
        }
    );
    println!(
        "[verdict] user.subscription = {}",
        user.get("subscription")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "<absent>".into())
    );

    // Hand-built session so the authenticated GETs below can run even when
    // the parser would have refused. In-memory only.
    client
        .set_session(UserSession {
            user_auth_token: session_token,
            user_id,
            ..UserSession::default()
        })
        .await;

    // 2. Reads: favorites totals per type + playlists.
    for fav_type in ["albums", "tracks", "artists", "labels", "awards"] {
        match client.get_favorites(fav_type, 1, 0).await {
            Ok(v) => println!(
                "[favorites] {fav_type}: total={}",
                v.get(fav_type)
                    .and_then(|t| t.get("total"))
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "?".into())
            ),
            Err(e) => println!("[favorites] {fav_type}: ERROR {e}"),
        }
    }
    match client.get_user_playlists().await {
        Ok(p) => println!("[playlists] getUserPlaylists: {} entries", p.len()),
        Err(e) => println!("[playlists] getUserPlaylists: ERROR {e}"),
    }

    // 3. Streaming: catalog track(s) at CD and Hi-Res.
    for track_id in &track_ids {
        for quality in [Quality::Lossless, Quality::UltraHiRes] {
            match client.get_stream_url_raw(*track_id, quality).await {
                Ok((status, body)) => print_json(
                    &format!("getFileUrl track={track_id} format_id={}", quality.id()),
                    status,
                    &body,
                ),
                Err(e) => println!(
                    "\n=== getFileUrl track={track_id} format_id={} === ERROR {e}",
                    quality.id()
                ),
            }
        }
    }

    // 4. Streaming a purchased track, if the caller named one.
    if let Some(track_id) = purchased {
        for quality in [Quality::Lossless, Quality::UltraHiRes] {
            match client.get_stream_url_raw(track_id, quality).await {
                Ok((status, body)) => print_json(
                    &format!(
                        "getFileUrl PURCHASED track={track_id} format_id={}",
                        quality.id()
                    ),
                    status,
                    &body,
                ),
                Err(e) => println!(
                    "\n=== getFileUrl PURCHASED track={track_id} format_id={} === ERROR {e}",
                    quality.id()
                ),
            }
        }
    }

    eprintln!("[probe] done; nothing was written to Qobuz or to disk");
    Ok(())
}
