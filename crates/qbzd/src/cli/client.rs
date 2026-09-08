// crates/qbzd/src/cli/client.rs — the stateless HTTP client behind every
// networked CLI verb (02-cli-and-api.md §1.1). One verb = one request; the
// client holds no daemon state beyond target discovery (§1.5).
//
// Target discovery precedence (§1.5): `--host` > `QBZD_HOST` > local default
// `127.0.0.1:8182`. The Bearer is sent ONLY in opt-in mode: when `QBZD_TOKEN`
// is set (remote or local) or, when targeting the LOCAL daemon, the local
// `qbzd.toml` carries `[server] token`. Default: no token anywhere.
use std::time::Duration;

use serde_json::Value;

use crate::config::QbzdConfig;
use crate::paths::ProfileRoots;

/// The frozen exit-code taxonomy (02 §1.3). `exit_code()` is the ONLY source of
/// a networked verb's process exit code; scripts encode these numbers forever.
#[derive(Debug)]
pub enum CliError {
    /// exit 3 — connect refused / timeout on the target (carries `host` for the
    /// §1.4 daemon-down copy).
    Unreachable(String),
    /// exit 4 — daemon in NeedsAuth; a Qobuz session is required.
    NeedsAuth,
    /// exit 5 — audio/device error (device unopenable, volume/seek fixed in DSD).
    Device(String),
    /// exit 6 — unknown id / index out of range.
    NotFound(String),
    /// exit 1 — breaking `api_version` skew: the verb refuses politely (§1.6).
    ApiSkew { daemon: u32, cli: u32 },
    /// exit 1 — any other runtime error (daemon said no / local failure).
    Runtime(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Runtime(_) | CliError::ApiSkew { .. } => 1,
            CliError::Unreachable(_) => 3,
            CliError::NeedsAuth => 4,
            CliError::Device(_) => 5,
            CliError::NotFound(_) => 6,
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::cli::copy;
        match self {
            CliError::Unreachable(host) => write!(f, "{}", copy::daemon_down(host)),
            CliError::NeedsAuth => write!(f, "{}", copy::daemon_up_needs_auth()),
            // `error_from_envelope` pre-builds the two DSD-specific codes into
            // the full verbatim §1.4 copy (already "error: ..." prefixed); a
            // plain device message from the server gets the generic prefix.
            CliError::Device(m) => {
                if m.starts_with("error:") {
                    write!(f, "{m}")
                } else {
                    write!(f, "error: {m}")
                }
            }
            CliError::NotFound(m) => write!(f, "error: {m}"),
            CliError::ApiSkew { daemon, cli } => write!(f, "{}", copy::api_version_skew(*daemon, *cli)),
            CliError::Runtime(m) => write!(f, "error: {m}"),
        }
    }
}
impl std::error::Error for CliError {}

/// A resolved target host + whether it is the local daemon (governs whether the
/// local `qbzd.toml` token and the linger check apply).
pub struct Target {
    pub addr: String,
    pub is_local: bool,
}

/// §1.5 target discovery: `--host` > `QBZD_HOST` > local `127.0.0.1:8182`. An
/// explicit override (flag or env) is treated as remote — only the local
/// default reads the local token / runs the linger check.
pub fn resolve_host(flag: Option<String>) -> Target {
    if let Some(h) = flag.filter(|h| !h.is_empty()) {
        return Target { addr: normalize_hostport(&h), is_local: false };
    }
    if let Ok(h) = std::env::var("QBZD_HOST") {
        if !h.is_empty() {
            return Target { addr: normalize_hostport(&h), is_local: false };
        }
    }
    Target { addr: "127.0.0.1:8182".into(), is_local: true }
}

/// Append the default port when the operator gave a bare host.
fn normalize_hostport(h: &str) -> String {
    if h.contains(':') {
        h.to_string()
    } else {
        format!("{h}:8182")
    }
}

/// §1.5 token discovery: `QBZD_TOKEN` (remote or local), else — only when
/// targeting the local daemon — the local `qbzd.toml` `[server] token`.
pub(crate) fn resolve_token(target: &Target, roots: &ProfileRoots) -> Option<String> {
    if let Ok(t) = std::env::var("QBZD_TOKEN") {
        if !t.is_empty() {
            return Some(t);
        }
    }
    if target.is_local {
        if let Ok((cfg, _)) = QbzdConfig::load(&roots.config.join("qbzd.toml")) {
            return cfg.server.token.filter(|t| !t.trim().is_empty());
        }
    }
    None
}

/// A thin skin over one HTTP request to the daemon.
pub struct ApiClient {
    base: String,
    host: String,
    is_local: bool,
    token: Option<String>,
    client: reqwest::Client,
}

impl ApiClient {
    /// Discover the target + token per §1.5 and build the client.
    pub fn new(host: Option<String>, roots: &ProfileRoots) -> Self {
        let target = resolve_host(host);
        let token = resolve_token(&target, roots);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client");
        ApiClient {
            base: format!("http://{}", target.addr),
            host: target.addr,
            is_local: target.is_local,
            token,
            client,
        }
    }

    /// The target `ip:port` (for the daemon-down copy + the status header line).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether the target is the local daemon (governs the linger check).
    pub fn is_local(&self) -> bool {
        self.is_local
    }

    pub async fn get(&self, path: &str) -> Result<Value, CliError> {
        let req = self.bearer(self.client.get(format!("{}{}", self.base, path)));
        self.send(req).await
    }

    /// P0 mutation transport — consumed by the T7 transport verbs
    /// (play/pause/toggle/stop/next/prev/seek/volume/mute).
    pub async fn post(&self, path: &str, body: Value) -> Result<Value, CliError> {
        let req = self.bearer(self.client.post(format!("{}{}", self.base, path)).json(&body));
        self.send(req).await
    }

    fn bearer(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value, CliError> {
        let resp = req.send().await.map_err(|e| self.classify_transport(e))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => Ok(v),
                // Unparseable 2xx body → the §1.6 sanctioned second request.
                Err(_) => Err(self.diagnose_skew().await),
            }
        } else {
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => Err(error_from_envelope(&v)),
                Err(_) => Err(self.diagnose_skew().await),
            }
        }
    }

    fn classify_transport(&self, e: reqwest::Error) -> CliError {
        if e.is_connect() || e.is_timeout() {
            CliError::Unreachable(self.host.clone())
        } else {
            CliError::Runtime(format!("request failed: {e}"))
        }
    }

    /// §1.6: an unreadable body / unknown envelope is the only trigger for the
    /// single sanctioned second request, `GET /api/info` — the stable identity
    /// route. `api_version` mismatch → refuse politely; otherwise a plain error.
    async fn diagnose_skew(&self) -> CliError {
        if let Some(api) = self.info_api_version().await {
            if api != crate::API_VERSION {
                return CliError::ApiSkew {
                    daemon: api,
                    cli: crate::API_VERSION,
                };
            }
        }
        CliError::Runtime("daemon returned an unreadable response".to_string())
    }

    async fn info_api_version(&self) -> Option<u32> {
        let req = self.bearer(self.client.get(format!("{}/api/info", self.base)));
        let resp = req.send().await.ok()?;
        let v: Value = resp.json().await.ok()?;
        v.get("api_version").and_then(|a| a.as_u64()).map(|a| a as u32)
    }
}

/// Map an error envelope's `code` (02 §3.1.3) to the frozen exit taxonomy. The
/// CLI keys off `code`, never raw HTTP status. `origin_forbidden`/`invalid_token`
/// and anything unrecognized → exit 1.
fn error_from_envelope(v: &Value) -> CliError {
    let err = v.get("error");
    let code = err
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let message = err
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("daemon returned an error")
        .to_string();
    match code {
        "needs_auth" => CliError::NeedsAuth,
        "not_found" => CliError::NotFound(message),
        // §1.4's verbatim DSD blocks are frozen client-side copy, not the
        // server's short envelope message — swap them in so `qbzd seek`/
        // `volume`/`mute` print the exact documented multi-line text.
        "volume_fixed_dsd" => CliError::Device(crate::cli::copy::volume_fixed_dsd()),
        "seek_unsupported_dsd" => CliError::Device(crate::cli::copy::seek_unsupported_dsd()),
        "audio_unavailable" | "device_error" => {
            CliError::Device(message)
        }
        _ => CliError::Runtime(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_the_frozen_table() {
        // 02-cli-and-api.md §1.3.
        assert_eq!(CliError::Unreachable("x".into()).exit_code(), 3);
        assert_eq!(CliError::NeedsAuth.exit_code(), 4);
        assert_eq!(CliError::Device("x".into()).exit_code(), 5);
        assert_eq!(CliError::NotFound("x".into()).exit_code(), 6);
        assert_eq!(CliError::ApiSkew { daemon: 2, cli: 1 }.exit_code(), 1);
        assert_eq!(CliError::Runtime("x".into()).exit_code(), 1);
    }

    #[test]
    fn error_envelope_maps_code_to_exit() {
        let needs = serde_json::json!({"error": {"code": "needs_auth", "message": "no"}});
        assert_eq!(error_from_envelope(&needs).exit_code(), 4);
        let nf = serde_json::json!({"error": {"code": "not_found", "message": "no"}});
        assert_eq!(error_from_envelope(&nf).exit_code(), 6);
        let dev = serde_json::json!({"error": {"code": "volume_fixed_dsd", "message": "no"}});
        assert_eq!(error_from_envelope(&dev).exit_code(), 5);
        // origin_forbidden / invalid_token / unknown all fall to 1.
        let origin = serde_json::json!({"error": {"code": "origin_forbidden", "message": "no"}});
        assert_eq!(error_from_envelope(&origin).exit_code(), 1);
        let tok = serde_json::json!({"error": {"code": "invalid_token", "message": "no"}});
        assert_eq!(error_from_envelope(&tok).exit_code(), 1);
    }

    #[test]
    fn host_discovery_appends_default_port_and_flags_local() {
        assert_eq!(normalize_hostport("192.168.0.40"), "192.168.0.40:8182");
        assert_eq!(normalize_hostport("192.168.0.40:9000"), "192.168.0.40:9000");
        // An explicit flag is remote; the bare default is local.
        assert!(!resolve_host(Some("192.168.0.40".into())).is_local);
        // (Env-free path.) The default target is local.
        let t = resolve_host(None);
        if std::env::var("QBZD_HOST").is_err() {
            assert!(t.is_local);
            assert_eq!(t.addr, "127.0.0.1:8182");
        }
    }
}
