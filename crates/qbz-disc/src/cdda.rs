//! Reading a CD, whichever way this platform offers.
//!
//! ONE set of names — `list_devices`, `read_toc`, `TrackReader`, `read_audio`,
//! `drive_model` — over two implementations that share no code at all:
//!
//!   * **Linux** ([`crate::cdda_linux`]) talks to the drive through the
//!     kernel's ioctls and reads raw sectors.
//!   * **macOS** ([`crate::cdda_macos`]) does not: the OS mounts the disc and
//!     exposes one AIFF per track, so the "read" is a file read.
//!
//! This facade exists so nothing above it branches on the platform. `qbz-rip`,
//! `audible_qt` and `cdda_qt` were written against the Linux names and needed
//! no change when macOS arrived, which is the test of whether a seam is in the
//! right place.
//!
//! The two readers agree on the ANSWER even though they agree on nothing else:
//! measured on the same disc the same afternoon, both produce DiscID
//! `BeNBMsD8Du5NO2W61Yk.B2jwwIs-` and fingerprint `38bef21351f7fca3` (see
//! [`crate::toc`]).
//!
//! ON EVERY OTHER PLATFORM the calls are present and report "no drive". A
//! Windows build must COMPILE — the disc feature is one view of many — and the
//! honest failure is an empty drive list, not a crate that will not build.

pub use crate::toc::{CdError, Toc, TocTrack};

/// The kernel accepts 1..=75 frames per `CDROMREADAUDIO` call; the macOS
/// reader uses the same chunk size so both platforms hand callers the same
/// shape of buffer.
pub const MAX_FRAMES_PER_READ: u32 = 75;

#[cfg(target_os = "linux")]
pub use crate::cdda_linux::{
    drive_model, list_devices, looks_byte_swapped, read_audio, read_toc, TrackReader,
};

#[cfg(target_os = "macos")]
pub use crate::cdda_macos::{drive_model, list_devices, read_audio, read_toc, TrackReader};

/// The byte-order diagnostic is Linux's — it exists because a DRIVE can hand
/// back either order and this crate measured which. On macOS the OS produced
/// the file, AIFF's order is defined by the format, and there is nothing to
/// sniff; the reader normalises and the question does not arise.
#[cfg(not(target_os = "linux"))]
pub fn looks_byte_swapped(_pcm: &[u8]) -> bool {
    false
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported {
    use super::*;
    use std::path::{Path, PathBuf};

    pub fn list_devices() -> Vec<PathBuf> {
        Vec::new()
    }
    pub fn drive_model(_dev: &Path) -> Option<String> {
        None
    }
    pub fn read_toc(_dev: &Path) -> Result<Toc, CdError> {
        Err(CdError::NoDrive)
    }
    pub fn read_audio(
        _dev: &Path,
        _lsn: u32,
        _frames: u32,
        _out: &mut Vec<u8>,
    ) -> Result<(), CdError> {
        Err(CdError::NoDrive)
    }

    /// Present so callers compile; every construction fails.
    pub struct TrackReader(());
    impl TrackReader {
        pub fn open(_dev: &Path, _track: &TocTrack) -> Result<Self, CdError> {
            Err(CdError::NoDrive)
        }
        pub fn next_chunk(&mut self, _out: &mut Vec<u8>) -> Result<usize, CdError> {
            Err(CdError::NoDrive)
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use unsupported::{drive_model, list_devices, read_audio, read_toc, TrackReader};
