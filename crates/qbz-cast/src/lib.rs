//! qbz-cast - Casting support for QBZ
//!
//! Provides casting capabilities for Chromecast and DLNA devices.
//! This crate is Tauri-agnostic and can be used by any Rust application.
//!
//! # Architecture
//!
//! - **Chromecast**: Full implementation using rust-cast. Uses a dedicated thread
//!   for connection management due to rust-cast's use of Rc (not thread-safe).
//!
//! - **DLNA/UPnP**: Full implementation using rupnp for AVTransport/RenderingControl.
//!
//! - **MediaServer**: Local HTTP server for streaming audio to cast devices.
//!   Supports byte-range requests for seeking.

pub mod chromecast;
pub mod dlna;
pub mod errors;
pub mod media_server;

// Re-export error types at root
pub use errors::{CastError, DlnaError};

// Re-export media server
pub use media_server::{MediaServer, RangeSource};

// Re-export Chromecast types
pub use chromecast::{
    CastApplication, CastCommand, CastDeviceConnection, CastPositionInfo, CastStatus,
    ChromecastHandle, DeviceDiscovery, DiscoveredDevice, MediaMetadata,
};

// Re-export DLNA types
pub use dlna::{
    DiscoveredDlnaDevice, DlnaConnection, DlnaDiscovery, DlnaMetadata, DlnaPositionInfo, DlnaStatus,
};

/// Cast device type alias for backwards compatibility
pub type CastDevice = CastDeviceConnection;

