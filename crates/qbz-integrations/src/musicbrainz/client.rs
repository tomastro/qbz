//! MusicBrainz API client
//!
//! HTTP client with rate limiting and proper User-Agent handling.
//! Uses Cloudflare Workers proxy for consistent rate limiting.

use reqwest::Client;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use super::models::*;
use crate::error::{IntegrationError, IntegrationResult};

/// Proxy URL for MusicBrainz requests
const MUSICBRAINZ_PROXY_URL: &str = "https://qbz-api-proxy.blitzkriegfc.workers.dev/musicbrainz";

/// Direct MusicBrainz API URL (fallback)
const MUSICBRAINZ_API_URL: &str = "https://musicbrainz.org/ws/2";

/// Rate limiter for MusicBrainz API
pub struct RateLimiter {
    last_request: Mutex<Instant>,
    min_interval: Duration,
}

impl RateLimiter {
    /// Create rate limiter for direct MusicBrainz API (1 req/sec)
    pub fn new() -> Self {
        Self::with_interval(Duration::from_millis(1100))
    }

    /// Create rate limiter for proxy (faster, proxy handles actual rate limiting)
    pub fn for_proxy() -> Self {
        Self::with_interval(Duration::from_millis(200))
    }

    /// Create rate limiter with custom interval
    pub fn with_interval(min_interval: Duration) -> Self {
        Self {
            // Start in the past so first request doesn't wait
            last_request: Mutex::new(Instant::now() - Duration::from_secs(2)),
            min_interval,
        }
    }

    pub async fn wait(&self) {
        let mut last = self.last_request.lock().await;
        let elapsed = last.elapsed();
        if elapsed < self.min_interval {
            tokio::time::sleep(self.min_interval - elapsed).await;
        }
        *last = Instant::now();
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// MusicBrainz API client configuration
#[derive(Debug, Clone)]
pub struct MusicBrainzConfig {
    /// Whether MusicBrainz integration is enabled
    pub enabled: bool,
    /// Use proxy instead of direct API
    pub use_proxy: bool,
}

impl Default for MusicBrainzConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // Direct-to-MusicBrainz by default: each client uses its OWN IP, so
            // the per-IP 1 req/s budget is per-user instead of shared across all
            // QBZ users behind the proxy's Cloudflare egress IPs (which is what
            // triggered the 503s). MB read access needs no key, so the proxy
            // added only the funnel. Flip to true to route via the proxy again.
            use_proxy: false,
        }
    }
}

/// MusicBrainz API client
pub struct MusicBrainzClient {
    client: Client,
    rate_limiter: Arc<RateLimiter>,
    config: Arc<Mutex<MusicBrainzConfig>>,
}

impl Default for MusicBrainzClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MusicBrainzClient {
    /// Create a new MusicBrainz client with default config
    pub fn new() -> Self {
        Self::with_config(MusicBrainzConfig::default())
    }

    /// Create client with specific configuration
    pub fn with_config(config: MusicBrainzConfig) -> Self {
        let version = "1.0.0";
        let user_agent = format!(
            "QBZ/{} (https://github.com/vicrodh/qbz; qbz@vicrodh.dev)",
            version
        );

        let client = Client::builder()
            .user_agent(&user_agent)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        // Use faster rate limiter when using proxy
        let rate_limiter = if config.use_proxy {
            RateLimiter::for_proxy()
        } else {
            RateLimiter::new()
        };

        Self {
            client,
            rate_limiter: Arc::new(rate_limiter),
            config: Arc::new(Mutex::new(config)),
        }
    }

    /// Check if MusicBrainz integration is enabled
    pub async fn is_enabled(&self) -> bool {
        self.config.lock().await.enabled
    }

    /// Enable or disable MusicBrainz integration
    pub async fn set_enabled(&self, enabled: bool) {
        self.config.lock().await.enabled = enabled;
    }

    /// Get the base URL based on configuration
    async fn base_url(&self) -> &'static str {
        if self.config.lock().await.use_proxy {
            MUSICBRAINZ_PROXY_URL
        } else {
            MUSICBRAINZ_API_URL
        }
    }

    /// Search recordings by ISRC
    pub async fn search_recording_by_isrc(
        &self,
        isrc: &str,
    ) -> IntegrationResult<RecordingSearchResponse> {
        if !self.is_enabled().await {
            return Err(IntegrationError::ServiceUnavailable(
                "MusicBrainz integration is disabled".into(),
            ));
        }

        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let url = format!("{}/recording?query=isrc:{}&fmt=json", base, isrc);

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Search artists by name
    pub async fn search_artist(
        &self,
        name: &str,
        limit: u32,
    ) -> IntegrationResult<ArtistSearchResponse> {
        if !self.is_enabled().await {
            return Err(IntegrationError::ServiceUnavailable(
                "MusicBrainz integration is disabled".into(),
            ));
        }

        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let encoded_name = urlencoding::encode(name);
        let url = format!(
            "{}/artist?query=artist:{}&limit={}&fmt=json",
            base, encoded_name, limit
        );

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Resolve a track to get MusicBrainz IDs
    ///
    /// Searches by ISRC if available, falling back to text search.
    pub async fn resolve_track(
        &self,
        artist: &str,
        title: &str,
        isrc: Option<&str>,
    ) -> IntegrationResult<Option<ResolvedTrack>> {
        // Try ISRC first (most accurate)
        if let Some(isrc) = isrc {
            let response = self.search_recording_by_isrc(isrc).await?;
            if let Some(recording) = response.recordings.first() {
                let confidence = if recording
                    .isrcs
                    .as_ref()
                    .map_or(false, |isrcs| isrcs.contains(&isrc.to_string()))
                {
                    MatchConfidence::Exact
                } else {
                    MatchConfidence::from_score(recording.score)
                };

                return Ok(Some(ResolvedTrack {
                    recording_mbid: recording.id.clone(),
                    title: recording.title.clone().unwrap_or_default(),
                    artist_mbids: recording
                        .artist_credit
                        .as_ref()
                        .map(|ac| ac.iter().map(|a| a.artist.id.clone()).collect())
                        .unwrap_or_default(),
                    release_mbid: recording
                        .releases
                        .as_ref()
                        .and_then(|r| r.first())
                        .map(|r| r.id.clone()),
                    isrcs: recording.isrcs.clone().unwrap_or_default(),
                    confidence,
                }));
            }
        }

        // TODO: Implement text-based search fallback
        // For now, return None if ISRC search fails
        let _ = (artist, title); // Silence unused warnings
        Ok(None)
    }

    /// Resolve an artist to get MusicBrainz ID
    ///
    /// Prefers exact name matches over highest score to avoid disambiguation
    /// issues (e.g., multiple artists named "The Warning").
    pub async fn resolve_artist(&self, name: &str) -> IntegrationResult<Option<ResolvedArtist>> {
        let response = self.search_artist(name, 10).await?;

        if response.artists.is_empty() {
            return Ok(None);
        }

        // Prefer exact name match (case-insensitive)
        let target = name.trim().to_lowercase();
        let best = response
            .artists
            .iter()
            .find(|a| a.name.trim().to_lowercase() == target && a.score.unwrap_or(0) >= 90)
            .or_else(|| response.artists.first());

        if let Some(artist) = best {
            let confidence = MatchConfidence::from_score(artist.score);

            return Ok(Some(ResolvedArtist {
                mbid: artist.id.clone(),
                name: artist.name.clone(),
                sort_name: artist.sort_name.clone(),
                artist_type: ArtistType::from(artist.artist_type.as_deref()),
                country: artist.country.clone(),
                disambiguation: artist.disambiguation.clone(),
                confidence,
            }));
        }

        Ok(None)
    }

    // ============ Extended API Methods ============

    /// Search recordings by title and artist
    pub async fn search_recording(
        &self,
        title: &str,
        artist: &str,
    ) -> IntegrationResult<RecordingSearchResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let query = format!(
            "recording:\"{}\" AND artist:\"{}\"",
            Self::escape_query(title),
            Self::escape_query(artist)
        );
        let url = format!(
            "{}/recording?query={}&fmt=json&limit=5",
            base,
            urlencoding::encode(&query)
        );

        let response = self.client.get(&url).send().await?;
        self.check_response(&response).await;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Get artist details with relationships and tags
    pub async fn get_artist_with_relations(
        &self,
        mbid: &str,
    ) -> IntegrationResult<ArtistFullResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let url = format!("{}/artist/{}?inc=artist-rels+tags&fmt=json", base, mbid);

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Fetch artist tags only (lightweight, no relations)
    pub async fn get_artist_tags(&self, mbid: &str) -> IntegrationResult<Vec<String>> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let url = format!("{}/artist/{}?inc=tags&fmt=json", base, mbid);

        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let artist: ArtistFullResponse = response.json().await.map_err(|e| {
            IntegrationError::internal(format!("Failed to parse MusicBrainz response: {}", e))
        })?;

        let mut tags: Vec<_> = artist
            .tags
            .unwrap_or_default()
            .into_iter()
            .filter(|tag| tag.count.unwrap_or(0) > 0)
            .collect();
        tags.sort_by(|a, b| b.count.unwrap_or(0).cmp(&a.count.unwrap_or(0)));
        Ok(tags
            .into_iter()
            .map(|tag| tag.name.to_lowercase())
            .collect())
    }

    /// MBID -> ISRCs bridge. Looks up a recording's ISRCs (the strong key for Qobuz matching).
    /// GET {base}/recording/{recording_mbid}?inc=isrcs&fmt=json
    /// Returns the ISRC list, or an EMPTY vec on any non-success/parse failure (a missing ISRC is normal, not an error).
    pub async fn get_recording_isrcs(&self, recording_mbid: &str) -> IntegrationResult<Vec<String>> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;
        let base = self.base_url().await;
        let url = format!("{}/recording/{}?inc=isrcs&fmt=json", base, recording_mbid);
        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
            return Ok(Vec::new());
        }
        let parsed: RecordingLookupResponse = match response.json().await {
            Ok(p) => p,
            Err(_) => return Ok(Vec::new()),
        };
        Ok(parsed.isrcs.unwrap_or_default())
    }

    /// Search artists by tag (genre)
    pub async fn search_artists_by_tag(
        &self,
        tag: &str,
        limit: usize,
    ) -> IntegrationResult<ArtistSearchResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let limit = limit.min(100).max(1);
        let query = format!("tag:\"{}\"", Self::escape_query(tag));
        let url = format!(
            "{}/artist?query={}&fmt=json&limit={}",
            base,
            urlencoding::encode(&query),
            limit
        );

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Search artists by tag AND area
    pub async fn search_artists_by_tag_and_area(
        &self,
        tag: &str,
        area_name: &str,
        country: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> IntegrationResult<ArtistSearchResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let limit = limit.min(100).max(1);
        let search_area = country.unwrap_or(area_name);
        let query = format!(
            "tag:\"{}\" AND area:\"{}\"",
            Self::escape_query(tag),
            Self::escape_query(search_area)
        );
        let url = format!(
            "{}/artist?query={}&fmt=json&limit={}&offset={}",
            base,
            urlencoding::encode(&query),
            limit,
            offset
        );

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Search releases by barcode (UPC/EAN)
    pub async fn search_release_by_barcode(
        &self,
        barcode: &str,
    ) -> IntegrationResult<ReleaseSearchResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let url = format!(
            "{}/release?query=barcode:{}&fmt=json&limit=5",
            base, barcode
        );

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Search releases by title and artist
    pub async fn search_release(
        &self,
        title: &str,
        artist: &str,
    ) -> IntegrationResult<ReleaseSearchResponse> {
        self.search_releases_extended(title, artist, None, 5).await
    }

    /// Search releases with extended options
    pub async fn search_releases_extended(
        &self,
        title: &str,
        artist: &str,
        catalog_number: Option<&str>,
        limit: usize,
    ) -> IntegrationResult<ReleaseSearchResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let query = if let Some(catno) = catalog_number.filter(|s| !s.trim().is_empty()) {
            format!(
                "catno:\"{}\" AND artist:\"{}\"",
                Self::escape_query(catno),
                Self::escape_query(artist)
            )
        } else {
            format!(
                "release:\"{}\" AND artist:\"{}\"",
                Self::escape_query(title),
                Self::escape_query(artist)
            )
        };

        let limit = limit.min(25).max(1);
        let url = format!(
            "{}/release?query={}&fmt=json&limit={}",
            base,
            urlencoding::encode(&query),
            limit
        );

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Get full release details including tracks
    pub async fn get_release_with_tracks(
        &self,
        release_id: &str,
    ) -> IntegrationResult<ReleaseFullResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let url = format!(
            "{}/release/{}?inc=recordings+artist-credits+labels+tags&fmt=json",
            base, release_id
        );

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Browse artists by area MBID
    pub async fn browse_artists_by_area(
        &self,
        area_id: &str,
        limit: usize,
        offset: usize,
    ) -> IntegrationResult<ArtistBrowseResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let limit = limit.min(100).max(1);
        let url = format!(
            "{}/artist?area={}&fmt=json&limit={}&offset={}&inc=tags",
            base, area_id, limit, offset
        );

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Search for an area by name
    pub async fn search_area(
        &self,
        name: &str,
        area_type: Option<&str>,
    ) -> IntegrationResult<AreaSearchResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let query = if let Some(atype) = area_type {
            format!(
                "area:\"{}\" AND type:\"{}\"",
                Self::escape_query(name),
                Self::escape_query(atype)
            )
        } else {
            format!("area:\"{}\"", Self::escape_query(name))
        };

        let url = format!(
            "{}/area?query={}&fmt=json&limit=5",
            base,
            urlencoding::encode(&query)
        );

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Look up an area and its parent relationships
    pub async fn get_area_with_relations(
        &self,
        area_id: &str,
    ) -> IntegrationResult<AreaDetailResponse> {
        self.check_enabled().await?;
        self.rate_limiter.wait().await;

        let base = self.base_url().await;
        let url = format!("{}/area/{}?inc=area-rels&fmt=json", base, area_id);

        let response = self.client.get(&url).send().await?;
        let response = self.handle_response_status(response).await?;
        response.json().await.map_err(Into::into)
    }

    /// Resolve a city area to its parent subdivision (state/region)
    pub async fn resolve_parent_subdivision(
        &self,
        area_id: &str,
    ) -> IntegrationResult<Option<(String, String)>> {
        let mut current_id = area_id.to_string();
        let mut path: Vec<String> = Vec::new();
        let max_hops = 5;

        for _hop in 0..max_hops {
            let detail = self.get_area_with_relations(&current_id).await?;
            path.push(format!("{}[{:?}]", detail.name, detail.area_type));

            let parents: Vec<_> = detail
                .relations
                .as_ref()
                .map(|rels| {
                    rels.iter()
                        .filter(|rel| {
                            rel.relation_type == "part of"
                                && rel.direction.as_deref() == Some("backward")
                        })
                        .filter_map(|rel| rel.area.as_ref())
                        .collect()
                })
                .unwrap_or_default();

            if parents.is_empty() {
                return Ok(None);
            }

            let has_country_parent = parents.iter().any(|p| {
                p.area_type
                    .as_deref()
                    .map(|t| t.eq_ignore_ascii_case("country"))
                    .unwrap_or(false)
            });

            if has_country_parent {
                let own_type = detail.area_type.as_deref().unwrap_or("");
                if own_type.eq_ignore_ascii_case("subdivision") {
                    if current_id == area_id {
                        return Ok(None);
                    }
                    return Ok(Some((detail.name.clone(), detail.id.clone())));
                }
                if current_id == area_id {
                    return Ok(None);
                }
                return Ok(Some((detail.name.clone(), detail.id.clone())));
            }

            let next = parents
                .iter()
                .find(|p| {
                    p.area_type
                        .as_deref()
                        .map(|t| t.eq_ignore_ascii_case("subdivision"))
                        .unwrap_or(false)
                })
                .or_else(|| {
                    parents.iter().find(|p| {
                        let t = p.area_type.as_deref().unwrap_or("");
                        !t.eq_ignore_ascii_case("city") && !t.eq_ignore_ascii_case("country")
                    })
                })
                .or_else(|| parents.first());

            match next {
                Some(parent) => {
                    current_id = parent.id.clone();
                }
                None => {
                    return Ok(None);
                }
            }
        }

        Ok(None)
    }

    /// Resolve the country that an area belongs to by walking up the hierarchy.
    ///
    /// Frankfurt am Main → Hessen → Germany → returns ("Germany", Some("de"))
    /// Monterrey → Nuevo León → Mexico → returns ("Mexico", Some("mx"))
    ///
    /// Returns (country_name, country_code_lowercase) or None.
    pub async fn resolve_area_country(
        &self,
        area_id: &str,
    ) -> IntegrationResult<Option<(String, Option<String>)>> {
        let mut current_id = area_id.to_string();
        let max_hops = 5;

        for _hop in 0..max_hops {
            let detail = self.get_area_with_relations(&current_id).await?;

            // If current area IS a country, return it
            if detail
                .area_type
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("country"))
                .unwrap_or(false)
            {
                let code = detail
                    .iso_codes
                    .as_ref()
                    .and_then(|c| c.first())
                    .map(|c| c.to_lowercase());
                return Ok(Some((detail.name, code)));
            }

            // Find "part of" parent
            let parent = detail
                .relations
                .as_ref()
                .and_then(|rels| {
                    rels.iter()
                        .find(|rel| {
                            rel.relation_type == "part of"
                                && rel.direction.as_deref() == Some("backward")
                        })
                        .and_then(|rel| rel.area.as_ref())
                });

            match parent {
                Some(p) => {
                    if p.area_type
                        .as_deref()
                        .map(|t| t.eq_ignore_ascii_case("country"))
                        .unwrap_or(false)
                    {
                        let code = p
                            .iso_codes
                            .as_ref()
                            .and_then(|c| c.first())
                            .map(|c| c.to_lowercase());
                        return Ok(Some((p.name.clone(), code)));
                    }
                    current_id = p.id.clone();
                }
                None => return Ok(None),
            }
        }

        Ok(None)
    }

    // ============ Internal Helpers ============

    async fn check_enabled(&self) -> IntegrationResult<()> {
        if !self.is_enabled().await {
            return Err(IntegrationError::ServiceUnavailable(
                "MusicBrainz integration is disabled".into(),
            ));
        }
        Ok(())
    }

    #[allow(unused)]
    async fn check_response(&self, _response: &reqwest::Response) {
        // Placeholder for response logging/metrics
    }

    async fn handle_response_status(
        &self,
        response: reqwest::Response,
    ) -> IntegrationResult<reqwest::Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        // 429 (proxy-translated) and 503 (direct MusicBrainz) both signal that
        // the rate limit was hit. Surface the server's Retry-After so the caller
        // can back off instead of treating it as a generic error.
        if matches!(status.as_u16(), 429 | 503) {
            return Err(IntegrationError::RateLimited(Self::parse_retry_after(
                &response,
            )));
        }
        let text = response.text().await.unwrap_or_default();
        Err(IntegrationError::internal(format!(
            "MusicBrainz API error {}: {}",
            status, text
        )))
    }

    /// Parse the `Retry-After` header (whole seconds). MusicBrainz sends it on
    /// HTTP 503 when the per-IP rate limit is exceeded. Defaults to 1s because
    /// MB's per-IP limiter recovers within ~1 second.
    fn parse_retry_after(response: &reqwest::Response) -> u64 {
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .unwrap_or(1)
    }

    /// Escape special characters in Lucene queries
    fn escape_query(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace(':', "\\:")
            .replace('(', "\\(")
            .replace(')', "\\)")
            .replace('[', "\\[")
            .replace(']', "\\]")
            .replace('{', "\\{")
            .replace('}', "\\}")
            .replace('^', "\\^")
            .replace('~', "\\~")
            .replace('*', "\\*")
            .replace('?', "\\?")
            .replace('!', "\\!")
            .replace('+', "\\+")
            .replace('-', "\\-")
            .replace('&', "\\&")
            .replace('|', "\\|")
    }
}
