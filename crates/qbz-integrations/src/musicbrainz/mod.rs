//! MusicBrainz integration
//!
//! Provides entity resolution and metadata enrichment from MusicBrainz.
//! This module operates as a background semantic engine - it enriches
//! data without affecting the UI directly (Stage 1).
//!
//! ## Rate Limiting
//!
//! MusicBrainz has strict rate limits (1 req/sec). This module uses
//! a Cloudflare Workers proxy for better rate limit handling.
//!
//! ## Usage
//!
//! ```no_run
//! use qbz_integrations::{MusicBrainzClient, MusicBrainzConfig};
//!
//! async fn example() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = MusicBrainzConfig::default();
//!     let client = MusicBrainzClient::with_config(config);
//!
//!     // Search by ISRC
//!     let recording = client.search_recording_by_isrc("USRC17607839").await?;
//!
//!     Ok(())
//! }
//! ```

mod client;
mod models;

#[cfg(feature = "cache")]
pub mod cache;

pub mod genre;
pub mod location;

pub use client::{MusicBrainzClient, MusicBrainzConfig};
pub use models::*;
