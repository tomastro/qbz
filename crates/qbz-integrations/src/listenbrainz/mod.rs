//! ListenBrainz integration
//!
//! Provides scrobbling and now-playing notifications to ListenBrainz.
//! Uses personal user tokens (not OAuth) for authentication.
//!
//! ## Scrobbling Rules
//!
//! - Now Playing: Submitted when track starts
//! - Scrobble: Submitted after 50% of track OR 4 minutes played
//! - Skip: No submission if < 30 seconds played
//!
//! ## Usage
//!
//! ```no_run
//! use qbz_integrations::{ListenBrainzClient, ListenBrainzConfig};
//!
//! async fn example() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = ListenBrainzConfig {
//!         enabled: true,
//!         token: Some("your-user-token".to_string()),
//!         user_name: Some("username".to_string()),
//!     };
//!
//!     let client = ListenBrainzClient::with_config(config);
//!
//!     // Now playing (no timestamp)
//!     client
//!         .submit_playing_now("Artist", "Track", Some("Album"), None)
//!         .await?;
//!
//!     // Scrobble after the track finishes
//!     client
//!         .submit_listen("Artist", "Track", Some("Album"), 1_700_000_000, None)
//!         .await?;
//!
//!     Ok(())
//! }
//! ```

mod client;
mod models;

#[cfg(feature = "cache")]
pub mod cache;

pub use client::{ListenBrainzClient, ListenBrainzConfig};
pub use models::{
    AdditionalInfo, CfRecommendation, LbFreshRelease, LbListen, LbPlaylistMeta, LbPlaylistTrack,
    LbRecordingMeta, Listen, ListenBrainzStatus, ListenType, QueuedListen, SubmitListensPayload,
    TrackMetadata, UserInfo,
};
