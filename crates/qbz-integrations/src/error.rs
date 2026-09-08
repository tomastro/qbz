//! Error types for integrations

use thiserror::Error;

/// Result type alias for integration operations
pub type IntegrationResult<T> = Result<T, IntegrationError>;

/// Error types for third-party service integrations
#[derive(Error, Debug)]
pub enum IntegrationError {
    /// HTTP request failed
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON parsing failed
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Authentication required but not provided
    #[error("Authentication required")]
    NotAuthenticated,

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    /// Rate limit exceeded
    #[error("Rate limit exceeded, retry after {0} seconds")]
    RateLimited(u64),

    /// API returned an error
    #[error("API error ({code}): {message}")]
    ApiError { code: u32, message: String },

    /// Service temporarily unavailable
    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    /// Database/cache error (requires cache feature)
    #[cfg(feature = "cache")]
    #[error("Cache error: {0}")]
    Cache(#[from] rusqlite::Error),

    /// Generic internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl IntegrationError {
    /// Create an API error
    pub fn api(code: u32, message: impl Into<String>) -> Self {
        Self::ApiError {
            code,
            message: message.into(),
        }
    }

    /// Create an internal error
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}
