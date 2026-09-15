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
fn cache_path(cache_dir: Option<&Path>) -> Option<PathBuf> {
    let root = cache_dir.map(Path::to_path_buf).or_else(dirs::cache_dir)?;
    Some(root.join("qbz").join("bundle_tokens.json"))
}

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

/// Load cached tokens if a valid cache file exists.
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

fn bundle_version_from_url(bundle_url: &str) -> String {
    bundle_url
        .trim_start_matches("/resources/")
        .trim_end_matches("/bundle.js")
        .to_string()
}

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

async fn extract_bundle_tokens_once(client: &Client) -> Result<(BundleTokens, String)> {
    let (bundle_url, version) = fetch_bundle_url(client).await?;
    let full_bundle_url = format!("{}{}", BUNDLE_BASE_URL, bundle_url);

    let bundle_content = client
        .get(&full_bundle_url)
        .timeout(BUNDLE_FETCH_TIMEOUT)
        .send()
        .await?
        .text()
        .await?;

    let app_id = extract_app_id(&bundle_content)?;
    let secrets = extract_secrets(&bundle_content)?;

    if secrets.is_empty() {
        return Err(ApiError::BundleExtractionError(
            "No secrets found in bundle".to_string(),
        ));
    }

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
                if attempt < attempts {
                    tokio::time::sleep(Duration::from_millis(600 * attempt as u64)).await;
                }
            }
        }
    }

    log::warn!(
        "[Bundle] All live extraction attempts failed ({:?}). Using verified fallback bundle tokens.",
        last_err
    );
    let fallback = BundleTokens {
        app_id: "798273057".to_string(),
        secrets: vec![
            "806331c3b0b641da923b890aed01d04a".to_string(),
            "f69a7734686cb9427629378a4b7ac381".to_string(),
            "abb21364945c0583309667d13ca3d93a".to_string(),
        ],
        private_key: Some("6lz8C03UDIC7".to_string()),
    };
    save_cached_bundle(
        &CachedBundle {
            bundle_version: "fallback-8.2.0".to_string(),
            app_id: fallback.app_id.clone(),
            secrets: fallback.secrets.clone(),
            private_key: fallback.private_key.clone(),
            fetched_at: now_unix(),
        },
        cache_dir,
    );
    Ok(fallback)
}

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

pub async fn extract_bundle_tokens(client: &Client) -> Result<BundleTokens> {
    extract_bundle_tokens_once(client).await.map(|(t, _)| t)
}

fn extract_bundle_url(html: &str) -> Result<String> {
    let patterns = [
        r#"src=["'](/resources/[^"'\s]+/bundle\.js)["']"#,
        r#"<script src="(/resources/[^"]+/bundle\.js)"></script>"#,
        r#"(/resources/[^"'\s]+/bundle\.js)"#,
    ];

    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            if let Some(caps) = re.captures(html) {
                if let Some(m) = caps.get(1) {
                    return Ok(m.as_str().to_string());
                }
            }
        }
    }

    Err(ApiError::BundleExtractionError("Bundle URL not found".to_string()))
}

fn extract_app_id(bundle: &str) -> Result<String> {
    let patterns = [
        r#"production:\{api:\{appId:"(?P<app_id>\d{9})""#,
        r#"api:\{appId:"(?P<app_id>\d{9})""#,
        r#"appId:"(?P<app_id>\d{9})""#,
        r#"app_id:"(?P<app_id>\d{9})""#,
    ];

    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            if let Some(caps) = re.captures(bundle) {
                if let Some(m) = caps.name("app_id") {
                    return Ok(m.as_str().to_string());
                }
            }
        }
    }

    Err(ApiError::BundleExtractionError("App ID not found".to_string()))
}

fn extract_secrets(bundle: &str) -> Result<Vec<String>> {
    let seed_re = Regex::new(
        r#"(?:[a-z]\.)?initialSeed\("(?P<seed>[\w=]+)"\s*,\s*(?:window\.)?utimezone\.(?P<timezone>[a-z]+)\)"#,
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

    let mut secrets = Vec::new();

    if !seeds.is_empty() {
        let tz_pattern: Vec<String> = timezones
            .iter()
            .map(|tz| {
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

        if let Ok(info_re) = Regex::new(&info_pattern) {
            for caps in info_re.captures_iter(bundle) {
                if let (Some(tz), Some(info), Some(extras)) = (
                    caps.name("timezone"),
                    caps.name("info"),
                    caps.name("extras"),
                ) {
                    let tz_lower = tz.as_str().to_lowercase();
                    if let Some(seed) = seeds.get(&tz_lower) {
                        let combined = format!("{}{}{}", seed, info.as_str(), extras.as_str());
                        if combined.len() > 44 {
                            let trimmed = &combined[..combined.len() - 44];
                            if let Ok(decoded) = base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                trimmed,
                            ) {
                                if let Ok(secret) = String::from_utf8(decoded) {
                                    secrets.push(secret);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if secrets.is_empty() {
        let simple_patterns = [
            r#"appSecret:"([a-f0-9]{32})""#,
            r#"appSecret:\s*["']([a-f0-9]{32})["']"#,
            r#"app_secret:\s*["']([a-f0-9]{32})["']"#,
        ];
        for pat in simple_patterns {
            if let Ok(simple_re) = Regex::new(pat) {
                for caps in simple_re.captures_iter(bundle) {
                    if let Some(secret) = caps.get(1) {
                        let s = secret.as_str().to_string();
                        if !secrets.contains(&s) {
                            secrets.push(s);
                        }
                    }
                }
            }
        }
    }

    if secrets.is_empty() {
        return Err(ApiError::BundleExtractionError(
            "No secrets found in bundle".to_string(),
        ));
    }

    log::info!("Extracted {} app secrets from bundle", secrets.len());
    Ok(secrets)
}

fn extract_private_key(bundle: &str) -> Option<String> {
    let patterns = [
        r#"privateKey:\s*["'](?P<key>[A-Za-z0-9]{6,30})["']"#,
        r#"private_key:\s*["'](?P<key>[A-Za-z0-9]{6,30})["']"#,
    ];

    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            if let Some(caps) = re.captures(bundle) {
                if let Some(m) = caps.name("key") {
                    return Some(m.as_str().to_string());
                }
            }
        }
    }

    None
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

    #[test]
    fn test_extract_app_id() {
        let bundle = r#"production:{api:{appId:"123456789",appSecret:"abc"}"#;
        let result = extract_app_id(bundle);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "123456789");
    }
}
