//! Unified DTOs for remote metadata providers (MusicBrainz, Discogs)
//!
//! These structs provide a provider-neutral interface for the Tag Editor
//! to consume album metadata from different sources.

use serde::{Deserialize, Serialize};

/// Remote metadata provider
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteProvider {
    MusicBrainz,
    Discogs,
}

impl std::fmt::Display for RemoteProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MusicBrainz => write!(f, "musicbrainz"),
            Self::Discogs => write!(f, "discogs"),
        }
    }
}

impl std::str::FromStr for RemoteProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "musicbrainz" | "mb" => Ok(Self::MusicBrainz),
            "discogs" => Ok(Self::Discogs),
            _ => Err(format!("Unknown provider: {}", s)),
        }
    }
}

/// Lightweight search result for displaying in results list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAlbumSearchResult {
    /// Provider identifier
    pub provider: RemoteProvider,
    /// Provider-specific ID (MusicBrainz release ID or Discogs release ID)
    pub provider_id: String,
    /// Album title
    pub title: String,
    /// Album artist
    pub artist: String,
    /// Release year (extracted from date)
    pub year: Option<u16>,
    /// Number of tracks (if available from search)
    pub track_count: Option<u16>,
    /// Release country
    pub country: Option<String>,
    /// Record label
    pub label: Option<String>,
    /// Catalog number
    pub catalog_number: Option<String>,
    /// Match confidence (0-100, provider-specific)
    pub confidence: Option<u8>,
    /// Format info (e.g., "CD", "Vinyl", "Digital")
    pub format: Option<String>,
}

/// Full album metadata with tracks (for applying to form)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAlbumMetadata {
    /// Provider identifier
    pub provider: RemoteProvider,
    /// Provider-specific ID
    pub provider_id: String,
    /// Album title
    pub title: String,
    /// Album artist
    pub artist: String,
    /// Ordered artist-credit components. `artist` remains the display credit
    /// including join phrases; these rows preserve identity without guessing
    /// how a human-readable credit should be split.
    #[serde(default)]
    pub artist_credits: Vec<RemoteArtistCredit>,
    /// Release year
    pub year: Option<u16>,
    /// Genres/styles/tags
    pub genres: Vec<String>,
    /// Record label
    pub label: Option<String>,
    /// Catalog number
    pub catalog_number: Option<String>,
    /// Release country
    pub country: Option<String>,
    /// Barcode/UPC (if available)
    pub barcode: Option<String>,
    /// Track list organized by disc
    pub tracks: Vec<RemoteTrackMetadata>,
    /// Total disc count
    pub disc_count: u8,
    /// URL to view on provider website
    pub source_url: Option<String>,
    /// MusicBrainz release-group identity when the provider exposes it.
    #[serde(default)]
    pub release_group_id: Option<String>,
}

/// One component of an album or track artist credit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteArtistCredit {
    pub name: String,
    #[serde(default)]
    pub join_phrase: String,
    #[serde(default)]
    pub provider_id: Option<String>,
}

/// Single track metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTrackMetadata {
    /// Disc number (1-based)
    pub disc_number: u8,
    /// Track number within disc (1-based)
    pub track_number: u8,
    /// Track title
    pub title: String,
    /// Duration in milliseconds (if available)
    pub duration_ms: Option<u32>,
    /// Display credit, including intentional join phrases.
    #[serde(default)]
    pub artist_credit: String,
    /// Ordered, typed credit components.
    #[serde(default)]
    pub artist_credits: Vec<RemoteArtistCredit>,
    #[serde(default)]
    pub recording_id: Option<String>,
    #[serde(default)]
    pub track_id: Option<String>,
}

/// Search request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSearchRequest {
    /// Provider to search
    pub provider: RemoteProvider,
    /// Search query (usually "artist album")
    pub query: String,
    /// Optional catalog number for more precise matching
    pub catalog_id: Option<String>,
    /// Optional artist name for filtering
    pub artist: Option<String>,
    /// Maximum results to return (default: 10)
    pub limit: Option<usize>,
}

impl RemoteSearchRequest {
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(10).min(25)
    }
}

/// Search response wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSearchResponse {
    /// Provider that was searched
    pub provider: RemoteProvider,
    /// Search results
    pub results: Vec<RemoteAlbumSearchResult>,
    /// Total results available (may be more than returned)
    pub total_count: Option<usize>,
    /// Whether rate limit was hit
    pub rate_limited: bool,
}

impl RemoteSearchResponse {
    pub fn empty(provider: RemoteProvider) -> Self {
        Self {
            provider,
            results: Vec::new(),
            total_count: Some(0),
            rate_limited: false,
        }
    }

    pub fn rate_limited(provider: RemoteProvider) -> Self {
        Self {
            provider,
            results: Vec::new(),
            total_count: None,
            rate_limited: true,
        }
    }
}

/// Error types for remote metadata operations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "message")]
pub enum RemoteMetadataError {
    /// Network or connection error
    NetworkError(String),
    /// Rate limit exceeded
    RateLimited(String),
    /// Invalid response from provider
    InvalidResponse(String),
    /// No results found
    NoResults,
    /// Provider not available
    ProviderUnavailable(String),
    /// Invalid provider ID
    InvalidProviderId(String),
}

impl std::fmt::Display for RemoteMetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NetworkError(msg) => write!(f, "Network error: {}", msg),
            Self::RateLimited(msg) => write!(f, "Rate limited: {}", msg),
            Self::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            Self::NoResults => write!(f, "No results found"),
            Self::ProviderUnavailable(msg) => write!(f, "Provider unavailable: {}", msg),
            Self::InvalidProviderId(msg) => write!(f, "Invalid provider ID: {}", msg),
        }
    }
}

impl From<RemoteMetadataError> for String {
    fn from(err: RemoteMetadataError) -> Self {
        err.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_parsing() {
        assert_eq!(
            "musicbrainz".parse::<RemoteProvider>().unwrap(),
            RemoteProvider::MusicBrainz
        );
        assert_eq!(
            "discogs".parse::<RemoteProvider>().unwrap(),
            RemoteProvider::Discogs
        );
        assert_eq!(
            "MB".parse::<RemoteProvider>().unwrap(),
            RemoteProvider::MusicBrainz
        );
        assert!("unknown".parse::<RemoteProvider>().is_err());
    }

    #[test]
    fn test_search_request_limit() {
        let req = RemoteSearchRequest {
            provider: RemoteProvider::MusicBrainz,
            query: "test".to_string(),
            catalog_id: None,
            artist: None,
            limit: None,
        };
        assert_eq!(req.limit(), 10);

        let req_custom = RemoteSearchRequest {
            limit: Some(50),
            ..req.clone()
        };
        assert_eq!(req_custom.limit(), 25); // Capped at 25
    }
}
