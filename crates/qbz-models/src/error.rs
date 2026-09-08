//! Common error types for QBZ
//!
//! This module defines error types that are shared across crates.

use thiserror::Error;

/// Top-level error type for QBZ operations
#[derive(Error, Debug)]
pub enum QbzError {
    /// Authentication failed
    #[error("Authentication failed: {0}")]
    AuthError(String),

    /// API request failed
    #[error("API error: {0}")]
    ApiError(String),

    /// Network error
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Playback error
    #[error("Playback error: {0}")]
    PlaybackError(String),

    /// Audio device error
    #[error("Audio device error: {0}")]
    AudioDeviceError(String),

    /// Queue error
    #[error("Queue error: {0}")]
    QueueError(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Storage/database error
    #[error("Storage error: {0}")]
    StorageError(String),

    /// Track not found
    #[error("Track not found: {0}")]
    TrackNotFound(u64),

    /// Album not found
    #[error("Album not found: {0}")]
    AlbumNotFound(String),

    /// Artist not found
    #[error("Artist not found: {0}")]
    ArtistNotFound(u64),

    /// Playlist not found
    #[error("Playlist not found: {0}")]
    PlaylistNotFound(u64),

    /// Not authenticated (login required)
    #[error("Not authenticated")]
    NotAuthenticated,

    /// Stream URL unavailable (geo-restriction or rights issue)
    #[error("Stream unavailable: {0}")]
    StreamUnavailable(String),

    /// Operation cancelled
    #[error("Operation cancelled")]
    Cancelled,

    /// Generic/unknown error
    #[error("{0}")]
    Other(String),
}

impl QbzError {
    /// Check if this error is recoverable (user can retry or fix)
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            QbzError::NetworkError(_)
                | QbzError::NotAuthenticated
                | QbzError::StreamUnavailable(_)
                | QbzError::Cancelled
        )
    }

    /// Get an error code for frontend handling
    pub fn code(&self) -> &'static str {
        match self {
            QbzError::AuthError(_) => "AUTH_ERROR",
            QbzError::ApiError(_) => "API_ERROR",
            QbzError::NetworkError(_) => "NETWORK_ERROR",
            QbzError::PlaybackError(_) => "PLAYBACK_ERROR",
            QbzError::AudioDeviceError(_) => "AUDIO_DEVICE_ERROR",
            QbzError::QueueError(_) => "QUEUE_ERROR",
            QbzError::ConfigError(_) => "CONFIG_ERROR",
            QbzError::StorageError(_) => "STORAGE_ERROR",
            QbzError::TrackNotFound(_) => "TRACK_NOT_FOUND",
            QbzError::AlbumNotFound(_) => "ALBUM_NOT_FOUND",
            QbzError::ArtistNotFound(_) => "ARTIST_NOT_FOUND",
            QbzError::PlaylistNotFound(_) => "PLAYLIST_NOT_FOUND",
            QbzError::NotAuthenticated => "NOT_AUTHENTICATED",
            QbzError::StreamUnavailable(_) => "STREAM_UNAVAILABLE",
            QbzError::Cancelled => "CANCELLED",
            QbzError::Other(_) => "UNKNOWN_ERROR",
        }
    }
}

/// Result type alias using QbzError
pub type QbzResult<T> = Result<T, QbzError>;
