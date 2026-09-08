//! DSD (Direct Stream Digital) support: DSF/DFF demuxing and streaming
//! DSD-to-PCM conversion.
//!
//! Phase 1 scope (see qbz-nix-docs/dsd-support/): local .dsf/.dff files are
//! demuxed here and converted on the fly to 176.4 kHz / 24-bit PCM, which the
//! player delivers through its existing (bit-perfect-capable) PCM pipeline.
//! DoP and native-DSD delivery are later phases and intentionally absent.
//!
//! Frontend-agnostic (ADR-006): no UI, no player, no Tauri types.

mod convert;
mod demux;
mod dop;
mod dsd2pcm;
mod native;
mod sacd_source;
mod wav;

pub use convert::{DsdPcmConverter, DEFAULT_GAIN_DB, OUTPUT_RATE};
pub use demux::{open_dsd, DsdDemuxer, DsdError, DsdStreamInfo, DsdTags};
pub use dop::{dop_carrier_rate, DopPacker, DopStream};
pub use native::{native_u32_rate, NativeDsdStream, NATIVE_DSD_SILENCE_U32};
pub use sacd_source::{ChannelLayout, SacdDemuxer};
pub use wav::{frames_to_pcm24, wav_header, wav_total_size};

/// Common surface of the boxed DSD word streams (DoP and native): an
/// `Iterator<Item = i32>` that can also report a mid-stream demux I/O
/// error once it ends, since the iterator surface itself can only express
/// "no more items" and would otherwise make a read failure look like a
/// clean end of track.
pub trait DsdWordSource: Iterator<Item = i32> + Send {
    /// Mid-stream demux I/O error, if any. Clean EOF leaves this `None`.
    fn io_error(&self) -> Option<&str>;
}

impl DsdWordSource for DopStream {
    fn io_error(&self) -> Option<&str> {
        DopStream::io_error(self)
    }
}

impl DsdWordSource for NativeDsdStream {
    fn io_error(&self) -> Option<&str> {
        NativeDsdStream::io_error(self)
    }
}

/// DSD64 base rate (bits per second per channel). Multiples of this are the
/// only valid DSD rates: DSD64/128/256/512.
pub const DSD64_RATE: u32 = 2_822_400;

/// True when the path has a DSD container extension (.dsf / .dff).
pub fn is_dsd_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref(),
        Some("dsf") | Some("dff")
    )
}

/// "DSD64" / "DSD128" / … label for a DSD bit rate. Falls back to "DSD" for
/// non-standard rates.
pub fn dsd_label(dsd_rate: u32) -> String {
    if dsd_rate >= DSD64_RATE && dsd_rate % DSD64_RATE == 0 {
        format!("DSD{}", 64 * (dsd_rate / DSD64_RATE))
    } else {
        "DSD".to_string()
    }
}
