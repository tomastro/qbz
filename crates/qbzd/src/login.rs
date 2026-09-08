// crates/qbzd/src/login.rs — `qbzd login` / `qbzd logout` (02-cli-and-api.md §2.2,
// memo D6). Ported from the desktop system-browser OAuth (crates/qbz/src/auth.rs)
// with three deliberate daemon changes:
//
//   1. The one-shot listener binds an EPHEMERAL port (`bind((host, 0))`), NEVER
//      the control-API port (D6 — this is what dissolves the loopback-vs-LAN 401
//      contradiction: the callback lives on its own throwaway listener).
//   2. A CSPRNG nonce is bound into the redirect PATH
//      (`redirect_url=http://<host>:<port>/<nonce>`) and validated against the
//      callback's request path; a mismatched or second callback is dropped. The
//      nonce lives in the PATH — not an OAuth `state` param — because the
//      working desktop flow sends no `state` and there is no evidence Qobuz
//      echoes one; the redirect URL itself is preserved verbatim.
//   3. The command only VALIDATES (live `login_with_token` / `login_with_oauth_code`)
//      and PERSISTS the token into the daemon root, then best-effort nudges a
//      running daemon to reload. It never activates a session in-process — the
//      daemon (a separate process) owns session activation.
//
// There is NO email+password surface anywhere (D6/D12): the only ways in are the
// browser flow, a pasted redirect URL, and a directly-injected token.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use qbz_app::shell::AppRuntime;
use qbz_audio::settings::AudioSettings;
use qbz_core::NoOpAdapter;
use qbz_models::UserSession;

use crate::paths::ProfileRoots;

/// Browser-login deadline (02 §2.2). The desktop uses 180 s; the daemon spec
/// pins 300 s because a headless operator may need to forward the port first.
const LOGIN_DEADLINE: Duration = Duration::from_secs(300);

/// Cosmetic redirect port for the `--paste` flow. Nothing binds it — the browser
/// lands on a connection error and the operator copies the URL out of the address
/// bar — so the value only needs to be a syntactically valid, unprivileged port.
const PASTE_REDIRECT_PORT: u16 = 43717;

/// Everything that can go wrong on the way to a persisted session. Every variant
/// renders with a `→` fix line (02 §1.4). All three map to exit 1 in `main`
/// (login never reports 3/4 — it does its own OAuth and local persist, so it
/// works daemon-up or daemon-down, §2.2).
#[derive(Debug)]
pub enum LoginError {
    /// No nonce-valid redirect arrived within [`LOGIN_DEADLINE`]. Carries the
    /// ephemeral port so the timeout copy can forward exactly it.
    Timeout(u16),
    /// Qobuz explicitly rejected the credentials (401 / ineligible account).
    Rejected(String),
    /// Any other local or network-class failure.
    Failed(String),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::Timeout(port) => write!(f, "{}", crate::cli::copy::login_timeout(*port)),
            LoginError::Rejected(msg) => write!(
                f,
                "error: Qobuz rejected the credentials ({msg})\n  \
                 → check the token or sign in again:  qbzd login"
            ),
            LoginError::Failed(msg) => {
                write!(f, "error: {msg}")?;
                if !msg.contains('→') {
                    write!(
                        f,
                        "\n  → check your connection and retry:  qbzd login\n  \
                         → or inject a token directly:      qbzd login --token <user_auth_token>"
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LoginError {}

// ============================ public entry points ============================

/// Live-validate a raw `user_auth_token` via `login_with_token` BEFORE it is
/// ever persisted. The returned session is the source of truth for the user id
/// and plan. Registers the token as a redaction secret first, so no log line
/// that might carry it (in the client or elsewhere) can leak it.
///
/// T12 (settings import) and T13 (setup TUI Account screen) reuse this.
pub async fn validate_token(token: &str) -> Result<UserSession, LoginError> {
    // §6.3: register before any log line can carry the token.
    qbz_log::register_secret(token.to_string());
    let runtime = build_login_runtime().await?;
    runtime
        .core()
        .login_with_token(token)
        .await
        .map_err(map_core_err)
}

/// Path 1 (02 §2.2): system-browser OAuth on a one-shot, nonce-bound, ephemeral
/// listener. FB1 (owner feedback, post-smoke): the common real-world case is
/// configuring the daemon headless over SSH from another machine on the LAN,
/// so the callback host defaults to that LAN-reachable address — not
/// loopback-only — via [`resolve_callback_host`]. `callback_host = Some(ip)`
/// keeps its old explicit-override meaning; `None` now auto-detects from
/// `SSH_CONNECTION` before falling back to `127.0.0.1`.
pub async fn login_browser(
    roots: &ProfileRoots,
    callback_host: Option<String>,
) -> Result<UserSession, LoginError> {
    let runtime = build_login_runtime().await?;
    let app_id = read_app_id(&runtime).await?;

    // D6: an EPHEMERAL port on its own listener — never the control-API port.
    let ssh_connection = std::env::var("SSH_CONNECTION").ok();
    let (redirect_host, auto_detected) =
        resolve_callback_host(callback_host.as_deref(), ssh_connection.as_deref());

    let listener = bind_login_listener(&redirect_host)?;
    let port = listener
        .local_addr()
        .map_err(|e| LoginError::Failed(e.to_string()))?
        .port();

    let nonce = gen_nonce();
    let url = build_oauth_url(&app_id, &redirect_host, port, &nonce);

    // URL first, ALWAYS — the auto-detect note follows it.
    println!("Opening your browser to sign in to Qobuz.");
    println!("If it does not open, paste this URL into a browser:\n  {url}\n");
    if auto_detected {
        println!("{}", crate::cli::copy::login_ssh_detected());
    }
    if let Err(e) = open::that(&url) {
        // Headless boxes have no browser — not fatal; the listener still waits
        // and the printed URL (already shown above) is what the operator
        // forwards/opens from another device. Never an error-looking line.
        log::debug!("could not open a browser automatically: {e}");
        println!("{}", crate::cli::copy::login_browser_open_failed());
    }

    let nonce_owned = nonce.clone();
    let deadline = Instant::now() + LOGIN_DEADLINE;
    let captured = tokio::task::spawn_blocking(move || {
        capture_callback(listener, &nonce_owned, deadline)
    })
    .await
    .map_err(|e| LoginError::Failed(format!("login listener task panicked: {e}")))?
    .map_err(|e| LoginError::Failed(format!("login listener I/O error: {e}")))?;

    let code = captured.ok_or(LoginError::Timeout(port))?;
    let session = exchange_code(&runtime, &code).await?;
    finalize(roots, &session)?;
    Ok(session)
}

/// Path 2 (02 §2.2): print the authorize URL, read the redirect URL (or a bare
/// code) back from stdin. No listener binds — useful when the browser cannot
/// reach this machine at all. A pasted redirect URL carries the nonce in its
/// path, so it is validated (leniently); a bare code is accepted as-is
/// (explicit operator action).
pub async fn login_paste(roots: &ProfileRoots) -> Result<UserSession, LoginError> {
    let runtime = build_login_runtime().await?;
    let app_id = read_app_id(&runtime).await?;
    let nonce = gen_nonce();
    let url = build_oauth_url(&app_id, "127.0.0.1", PASTE_REDIRECT_PORT, &nonce);

    println!("Open this URL in a browser and sign in to Qobuz:\n  {url}\n");
    println!("Your browser will land on a page that fails to load — that is expected.");
    print!("Paste the full redirect URL (or just the code) here: ");
    let _ = std::io::stdout().flush();

    let line = read_stdin_line()?;
    let code = code_from_paste(line.trim(), &nonce).ok_or_else(|| {
        LoginError::Failed(
            "could not find an authorization code in the pasted input\n  \
             → paste the full redirect URL from the browser address bar\n  \
             → or inject a token directly:      qbzd login --token <user_auth_token>"
                .to_string(),
        )
    })?;

    let session = exchange_code(&runtime, &code).await?;
    finalize(roots, &session)?;
    Ok(session)
}

/// Path 3 (02 §2.2): a directly-injected `user_auth_token`. Validated live, then
/// persisted.
pub async fn login_with_token_arg(
    roots: &ProfileRoots,
    token: &str,
) -> Result<UserSession, LoginError> {
    let session = validate_token(token).await?;
    finalize(roots, &session)?;
    Ok(session)
}

/// `qbzd logout` (02 §2.2): clear the daemon-root credential file and nudge a
/// running daemon into NeedsAuth. Returns whether the daemon acknowledged the
/// reload, so the caller can pick the right success line.
pub fn logout(roots: &ProfileRoots) -> Result<bool, LoginError> {
    qbz_credentials::clear_oauth_token_at(&roots.config)
        .map_err(|e| LoginError::Failed(format!("could not clear the credential file: {e}")))?;
    let host = nudge_host(roots);
    // token: opt-in [server] token, wired by T6.
    Ok(nudge_reload(&host, None))
}

/// Three-state outcome of the ping-then-reload nudge. 04-settings-portability.md
/// §5.3 step 7 needs "daemon simply not running" (not an error) distinguished
/// from "daemon up but the reload was refused/500" (exit 1 with the restart
/// hint) — a single bool conflates them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeOutcome {
    /// Ping answered and the reload returned 2xx.
    Reloaded,
    /// Ping did not answer — no daemon to nudge (never an error).
    DaemonDown,
    /// Ping answered but the reload did not return 2xx.
    ReloadRefused,
}

/// Best-effort `GET /api/ping` → `POST /api/settings/reload` against a local
/// daemon. `token` carries the opt-in `[server] token` as
/// `Authorization: Bearer` when present; T5 callers pass `None`.
pub fn nudge_reload_outcome(host: &str, token: Option<&str>) -> NudgeOutcome {
    if !http_request_2xx(host, "GET", "/api/ping", token) {
        return NudgeOutcome::DaemonDown;
    }
    if http_request_2xx(host, "POST", "/api/settings/reload", token) {
        NudgeOutcome::Reloaded
    } else {
        NudgeOutcome::ReloadRefused
    }
}

/// Boolean skin over [`nudge_reload_outcome`] for the callers that only need
/// "did a running daemon acknowledge?" (login/logout/`settings set` — they are
/// specified to work daemon-down, 02 §2.2, so any non-reload is just "the
/// daemon picks it up on next start").
pub fn nudge_reload(host: &str, token: Option<&str>) -> bool {
    nudge_reload_outcome(host, token) == NudgeOutcome::Reloaded
}

// ============================ pure, unit-tested ============================

/// FB1 (owner feedback, post-smoke): resolve which host the OAuth redirect
/// targets. The common real-world case is configuring the daemon headless
/// over SSH from another machine on the LAN — the login URL must be openable
/// from ANY browser on the network by default, no flags.
///
/// Priority:
///   1. `cli_flag` (`--callback-host`) — explicit, unchanged, always wins.
///   2. `ssh_connection` (`$SSH_CONNECTION`)'s 3rd whitespace-separated field
///      — the SERVER ip, i.e. exactly the address the operator's other
///      machine used to reach this box. Malformed/short/non-IP values fall
///      through.
///   3. `127.0.0.1` — today's local-laptop behavior, unchanged.
///
/// Returns `(host, auto_detected)`; `auto_detected` is true only for case 2,
/// so callers can print the one extra explanatory line (§1.4 voice).
pub fn resolve_callback_host(
    cli_flag: Option<&str>,
    ssh_connection: Option<&str>,
) -> (String, bool) {
    if let Some(h) = cli_flag {
        return (h.to_string(), false);
    }
    if let Some(server_ip) = ssh_connection
        .and_then(|c| c.split_whitespace().nth(2))
        .filter(|candidate| candidate.parse::<std::net::IpAddr>().is_ok())
    {
        return (server_ip.to_string(), true);
    }
    ("127.0.0.1".to_string(), false)
}

/// Build the Qobuz browser authorize URL. Mirrors the desktop shape
/// (`crates/qbz/src/auth.rs:76-80`) except the redirect URL carries the CSPRNG
/// nonce as its path segment: `redirect_url=http://<host>:<port>/<nonce>`. The
/// binding rides the redirect URL itself (preserved verbatim by the OAuth
/// round-trip) instead of a `state` param, because the working desktop flow
/// sends no `state` and Qobuz is not proven to echo one.
pub fn build_oauth_url(ext_app_id: &str, host: &str, port: u16, nonce: &str) -> String {
    let redirect = format!("http://{}:{port}/{nonce}", bracket_ipv6_for_url(host));
    format!(
        "https://www.qobuz.com/signin/oauth?ext_app_id={}&redirect_url={}",
        ext_app_id,
        urlencoding::encode(&redirect),
    )
}

/// Bracket a bare IPv6 literal for use in a URL authority
/// (`2001:db8::2` → `[2001:db8::2]`); IPv4, hostnames, and already-bracketed
/// input pass through unchanged. FB1: `SSH_CONNECTION`'s server-IP field can
/// be IPv6.
fn bracket_ipv6_for_url(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

/// Parse an HTTP request line from the one-shot listener. Returns the
/// authorization code ONLY when the request PATH carries the expected nonce
/// (`GET /<nonce>?...`) — a mismatched or absent path nonce is dropped (the D6
/// binding). No dependency on any `state` query param (Qobuz is not proven to
/// echo one; one present is simply ignored). `code_autorisation` wins over
/// `code`, matching the desktop.
pub fn parse_callback(request_line: &str, expected_nonce: &str) -> Option<String> {
    let target = request_line.split_whitespace().nth(1)?;
    let (path, query) = target.split_once('?')?;
    if path.trim_matches('/') != expected_nonce {
        return None; // wrong or absent path nonce → drop
    }
    code_from_query(query)
}

/// Parse pasted `--paste` input: either a full redirect URL or a bare
/// authorization code. A pasted URL carries the nonce in its PATH (that is how
/// the redirect was built); validation is lenient — an empty path is tolerated
/// (hand-pasted, possibly truncated), a present-but-wrong path nonce is
/// rejected.
pub fn code_from_paste(input: &str, expected_nonce: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    match input.split_once('?') {
        Some((prefix, query)) => {
            let seg = url_path(prefix).trim_matches('/');
            if !seg.is_empty() && seg != expected_nonce {
                return None; // present but mismatched → drop
            }
            code_from_query(query)
        }
        // No query string: a bare code is fine; a bare URL has nothing to extract.
        None if input.contains("://") => None,
        None => Some(input.to_string()),
    }
}

/// The path component of a URL prefix (everything before `?`): strips a
/// `scheme://authority` head when present; a bare path passes through.
fn url_path(prefix: &str) -> &str {
    match prefix.find("://") {
        Some(i) => {
            let rest = &prefix[i + 3..];
            match rest.find('/') {
                Some(j) => &rest[j..],
                None => "",
            }
        }
        None => prefix,
    }
}

/// Extract the authorization code from a `&`-joined query string.
/// `code_autorisation` wins over `code` (desktop parity).
fn code_from_query(query: &str) -> Option<String> {
    let mut code_aut: Option<String> = None;
    let mut code_plain: Option<String> = None;
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        match k {
            "code_autorisation" => code_aut = decode(v),
            "code" => code_plain = decode(v),
            _ => {}
        }
    }
    code_aut.or(code_plain)
}

fn decode(v: &str) -> Option<String> {
    urlencoding::decode(v).ok().map(|s| s.into_owned())
}

/// A 48-hex-char (24-byte) CSPRNG nonce, bound into the redirect-URL path.
fn gen_nonce() -> String {
    use rand::RngExt;
    let mut bytes = [0u8; 24];
    rand::rng().fill(&mut bytes);
    let mut s = String::with_capacity(48);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ============================ IO helpers ============================

/// Compose the minimal client-only runtime: a headless [`NoOpAdapter`] and
/// default audio settings (no store is opened — login never touches audio),
/// then `init()` to extract the Qobuz bundle tokens the sign-in calls need.
async fn build_login_runtime() -> Result<AppRuntime<NoOpAdapter>, LoginError> {
    let runtime =
        AppRuntime::with_audio_settings(NoOpAdapter, None, AudioSettings::default(), None);
    if let Err(e) = runtime.init().await {
        return Err(LoginError::Failed(format!(
            "could not reach Qobuz to start login: {e}\n  → check your connection and retry"
        )));
    }
    Ok(runtime)
}

async fn read_app_id(runtime: &AppRuntime<NoOpAdapter>) -> Result<String, LoginError> {
    let client_lock = runtime.core().client();
    let guard = client_lock.read().await;
    let client = guard.as_ref().ok_or_else(|| {
        LoginError::Failed(
            "Qobuz client not initialized — could not reach Qobuz\n  \
             → check your connection and retry"
                .to_string(),
        )
    })?;
    client
        .app_id()
        .await
        .map_err(|e| LoginError::Failed(format!("could not read the Qobuz app id: {e}")))
}

async fn exchange_code(
    runtime: &AppRuntime<NoOpAdapter>,
    code: &str,
) -> Result<UserSession, LoginError> {
    let client_lock = runtime.core().client();
    let guard = client_lock.read().await;
    let client = guard
        .as_ref()
        .ok_or_else(|| LoginError::Failed("Qobuz client not initialized".to_string()))?;
    client.login_with_oauth_code(code).await.map_err(map_api_err)
}

/// Register the secret, persist the token into the daemon config root (0600),
/// then best-effort nudge a running daemon. Persist happens ONLY here — after
/// the caller already live-validated the session.
fn finalize(roots: &ProfileRoots, session: &UserSession) -> Result<(), LoginError> {
    // §6.3: register before the token can reach any log line (idempotent — the
    // token path already registered it in `validate_token`).
    qbz_log::register_secret(session.user_auth_token.clone());
    qbz_credentials::save_oauth_token_at(&roots.config, &session.user_auth_token).map_err(|e| {
        LoginError::Failed(format!(
            "could not save credentials to {}: {e}",
            roots.config.display()
        ))
    })?;
    let host = nudge_host(roots);
    // token: opt-in [server] token, wired by T6.
    let _ = nudge_reload(&host, None);
    Ok(())
}

/// The local daemon's reload address. Credentials are written to the LOCAL
/// config root, so the daemon to nudge is always local; its port comes from the
/// same `qbzd.toml` the daemon reads (default 8182).
pub(crate) fn nudge_host(roots: &ProfileRoots) -> String {
    let port = crate::config::QbzdConfig::load(&roots.config.join("qbzd.toml"))
        .map(|(c, _)| c.server.port)
        .unwrap_or(8182);
    format!("127.0.0.1:{port}")
}

/// FB6: bind the one-shot login listener WIDE by default, on an EPHEMERAL
/// port — the LAN-first posture applies to the login listener too, so the
/// OAuth redirect lands regardless of which address on the box is actually
/// reachable. This is independent of `redirect_host` (the resolved
/// `--callback-host` > `SSH_CONNECTION` > `127.0.0.1` value embedded in the
/// URL, unchanged): the listener no longer binds that specific address.
///
///   - Family-aware only: an explicit IPv6 `redirect_host` binds the IPv6
///     wildcard `::` (so a bracketed IPv6 redirect URL still lands — a v4
///     wildcard under a v6 URL would guarantee a 300 s timeout).
///   - Everything else — loopback, a v4 LAN IP, or non-IP input (a hostname
///     in `--callback-host`, never handed to `TcpListener::bind` to avoid a
///     blocking DNS lookup) — binds the IPv4 wildcard `0.0.0.0`.
fn bind_login_listener(redirect_host: &str) -> Result<TcpListener, LoginError> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let fail = |e: std::io::Error| {
        LoginError::Failed(format!("could not bind the login listener: {e}"))
    };
    let wildcard: IpAddr = match redirect_host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => Ipv6Addr::UNSPECIFIED.into(),
        _ => Ipv4Addr::UNSPECIFIED.into(),
    };
    TcpListener::bind((wildcard, 0)).map_err(fail)
}

fn read_stdin_line() -> Result<String, LoginError> {
    use std::io::BufRead;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| LoginError::Failed(format!("could not read from stdin: {e}")))?;
    Ok(line)
}

const SUCCESS_HTML: &str = "<html><body style=\"font-family:system-ui;text-align:center;padding:64px;background:#0f0f0f;color:#fff\">\
<h2>Login successful</h2><p>You can close this tab and return to your terminal.</p></body></html>";
const WAITING_HTML: &str = "<html><body style=\"font-family:system-ui;text-align:center;padding:64px;background:#0f0f0f;color:#fff\">\
<h2>Waiting for Qobuz…</h2></body></html>";

/// Accept connections until one carries a nonce-valid authorization code, then
/// return it and stop (exactly one accepted). Browser noise and nonce-mismatched
/// requests are answered with a neutral page and skipped. Non-blocking with a
/// 100 ms poll so the deadline is honored without a background thread leak — this
/// runs inside `spawn_blocking` and self-terminates at `deadline`.
fn capture_callback(
    listener: TcpListener,
    expected_nonce: &str,
    deadline: Instant,
) -> std::io::Result<Option<String>> {
    listener.set_nonblocking(true)?;
    loop {
        if Instant::now() >= deadline {
            return Ok(None);
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).ok();
                stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let request_line = request.lines().next().unwrap_or("");
                let code = parse_callback(request_line, expected_nonce);

                let body = if code.is_some() { SUCCESS_HTML } else { WAITING_HTML };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();

                if code.is_some() {
                    return Ok(code);
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    }
}

fn http_request_2xx(host: &str, method: &str, path: &str, token: Option<&str>) -> bool {
    let addr = match host.to_socket_addrs().ok().and_then(|mut a| a.next()) {
        Some(a) => a,
        None => return false,
    };
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(600)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let _ = stream.flush();
    let mut buf = [0u8; 128];
    let n = stream.read(&mut buf).unwrap_or(0);
    let status = String::from_utf8_lossy(&buf[..n]);
    matches!(
        status.lines().next().and_then(|l| l.split_whitespace().nth(1)),
        Some(code) if code.starts_with('2')
    )
}

// ============================ error mapping ============================

fn map_api_err(e: qbz_qobuz::ApiError) -> LoginError {
    match e {
        qbz_qobuz::ApiError::AuthenticationError(_) | qbz_qobuz::ApiError::IneligibleUser => {
            LoginError::Rejected(e.to_string())
        }
        other => LoginError::Failed(other.to_string()),
    }
}

fn map_core_err(e: qbz_core::CoreError) -> LoginError {
    if matches!(
        e,
        qbz_core::CoreError::Api(
            qbz_qobuz::ApiError::AuthenticationError(_) | qbz_qobuz::ApiError::IneligibleUser
        )
    ) {
        LoginError::Rejected(e.to_string())
    } else {
        LoginError::Failed(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_oauth_url_embeds_nonce_in_the_redirect_path() {
        // Step 1(a), amended: the URL embeds
        // redirect_url=http://<host>:<port>/<nonce> — the nonce rides the PATH,
        // never a state param the provider would have to echo.
        let url = build_oauth_url("app123", "127.0.0.1", 39114, "NONCEabc");
        assert!(url.starts_with("https://www.qobuz.com/signin/oauth?"), "{url}");
        assert!(url.contains("ext_app_id=app123"), "{url}");
        let decoded = urlencoding::decode(&url).unwrap();
        assert!(
            decoded.contains("redirect_url=http://127.0.0.1:39114/NONCEabc"),
            "{decoded}"
        );
    }

    #[test]
    fn callback_host_is_embedded_in_the_redirect() {
        // Step 1(c): --callback-host embeds that host in the redirect URL.
        let url = build_oauth_url("app123", "192.168.0.40", 40000, "n");
        let decoded = urlencoding::decode(&url).unwrap();
        assert!(
            decoded.contains("redirect_url=http://192.168.0.40:40000/n"),
            "{decoded}"
        );
    }

    #[test]
    fn build_oauth_url_brackets_an_ipv6_host_and_keeps_the_nonce_in_the_path() {
        // FB1: a non-loopback IPv6 host (e.g. from SSH_CONNECTION) must be
        // bracketed in the URL authority; the nonce still rides the path.
        let url = build_oauth_url("app123", "2001:db8::2", 40000, "nn");
        let decoded = urlencoding::decode(&url).unwrap();
        assert!(
            decoded.contains("redirect_url=http://[2001:db8::2]:40000/nn"),
            "{decoded}"
        );
    }

    // ---------------------- resolve_callback_host (FB1) ----------------------

    #[test]
    fn resolve_callback_host_cli_flag_wins_over_everything() {
        let (host, auto) =
            resolve_callback_host(Some("10.0.0.9"), Some("192.168.1.5 22 192.168.1.1 22"));
        assert_eq!(host, "10.0.0.9");
        assert!(!auto, "an explicit --callback-host is never auto-detected");
    }

    #[test]
    fn resolve_callback_host_reads_the_server_ip_field_from_ssh_connection() {
        // SSH_CONNECTION = "client_ip client_port server_ip server_port" —
        // the SERVER ip (3rd field) is exactly what the operator's other
        // machine used to reach this box.
        let (host, auto) = resolve_callback_host(None, Some("203.0.113.4 51820 192.168.1.50 22"));
        assert_eq!(host, "192.168.1.50");
        assert!(auto);
    }

    #[test]
    fn resolve_callback_host_handles_ipv6_ssh_connection() {
        let (host, auto) =
            resolve_callback_host(None, Some("2001:db8::1 51820 2001:db8::2 22"));
        assert_eq!(host, "2001:db8::2");
        assert!(auto);
    }

    #[test]
    fn resolve_callback_host_falls_through_on_malformed_ssh_connection() {
        // Too few fields — no server-ip field to read.
        let (host, auto) = resolve_callback_host(None, Some("only-two fields"));
        assert_eq!(host, "127.0.0.1");
        assert!(!auto);
    }

    #[test]
    fn resolve_callback_host_falls_through_on_non_ip_server_field() {
        // 3rd field present but not a parseable IP — reject, don't guess.
        let (host, auto) =
            resolve_callback_host(None, Some("203.0.113.4 51820 not-an-ip 22"));
        assert_eq!(host, "127.0.0.1");
        assert!(!auto);
    }

    #[test]
    fn resolve_callback_host_falls_through_on_empty_ssh_connection() {
        // SSH_CONNECTION set but empty — no fields at all.
        let (host, auto) = resolve_callback_host(None, Some(""));
        assert_eq!(host, "127.0.0.1");
        assert!(!auto);
    }

    #[test]
    fn resolve_callback_host_falls_through_when_ssh_connection_absent() {
        // The local-laptop case: no flag, no SSH session — unchanged default.
        let (host, auto) = resolve_callback_host(None, None);
        assert_eq!(host, "127.0.0.1");
        assert!(!auto);
    }

    // ---------------------- bind_login_listener (FB6) ----------------------

    #[test]
    fn bind_login_listener_loopback_v4_host_binds_the_v4_wildcard() {
        // FB6: LAN-first posture — even a loopback-resolved redirect host
        // binds the wide listener now; only the embedded URL stays loopback.
        let l = bind_login_listener("127.0.0.1").unwrap();
        let addr = l.local_addr().unwrap();
        assert!(addr.ip().is_unspecified(), "{addr}");
        assert!(addr.is_ipv4(), "{addr}");
    }

    #[test]
    fn bind_login_listener_v4_lan_host_binds_the_v4_wildcard() {
        // An explicit/SSH-detected LAN v4 host also binds wide, not that
        // specific address.
        let l = bind_login_listener("192.168.1.50").unwrap();
        let addr = l.local_addr().unwrap();
        assert!(addr.ip().is_unspecified(), "{addr}");
        assert!(addr.is_ipv4(), "{addr}");
    }

    #[test]
    fn bind_login_listener_v6_host_binds_the_v6_wildcard() {
        // Family-aware: an explicit IPv6 host binds the IPv6 wildcard `::`.
        match bind_login_listener("::1") {
            Ok(l) => {
                let addr = l.local_addr().unwrap();
                assert!(addr.ip().is_unspecified(), "{addr}");
                assert!(addr.is_ipv6(), "{addr}");
            }
            // A box with IPv6 disabled can't bind `::` at all.
            Err(e) => assert!(e.to_string().contains("could not bind"), "{e}"),
        }
    }

    #[test]
    fn bind_login_listener_never_does_dns_for_non_ip_hosts() {
        // A hostname must NOT reach TcpListener::bind (blocking DNS); it goes
        // straight to the 0.0.0.0 wildcard. An unresolvable name succeeding
        // proves no lookup happened.
        let l = bind_login_listener("definitely-not-resolvable.invalid").unwrap();
        let addr = l.local_addr().unwrap();
        assert!(addr.ip().is_unspecified(), "{addr}");
        assert!(addr.is_ipv4(), "{addr}");
    }

    #[test]
    fn parse_callback_accepts_matching_path_nonce_and_extracts_code() {
        let line = "GET /abc123?code_autorisation=THECODE HTTP/1.1";
        assert_eq!(parse_callback(line, "abc123"), Some("THECODE".to_string()));
    }

    #[test]
    fn parse_callback_falls_back_to_plain_code() {
        let line = "GET /n?code=plaincode HTTP/1.1";
        assert_eq!(parse_callback(line, "n"), Some("plaincode".to_string()));
    }

    #[test]
    fn parse_callback_prefers_code_autorisation_over_code() {
        let line = "GET /n?code=plain&code_autorisation=preferred HTTP/1.1";
        assert_eq!(parse_callback(line, "n"), Some("preferred".to_string()));
    }

    #[test]
    fn parse_callback_rejects_mismatched_path_nonce() {
        // Step 1(b): a wrong path nonce is dropped even with a valid-looking code.
        let line = "GET /WRONG?code_autorisation=THECODE HTTP/1.1";
        assert_eq!(parse_callback(line, "abc123"), None);
    }

    #[test]
    fn parse_callback_rejects_absent_path_nonce() {
        let line = "GET /?code_autorisation=THECODE HTTP/1.1";
        assert_eq!(parse_callback(line, "abc123"), None);
    }

    #[test]
    fn parse_callback_needs_no_state_param_and_ignores_one() {
        // The provider echoing state is exactly what we no longer depend on.
        let no_state = "GET /abc123?code=OK HTTP/1.1";
        assert_eq!(parse_callback(no_state, "abc123"), Some("OK".to_string()));
        let stray_state = "GET /abc123?state=whatever&code=OK HTTP/1.1";
        assert_eq!(parse_callback(stray_state, "abc123"), Some("OK".to_string()));
    }

    #[test]
    fn parse_callback_ignores_browser_noise() {
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1", "abc123"), None);
        assert_eq!(parse_callback("", "abc123"), None);
    }

    #[test]
    fn parse_callback_percent_decodes_the_code() {
        let line = "GET /n?code=x%2Fy HTTP/1.1";
        assert_eq!(parse_callback(line, "n"), Some("x/y".to_string()));
    }

    #[test]
    fn code_from_paste_accepts_full_redirect_url_with_path_nonce() {
        let pasted = "http://127.0.0.1:43717/nn?code_autorisation=PASTED";
        assert_eq!(code_from_paste(pasted, "nn"), Some("PASTED".to_string()));
    }

    #[test]
    fn code_from_paste_tolerates_a_missing_path_nonce() {
        // Lenient by design: the operator pasted the URL by hand.
        let pasted = "http://127.0.0.1:43717/?code_autorisation=PASTED";
        assert_eq!(code_from_paste(pasted, "nn"), Some("PASTED".to_string()));
    }

    #[test]
    fn code_from_paste_accepts_a_bare_code() {
        assert_eq!(code_from_paste("JUSTACODE", "nn"), Some("JUSTACODE".to_string()));
        assert_eq!(code_from_paste("  JUSTACODE  ", "nn"), Some("JUSTACODE".to_string()));
    }

    #[test]
    fn code_from_paste_rejects_mismatched_path_nonce_in_url() {
        let pasted = "http://127.0.0.1:43717/WRONG?code_autorisation=PASTED";
        assert_eq!(code_from_paste(pasted, "nn"), None);
    }

    #[test]
    fn code_from_paste_rejects_empty_input() {
        assert_eq!(code_from_paste("   ", "nn"), None);
    }

    #[test]
    fn gen_nonce_is_long_unique_and_hex() {
        let a = gen_nonce();
        let b = gen_nonce();
        assert_ne!(a, b, "two nonces collided");
        assert_eq!(a.len(), 48, "nonce length: {}", a.len());
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "non-hex: {a}");
    }

    #[test]
    fn nudge_reload_is_false_when_daemon_is_down() {
        // Bind then immediately drop to obtain a definitely-closed local port.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert!(!nudge_reload(&format!("127.0.0.1:{port}"), None));
    }

    #[test]
    fn nudge_outcome_is_daemon_down_when_nothing_listens() {
        // The three-state outcome: no listener → DaemonDown (never ReloadRefused).
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert_eq!(
            nudge_reload_outcome(&format!("127.0.0.1:{port}"), None),
            NudgeOutcome::DaemonDown
        );
    }

    #[test]
    fn login_timeout_error_renders_the_verbatim_copy_with_the_port() {
        let rendered = LoginError::Timeout(39114).to_string();
        assert!(rendered.contains("no OAuth redirect received within 300 s"), "{rendered}");
        assert!(rendered.contains("ssh -L 39114:localhost:39114"), "{rendered}");
        assert!(rendered.contains("qbzd login --paste"), "{rendered}");
        assert!(rendered.contains("qbzd login --token <user_auth_token>"), "{rendered}");
    }
}
