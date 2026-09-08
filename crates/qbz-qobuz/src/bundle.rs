//! Bundle token extraction from Qobuz web player
//!
//! Extracts app_id and secrets from the Qobuz JavaScript bundle.
//! This is necessary because Qobuz doesn't provide a public API.

use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::Builder;

use super::error::{ApiError, Result};

const LOGIN_PAGE_URL: &str = "https://play.qobuz.com/login";
const BUNDLE_BASE_URL: &str = "https://play.qobuz.com";

/// Per-request ceiling for the bundle fetch. The login page is tiny but the
/// bundle.js is ~7 MB and served from a CDN that is sometimes very slow; without
/// this, a stalled download blocks the entire app startup indefinitely.
const BUNDLE_FETCH_TIMEOUT: Duration = Duration::from_secs(45);
/// Extra attempts after the first on a failed/timed-out extraction.
const BUNDLE_EXTRACTION_RETRIES: usize = 2;

/// Extracted bundle tokens
#[derive(Debug, Clone)]
pub struct BundleTokens {
    pub app_id: String,
    pub secrets: Vec<String>,
    /// OAuth private key used for the /oauth/callback exchange.
    /// Present in recent bundle versions; None on older bundles.
    pub private_key: Option<String>,
}

/// On-disk cache of the extracted tokens, keyed by the Qobuz bundle version
/// (e.g. `8.1.0-b019`) so we can detect when Qobuz rotates the bundle and the
/// secrets change. Lives in the regenerable cache dir, never in precious data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedBundle {
    pub bundle_version: String,
    pub app_id: String,
    pub secrets: Vec<String>,
    #[serde(default)]
    pub private_key: Option<String>,
    /// Unix seconds when these tokens were fetched (freshness only; not a TTL).
    pub fetched_at: i64,
}

impl From<CachedBundle> for BundleTokens {
    fn from(c: CachedBundle) -> Self {
        BundleTokens {
            app_id: c.app_id,
            secrets: c.secrets,
            private_key: c.private_key,
        }
    }
}

/// Where the token cache lives.
///
/// `cache_dir` is what an embedder supplied; without one this falls back to the
/// platform cache directory, which is what the desktop has always used.
fn cache_path(cache_dir: Option<&Path>) -> Option<PathBuf> {
    let root = cache_dir.map(Path::to_path_buf).or_else(dirs::cache_dir)?;
    Some(root.join("qbz").join("bundle_tokens.json"))
}

/// Replace `path` with whatever `write` produces, or leave it exactly as it was.
///
/// The bytes land in a sibling temp file that is flushed and then renamed over
/// the target, so a reader either sees the previous cache or the new one and
/// never the gap between them. Writing in place cannot offer that: it truncates
/// the file before the first byte of the replacement is written.
///
/// This is process-crash atomicity, not a power-loss durability promise: the
/// file contents are synced before the rename, but the parent directory is not.
/// Concurrent writers are not serialized; the last successful rename wins,
/// while every visible version remains a complete file.
///
/// `write` is a closure rather than a `&[u8]` so a test can fail partway through
/// and assert what survives.
fn atomic_write_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache path has no parent"))?;
    std::fs::create_dir_all(parent)?;

    let mut temp = Builder::new()
        .prefix(".bundle_tokens.")
        .tempfile_in(parent)?;
    write(temp.as_file_mut())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_with(path, |file| file.write_all(bytes))
}

/// Load cached tokens if a valid cache file exists. Returns `None` on any error
/// (missing file, malformed JSON, empty fields) so the caller falls back to a
/// live fetch.
pub fn load_cached_bundle() -> Option<CachedBundle> {
    load_cached_bundle_from(None)
}

pub(crate) fn load_cached_bundle_from(cache_dir: Option<&Path>) -> Option<CachedBundle> {
    let path = cache_path(cache_dir)?;
    let data = std::fs::read(&path).ok()?;
    match serde_json::from_slice::<CachedBundle>(&data) {
        Ok(c) if !c.app_id.is_empty() && !c.secrets.is_empty() => Some(c),
        Ok(_) => {
            log::warn!("[Bundle] Cached tokens missing app_id/secrets, ignoring");
            None
        }
        Err(e) => {
            log::warn!("[Bundle] Failed to parse token cache: {}", e);
            None
        }
    }
}

fn save_cached_bundle(c: &CachedBundle, cache_dir: Option<&Path>) {
    let Some(path) = cache_path(cache_dir) else {
        log::warn!("[Bundle] No cache dir available, skipping token cache write");
        return;
    };
    match serde_json::to_vec_pretty(c) {
        Ok(bytes) => match atomic_write(&path, &bytes) {
            Ok(_) => log::info!("[Bundle] Cached tokens (version {})", c.bundle_version),
            Err(e) => log::warn!("[Bundle] Failed to write token cache: {}", e),
        },
        Err(e) => log::warn!("[Bundle] Failed to serialize token cache: {}", e),
    }
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Parse the bundle version out of the `/resources/<version>/bundle.js` path,
/// e.g. `/resources/8.1.0-b019/bundle.js` -> `8.1.0-b019`.
fn bundle_version_from_url(bundle_url: &str) -> String {
    bundle_url
        .trim_start_matches("/resources/")
        .trim_end_matches("/bundle.js")
        .to_string()
}

/// Fetch the login page and return the current bundle URL + parsed version.
/// Cheap (~small page); used both by the full extraction and the background
/// version check.
async fn fetch_bundle_url(client: &Client) -> Result<(String, String)> {
    let login_page = client
        .get(LOGIN_PAGE_URL)
        .timeout(BUNDLE_FETCH_TIMEOUT)
        .send()
        .await?
        .text()
        .await?;
    let bundle_url = extract_bundle_url(&login_page)?;
    let version = bundle_version_from_url(&bundle_url);
    Ok((bundle_url, version))
}

/// Single network extraction attempt: fetch login page -> bundle.js -> parse.
/// Returns the tokens together with the bundle version they came from.
async fn extract_bundle_tokens_once(client: &Client) -> Result<(BundleTokens, String)> {
    // Step 1: Get login page to find bundle URL + version
    let (bundle_url, version) = fetch_bundle_url(client).await?;
    let full_bundle_url = format!("{}{}", BUNDLE_BASE_URL, bundle_url);

    // Step 2: Fetch the bundle (large; bounded by BUNDLE_FETCH_TIMEOUT)
    let bundle_content = client
        .get(&full_bundle_url)
        .timeout(BUNDLE_FETCH_TIMEOUT)
        .send()
        .await?
        .text()
        .await?;

    // Step 3: Extract app_id
    let app_id = extract_app_id(&bundle_content)?;

    // Step 4: Extract secrets
    let secrets = extract_secrets(&bundle_content)?;

    if secrets.is_empty() {
        return Err(ApiError::BundleExtractionError(
            "No secrets found in bundle".to_string(),
        ));
    }

    // Step 5: Extract OAuth private_key (optional - present in newer bundles)
    let private_key = extract_private_key(&bundle_content);
    if private_key.is_some() {
        log::info!("OAuth private_key extracted from bundle");
    } else {
        log::debug!("OAuth private_key not found in bundle (older bundle version)");
    }

    Ok((
        BundleTokens {
            app_id,
            secrets,
            private_key,
        },
        version,
    ))
}

/// Extract app_id, secrets, and OAuth private_key from the live Qobuz bundle,
/// with a small retry loop, and persist the result to the on-disk cache.
///
/// This is the network ("cold") path. Prefer [`load_cached_bundle`] +
/// [`refresh_bundle_if_changed`] on warm starts so the UI never blocks on the
/// 7 MB download.
pub async fn extract_and_cache_bundle_tokens(client: &Client) -> Result<BundleTokens> {
    extract_and_cache_bundle_tokens_in(client, None).await
}

pub(crate) async fn extract_and_cache_bundle_tokens_in(
    client: &Client,
    cache_dir: Option<&Path>,
) -> Result<BundleTokens> {
    let mut last_err: Option<ApiError> = None;
    let attempts = BUNDLE_EXTRACTION_RETRIES + 1;
    for attempt in 1..=attempts {
        match extract_bundle_tokens_once(client).await {
            Ok((tokens, version)) => {
                save_cached_bundle(
                    &CachedBundle {
                        bundle_version: version,
                        app_id: tokens.app_id.clone(),
                        secrets: tokens.secrets.clone(),
                        private_key: tokens.private_key.clone(),
                        fetched_at: now_unix(),
                    },
                    cache_dir,
                );
                return Ok(tokens);
            }
            Err(e) => {
                log::warn!(
                    "[Bundle] Extraction attempt {}/{} failed: {}",
                    attempt,
                    attempts,
                    e
                );
                last_err = Some(e);
                // Back off before the next attempt. The attempts used to fire
                // back-to-back, so a brief network hiccup (DNS blip, dropped
                // connection, captive-portal redirect) failed all of them in a
                // few ms — the retries were effectively useless. A short growing
                // delay gives a transient failure time to clear.
                if attempt < attempts {
                    tokio::time::sleep(Duration::from_millis(600 * attempt as u64)).await;
                }
            }
        }
    }
    Err(last_err
        .unwrap_or_else(|| ApiError::BundleExtractionError("bundle extraction failed".into())))
}

/// Background refresh: cheaply re-check the current bundle version. If Qobuz
/// rotated the bundle, re-extract (and re-cache) the new secrets and return
/// them; if unchanged, just bump the cache freshness timestamp and return
/// `None`. Never blocks the UI — call from a spawned task.
pub async fn refresh_bundle_if_changed(
    client: &Client,
    cached_version: &str,
) -> Option<BundleTokens> {
    refresh_bundle_if_changed_in(client, cached_version, None).await
}

pub(crate) async fn refresh_bundle_if_changed_in(
    client: &Client,
    cached_version: &str,
    cache_dir: Option<&Path>,
) -> Option<BundleTokens> {
    let (_, version) = fetch_bundle_url(client).await.ok()?;
    if version == cached_version {
        if let Some(mut c) = load_cached_bundle_from(cache_dir) {
            c.fetched_at = now_unix();
            save_cached_bundle(&c, cache_dir);
        }
        log::debug!("[Bundle] Background check: version {} unchanged", version);
        return None;
    }
    log::info!(
        "[Bundle] Background check: version changed {} -> {}, re-extracting",
        cached_version,
        version
    );
    extract_and_cache_bundle_tokens_in(client, cache_dir)
        .await
        .ok()
}

/// Backwards-compatible one-shot extraction (no caching). Retained for callers
/// that just want a live fetch; the app startup path uses
/// [`extract_and_cache_bundle_tokens`] instead.
pub async fn extract_bundle_tokens(client: &Client) -> Result<BundleTokens> {
    extract_bundle_tokens_once(client).await.map(|(t, _)| t)
}

fn extract_bundle_url(html: &str) -> Result<String> {
    // Pattern: <script src="/resources/X.X.X-bXXX/bundle.js"></script>
    let re =
        Regex::new(r#"<script src="(/resources/\d+\.\d+\.\d+-[a-z]\d{3}/bundle\.js)"></script>"#)
            .expect("Invalid regex");

    re.captures(html)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| ApiError::BundleExtractionError("Bundle URL not found".to_string()))
}

fn extract_app_id(bundle: &str) -> Result<String> {
    // Pattern: production:{api:{appId:"XXXXXXXXX"
    let re = Regex::new(r#"production:\{api:\{appId:"(?P<app_id>\d{9})""#).expect("Invalid regex");

    re.captures(bundle)
        .and_then(|caps| caps.name("app_id"))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| ApiError::BundleExtractionError("App ID not found".to_string()))
}

fn extract_secrets(bundle: &str) -> Result<Vec<String>> {
    // Extract seeds with their timezone keys
    // Pattern: X.initialSeed("SEED",window.utimezone.TIMEZONE)
    let seed_re = Regex::new(
        r#"[a-z]\.initialSeed\("(?P<seed>[\w=]+)",window\.utimezone\.(?P<timezone>[a-z]+)\)"#,
    )
    .expect("Invalid regex");

    let mut seeds: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut timezones: Vec<String> = Vec::new();

    for caps in seed_re.captures_iter(bundle) {
        if let (Some(seed), Some(tz)) = (caps.name("seed"), caps.name("timezone")) {
            let tz_str = tz.as_str().to_string();
            seeds.insert(tz_str.clone(), seed.as_str().to_string());
            timezones.push(tz_str);
        }
    }

    log::debug!(
        "Found {} seeds with timezones: {:?}",
        seeds.len(),
        timezones
    );

    if seeds.is_empty() {
        return Err(ApiError::BundleExtractionError(
            "No seeds found".to_string(),
        ));
    }

    // Build dynamic regex with found timezones (capitalize first letter for matching)
    // Pattern: name:"\w+/Timezone",info:"INFO",extras:"EXTRAS"
    let tz_pattern: Vec<String> = timezones
        .iter()
        .map(|tz| {
            // Capitalize first letter: "berlin" -> "Berlin"
            let mut chars = tz.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect();

    let tz_alternatives = tz_pattern.join("|");
    let info_pattern = format!(
        r#"name:"\w+/(?P<timezone>{})",info:"(?P<info>[\w=]+)",extras:"(?P<extras>[\w=]+)""#,
        tz_alternatives
    );

    log::debug!("Info regex pattern: {}", info_pattern);

    let info_re = Regex::new(&info_pattern).expect("Invalid info regex");

    let mut secrets = Vec::new();

    for caps in info_re.captures_iter(bundle) {
        if let (Some(tz), Some(info), Some(extras)) = (
            caps.name("timezone"),
            caps.name("info"),
            caps.name("extras"),
        ) {
            // Convert capitalized timezone back to lowercase for lookup
            let tz_lower = tz.as_str().to_lowercase();
            if let Some(seed) = seeds.get(&tz_lower) {
                // Concatenate seed + info + extras, remove last 44 chars, base64 decode
                let combined = format!("{}{}{}", seed, info.as_str(), extras.as_str());
                log::debug!(
                    "Combined length: {}, timezone: {}",
                    combined.len(),
                    tz_lower
                );

                if combined.len() > 44 {
                    let trimmed = &combined[..combined.len() - 44];
                    match base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        trimmed,
                    ) {
                        Ok(decoded) => {
                            if let Ok(secret) = String::from_utf8(decoded) {
                                log::info!(
                                    "Successfully extracted secret for timezone: {}",
                                    tz_lower
                                );
                                secrets.push(secret);
                            }
                        }
                        Err(e) => {
                            log::debug!("Base64 decode failed for {}: {}", tz_lower, e);
                        }
                    }
                }
            }
        }
    }

    // If the complex extraction fails, try a simpler pattern
    // that might work for some bundle versions
    if secrets.is_empty() {
        log::warn!("Complex extraction failed, trying simple appSecret pattern");
        let simple_re = Regex::new(r#"appSecret:"([a-f0-9]{32})""#).expect("Invalid regex");
        for caps in simple_re.captures_iter(bundle) {
            if let Some(secret) = caps.get(1) {
                secrets.push(secret.as_str().to_string());
            }
        }
    }

    log::info!("Extracted app secrets from bundle");
    Ok(secrets)
}

fn extract_private_key(bundle: &str) -> Option<String> {
    // Pattern: privateKey:"VALUE" (the static OAuth key used in /oauth/callback)
    let re = Regex::new(r#"privateKey:\s*"(?P<key>[A-Za-z0-9]{6,30})""#).expect("Invalid regex");

    re.captures(bundle)
        .and_then(|caps| caps.name("key"))
        .map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_bundle_url() {
        let html = r#"<script src="/resources/7.0.1-b001/bundle.js"></script>"#;
        let result = extract_bundle_url(html);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/resources/7.0.1-b001/bundle.js");
    }

    /// An embedder's root is used verbatim, and the `qbz/` namespace inside it
    /// is kept, so the cache has the same name and shape wherever it lands.
    #[test]
    fn an_explicit_cache_root_keeps_the_qbz_namespace() {
        let root = Path::new("/app/cache");
        assert_eq!(
            cache_path(Some(root)),
            Some(root.join("qbz").join("bundle_tokens.json"))
        );
    }

    /// Without one, nothing about the desktop's path changes.
    #[test]
    fn no_explicit_root_falls_back_to_the_platform_cache_dir() {
        assert_eq!(
            cache_path(None),
            dirs::cache_dir().map(|d| d.join("qbz").join("bundle_tokens.json"))
        );
    }

    /// The configurable root from #708 must cover the complete write/read path,
    /// not only path construction. The atomic writer also creates its namespace
    /// on first use.
    #[test]
    fn explicit_cache_root_round_trips_through_the_atomic_writer() {
        let root = tempfile::tempdir().expect("cache root");
        let expected = CachedBundle {
            bundle_version: "8.1.0-b019".to_string(),
            app_id: "123456789".to_string(),
            secrets: vec!["secret".to_string()],
            private_key: Some("private-key".to_string()),
            fetched_at: 42,
        };

        save_cached_bundle(&expected, Some(root.path()));

        let actual = load_cached_bundle_from(Some(root.path())).expect("cached bundle");
        assert_eq!(actual.bundle_version, expected.bundle_version);
        assert_eq!(actual.app_id, expected.app_id);
        assert_eq!(actual.secrets, expected.secrets);
        assert_eq!(actual.private_key, expected.private_key);
        assert_eq!(actual.fetched_at, expected.fetched_at);
    }

    #[test]
    fn test_extract_app_id() {
        let bundle = r#"production:{api:{appId:"123456789",appSecret:"abc"}"#;
        let result = extract_app_id(bundle);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "123456789");
    }

    /// The invariant the temp-file-and-rename buys: a write that dies partway
    /// leaves the tokens that were already there.
    ///
    /// Swap `atomic_write` back to `std::fs::write` and this fails with an empty
    /// file, which is the state a kill during the launch-time `fetched_at`
    /// refresh used to leave behind.
    #[test]
    fn interrupted_cache_write_preserves_the_previous_file() {
        let root = tempfile::tempdir().expect("cache root");
        let path = root.path().join("qbz").join("bundle_tokens.json");
        atomic_write(&path, b"previous cache").expect("initial cache write");

        let result = atomic_write_with(&path, |file| {
            file.write_all(b"partial replacement")?;
            Err(io::Error::other("simulated interruption"))
        });

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&path).expect("previous cache remains readable"),
            b"previous cache"
        );
    }

    /// What losing the file costs, and why the invariant above is worth a temp
    /// file: there is no partial recovery. Any truncated cache is simply gone,
    /// and the next launch pays a full live bundle extraction.
    #[test]
    fn a_truncated_cache_cannot_be_salvaged() {
        let root = tempfile::tempdir().expect("cache root");
        let path = root.path().join("bundle_tokens.json");
        let whole = serde_json::to_vec_pretty(&CachedBundle {
            bundle_version: "8.1.0-b019".to_string(),
            app_id: "123456789".to_string(),
            secrets: vec!["secret".to_string()],
            private_key: None,
            fetched_at: 1,
        })
        .expect("serialize");

        for kept in [0, whole.len() / 2] {
            std::fs::write(&path, &whole[..kept]).expect("write truncated cache");
            let data = std::fs::read(&path).expect("read truncated cache");
            assert!(
                serde_json::from_slice::<CachedBundle>(&data).is_err(),
                "{kept} of {} bytes should not parse",
                whole.len()
            );
        }
    }

    /// The replacement file is owner-only, where writing in place left it at
    /// whatever the umask allowed (0644 on a default Linux setup). These are
    /// app-level values scraped from a public bundle rather than user
    /// credentials, so this is tidiness on a shared machine, not a fix for a
    /// vulnerability.
    #[cfg(unix)]
    #[test]
    fn cache_file_is_owner_readable_and_writable_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("cache root");
        let path = root.path().join("qbz").join("bundle_tokens.json");
        atomic_write(&path, b"cache").expect("cache write");

        let mode = std::fs::metadata(path)
            .expect("cache metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// The cache dir is created on demand, so a first run on a clean machine
    /// caches its tokens rather than warning and moving on.
    #[test]
    fn a_missing_cache_directory_is_created() {
        let root = tempfile::tempdir().expect("cache root");
        let path = root.path().join("qbz").join("nested").join("tokens.json");

        atomic_write(&path, b"cache").expect("cache write into a missing directory");

        assert_eq!(std::fs::read(&path).expect("cache readable"), b"cache");
    }
}
