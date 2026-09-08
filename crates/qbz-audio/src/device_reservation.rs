//! D-Bus device reservation via org.freedesktop.ReserveDevice1.
//!
//! Acquires the bus name org.freedesktop.ReserveDevice1.Audio<N> for the
//! card index N, signalling to PulseAudio/PipeWire/WirePlumber that another
//! application owns the device exclusively. Released on Drop.
//!
//! See `qbz-nix-docs/specs/2026-05-07-alsa-exclusive-hardening-design.md`
//! for the full protocol specification and lifetime model.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(not(target_os = "linux"))]
mod stub;
#[cfg(not(target_os = "linux"))]
pub use stub::*;
