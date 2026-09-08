//! QBZ Player - Playback engine and queue management
//!
//! This crate provides:
//! - QueueManager: Track queue management with shuffle/repeat
//! - Player: Main playback engine
//! - StreamingSource: HTTP audio streaming
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                     qbz-player (Tier 2)                     │
//! │  Queue management, playback engine, streaming              │
//! └─────────────────────────────────────────────────────────────┘
//!                              ↑
//!              ┌───────────────┼───────────────┐
//!              │               │               │
//!         ┌────┴────┐    ┌─────┴─────┐   ┌─────┴─────┐
//!         │qbz-audio│    │qbz-models │   │qbz-qobuz  │
//!         │ Tier 1  │    │  Tier 0   │   │  Tier 1   │
//!         └─────────┘    └───────────┘   └───────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use qbz_player::{Player, QueueManager};
//! use qbz_audio::{AudioSettings, AudioDiagnostic};
//!
//! let player = Player::new(None, AudioSettings::default(), None, AudioDiagnostic::new());
//! let queue = QueueManager::new();
//! ```

pub mod player;
pub mod remote_stream;
pub mod queue;

// Re-export main types
pub use player::{
    BufferWriter, BufferedMediaSource, ExternalStreamHandle, IncrementalStreamingSource,
    InMemorySource, PlaybackBufferState, PlaybackEvent, PlaybackState,
    Player, SharedState, StreamingConfig,
};
pub use queue::{QueueAuthoritySnapshot, QueueManager};
