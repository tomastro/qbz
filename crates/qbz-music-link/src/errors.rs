//! Error types for the music-link resolver.
//!
//! Replaces the Tauri-only `RuntimeError` used by the `src-tauri` original.

use thiserror::Error;

/// Errors that can occur while resolving a cross-platform music link.
#[derive(Debug, Error)]
pub enum MusicLinkError {
    /// The input URL was empty after trimming.
    #[error("Empty URL")]
    EmptyUrl,

    /// The URL did not match any supported platform.
    #[error("Unsupported or invalid music link")]
    Unsupported,

    /// A generic internal/resolver error (network, Odesli, search, etc.).
    #[error("{0}")]
    Internal(String),
}

impl serde::Serialize for MusicLinkError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
