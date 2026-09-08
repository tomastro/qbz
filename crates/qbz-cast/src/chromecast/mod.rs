//! Chromecast casting support
//!
//! Provides Chromecast device discovery, connection management,
//! and media control via the Cast protocol.

pub mod device;
pub mod discovery;
pub mod thread;

pub use device::{
    CastApplication, CastDeviceConnection, CastPositionInfo, CastStatus, MediaMetadata,
};
pub use discovery::{DeviceDiscovery, DiscoveredDevice};
pub use thread::{CastCommand, ChromecastHandle};
