//! QBZ Core - Orchestrator for QBZ music player
//!
//! This crate provides the main entry point for QBZ:
//! - [`QbzCore<A: FrontendAdapter>`]: Main orchestrator struct
//! - Connects all subsystems (audio, player, queue, API)
//! - Provides unified public API for frontends
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      qbz-core (Tier 3)                      │
//! │         Main orchestrator, entry point for frontends        │
//! └─────────────────────────────────────────────────────────────┘
//!                              ↑
//!          ┌───────────────────┼───────────────────┐
//!          ↓                   ↓                   ↓
//! ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
//! │   qbz-player    │ │   qbz-audio     │ │   qbz-qobuz     │
//! │    (Tier 2)     │ │    (Tier 1)     │ │    (Tier 2)     │
//! └─────────────────┘ └─────────────────┘ └─────────────────┘
//!          ↓                   ↓                   ↓
//!                      ┌───────────────┐
//!                      │  qbz-models   │
//!                      │   (Tier 0)    │
//!                      └───────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use qbz_core::{QbzCore, CoreError};
//! use qbz_models::{FrontendAdapter, CoreEvent};
//!
//! struct MyAdapter;
//!
//! #[async_trait::async_trait]
//! impl FrontendAdapter for MyAdapter {
//!     async fn on_event(&self, event: CoreEvent) {
//!         // Handle event, update UI
//!         println!("Event: {:?}", event);
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), CoreError> {
//!     let core = QbzCore::new(MyAdapter);
//!     core.init().await?;
//!     core.login("email", "password").await?;
//!     // ... use core
//!     Ok(())
//! }
//! ```

pub mod core;
pub mod error;
pub mod offline_resolve;
pub mod system_capabilities;

// Re-exports from qbz-models for convenience
pub use qbz_models::{CoreEvent, FrontendAdapter, LoggingAdapter, NoOpAdapter};

// Re-exports from this crate
pub use core::{normalize_artist_name, QbzCore, QueueAuthoritySnapshot};
// Scene discovery ("artists from the same place"). A frontend driving it needs
// all four: the token to cancel a run it has navigated away from, the phase for
// the progress label, and the typed error to tell "offline" and "the upstreams
// all failed" apart from an honest empty result — which the untyped
// `CoreError` cannot express and which the view's error state depends on.
pub use core::{CancelToken, ScenePhase, SceneDiscoveryError};
pub use error::CoreError;
