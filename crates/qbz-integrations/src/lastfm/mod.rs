//! Last.fm integration
//!
//! Handles Last.fm authentication and scrobbling via Cloudflare Workers proxy.
//! The proxy handles API credentials and signature generation, so this module
//! only needs to manage session keys.
//!
//! ## Usage
//!
//! ```no_run
//! use qbz_integrations::LastFmClient;
//!
//! async fn example() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut client = LastFmClient::new();
//!
//!     // Step 1: Get authorization URL
//!     let (token, auth_url) = client.get_token().await?;
//!     println!("Authorize at: {}", auth_url);
//!
//!     // Step 2: After user authorizes, get session
//!     let session = client.get_session(&token).await?;
//!     println!("Logged in as: {}", session.name);
//!
//!     // Step 3: Scrobble tracks
//!     client.scrobble("Artist", "Track", Some("Album"), 1234567890).await?;
//!
//!     Ok(())
//! }
//! ```

mod client;
mod models;

pub use client::LastFmClient;
pub use models::{
    LastFmAlbum, LastFmArtist, LastFmSession, LastFmSimilarArtist, LastFmSimilarTrack, LastFmTrack,
};
