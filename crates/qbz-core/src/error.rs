//! Error types for qbz-core

use thiserror::Error;

/// Core error type
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("Authentication required")]
    AuthRequired,

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("API error: {0}")]
    Api(#[from] qbz_qobuz::ApiError),

    #[error("Player error: {0}")]
    Player(String),

    #[error("Playback error: {0}")]
    Playback(String),

    #[error("Audio error: {0}")]
    Audio(String),

    #[error("Queue error: {0}")]
    Queue(String),

    #[error("Not initialized")]
    NotInitialized,

    #[error("Internal error: {0}")]
    Internal(String),
}
