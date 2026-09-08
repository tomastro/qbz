//! Core traits for QBZ
//!
//! This module defines the key traits that enable multiple frontends:
//! - FrontendAdapter: Receives events from the core
//! - Additional interface traits as needed

use async_trait::async_trait;

use crate::events::CoreEvent;

/// Adapter trait that frontends implement to receive events from QBZ core.
///
/// This is the primary integration point for any frontend (Tauri, Slint, Iced, etc.).
/// The core calls `on_event` whenever something happens that the frontend should know about.
///
/// # Example
///
/// ```rust,ignore
/// use qbz_models::{FrontendAdapter, CoreEvent};
///
/// struct TauriAdapter {
///     app_handle: tauri::AppHandle,
/// }
///
/// #[async_trait]
/// impl FrontendAdapter for TauriAdapter {
///     async fn on_event(&self, event: CoreEvent) {
///         // Emit event to Tauri webview
///         self.app_handle.emit("core-event", event).ok();
///     }
/// }
/// ```
#[async_trait]
pub trait FrontendAdapter: Send + Sync {
    /// Called when the core emits an event.
    ///
    /// Implementations should handle this efficiently (non-blocking) since
    /// events may be emitted frequently (e.g., position updates every second).
    async fn on_event(&self, event: CoreEvent);

    /// Optional: Called when the core is ready for interaction.
    /// Default implementation does nothing.
    async fn on_ready(&self) {}

    /// Optional: Called when the core is shutting down.
    /// Default implementation does nothing.
    async fn on_shutdown(&self) {}
}

/// A no-op adapter for testing or headless operation
pub struct NoOpAdapter;

#[async_trait]
impl FrontendAdapter for NoOpAdapter {
    async fn on_event(&self, _event: CoreEvent) {
        // Intentionally empty - events are discarded
    }
}

/// An adapter that logs all events (useful for debugging)
pub struct LoggingAdapter {
    prefix: String,
}

impl LoggingAdapter {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

impl Default for LoggingAdapter {
    fn default() -> Self {
        Self::new("CoreEvent")
    }
}

#[async_trait]
impl FrontendAdapter for LoggingAdapter {
    async fn on_event(&self, event: CoreEvent) {
        log::debug!("{}: {:?}", self.prefix, event);
    }

    async fn on_ready(&self) {
        log::info!("{}: Core is ready", self.prefix);
    }

    async fn on_shutdown(&self) {
        log::info!("{}: Core is shutting down", self.prefix);
    }
}
