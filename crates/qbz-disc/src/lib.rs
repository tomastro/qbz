//! Optical media QBZ can PLAY without indexing any of it.
//!
//! Frontend-agnostic (ADR-006): no UI, no player, no Qt. The crate hands back
//! a track list and raw bytes; who plays them, and how, is the frontend's
//! problem.
//!
//! Deliberately does NOT depend on `qbz-dsd`. The dependency runs the other
//! way — `qbz-dsd` adapts this crate's SACD reader to its `DsdDemuxer` trait —
//! because the reverse closes a cycle.
//!
//! Phase 1 covers CD-DA from a real drive. What is not supported is DETECTED
//! and REPORTED, never silently degraded: a data track in a mixed-mode disc is
//! skipped by name, an unreadable sector is an error rather than a hole full
//! of zeros. Silence that pretends to be audio is the one outcome a
//! bit-perfect player must never produce.

/// What a table of contents IS — shared by every platform's reader.
pub mod toc;

/// Reading a CD, whichever way this platform offers. Always present: the
/// answer on a platform with no reader is "no drive", never a build failure.
pub mod cdda;

#[cfg(target_os = "linux")]
mod cdda_linux;
#[cfg(target_os = "macos")]
mod cdda_macos;

/// Disc IMAGES are portable — a .iso is a file, and reading one needs no
/// device and no privileges. Only the live-drive path is Linux-gated.
pub mod iso9660;

/// Naming a disc that carries no names.
pub mod discid;

/// SACD disc images (Scarlet Book).
pub mod sacd;

/// What a disc is called, remembered after it has been ejected.
pub mod store;

pub use cdda::{drive_model, list_devices, read_audio, read_toc};
pub use toc::{CdError, Toc, TocTrack};

/// Bytes per CD-DA sector (a "frame" in the kernel's vocabulary): 588 stereo
/// 16-bit sample pairs.
pub const CDDA_SECTOR_BYTES: usize = 2352;
/// Sectors per second of CD-DA audio.
pub const CDDA_SECTORS_PER_SEC: u32 = 75;
/// CD-DA is 44 100 Hz, 16-bit, stereo. These are not configurable and any code
/// that treats them as such has misunderstood the format.
pub const CDDA_SAMPLE_RATE: u32 = 44_100;
pub const CDDA_BITS: u16 = 16;
pub const CDDA_CHANNELS: u16 = 2;

/// Seconds of audio in `sectors`, rounded down.
pub fn sectors_to_secs(sectors: u32) -> u64 {
    (sectors / CDDA_SECTORS_PER_SEC) as u64
}

/// A 44-byte RIFF/WAVE header for 16-BIT PCM.
///
/// `qbz-dsd` has a `wav_header` already and it is hardcoded to 24-bit — the
/// depth its converter emits. Reusing it for a CD would mean either lying in
/// the header or promoting 16-bit samples to 24, and while that shift is
/// arithmetically lossless it makes the DAC open at 24 bits for a 16-bit
/// source. That is exactly the silent conversion a bit-perfect path must not
/// contain, so a CD gets its own header.
pub fn wav_header_16(total_frames: u64, channels: u16, sample_rate: u32) -> Vec<u8> {
    const HEADER_LEN: usize = 44;
    let bytes_per_sample = 2u64;
    let data_len = (total_frames * channels as u64 * bytes_per_sample) as u32;
    let byte_rate = sample_rate * channels as u32 * bytes_per_sample as u32;
    let block_align = channels * bytes_per_sample as u16;
    let mut h = Vec::with_capacity(HEADER_LEN);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data_len).to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&sample_rate.to_le_bytes());
    h.extend_from_slice(&byte_rate.to_le_bytes());
    h.extend_from_slice(&block_align.to_le_bytes());
    h.extend_from_slice(&CDDA_BITS.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    debug_assert_eq!(h.len(), HEADER_LEN);
    h
}

/// Total size of the WAV a track becomes — what the streaming player needs as
/// its `content_length`.
pub fn wav_total_size_16(total_frames: u64, channels: u16) -> u64 {
    44 + total_frames * channels as u64 * 2
}

/// A CD-DA track, encoded as ONE string so it can ride in `LocalTrack.file_path`
/// and through every store and queue that carries a path today.
///
/// `cdda:/dev/sr0#0+46577` — device, start sector, sector count.
///
/// It is a real type with a single `parse`/`to_path_string` pair rather than
/// ad-hoc `starts_with` tests scattered around, because the scattered version
/// is a known failure mode in this codebase: a second list of prefixes always
/// ends up missing one site, and the fold is silent (see the six "a place that
/// enumerates sources by hand" bugs of 2026-08-20). One parser, one shape.
///
/// It is NOT a filesystem path and nothing may stat, open or canonicalise it.
/// Callers test with [`CdRef::is_cd_path`] BEFORE reaching for the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdRef {
    pub device: std::path::PathBuf,
    pub start_lsn: u32,
    pub sectors: u32,
}

impl CdRef {
    pub const SCHEME: &'static str = "cdda:";

    /// Cheap test callers use to route BEFORE touching the filesystem.
    pub fn is_cd_path(s: &str) -> bool {
        s.starts_with(Self::SCHEME)
    }

    pub fn to_path_string(&self) -> String {
        format!(
            "{}{}#{}+{}",
            Self::SCHEME,
            self.device.display(),
            self.start_lsn,
            self.sectors
        )
    }

    pub fn parse(s: &str) -> Option<Self> {
        let rest = s.strip_prefix(Self::SCHEME)?;
        let (dev, range) = rest.rsplit_once('#')?;
        let (lsn, sectors) = range.split_once('+')?;
        if dev.is_empty() {
            return None;
        }
        Some(Self {
            device: std::path::PathBuf::from(dev),
            start_lsn: lsn.parse().ok()?,
            sectors: sectors.parse().ok()?,
        })
    }
}

/// A SACD track inside an image, as ONE string that rides in a
/// `LocalTrack.file_path` — the same trick `CdRef` plays, for the same reason.
///
/// `sacd:/path/to/disc.iso#7`
///
/// The track NUMBER, not its sector range: the area TOC is the authority on
/// extents and re-reading it costs one 18 KB read, so encoding the geometry
/// here would only create a second copy that can go stale. The image path may
/// contain `#` (a filename is user data), so the split is from the RIGHT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SacdRef {
    pub image: std::path::PathBuf,
    pub track: u8,
}

impl SacdRef {
    pub const SCHEME: &'static str = "sacd:";

    /// Cheap test callers use to route BEFORE touching the filesystem — an
    /// image path IS a real file, but `open_dsd` must not try to sniff DSF
    /// magic at its first byte.
    pub fn is_sacd_path(s: &str) -> bool {
        s.starts_with(Self::SCHEME)
    }

    pub fn to_path_string(&self) -> String {
        format!("{}{}#{}", Self::SCHEME, self.image.display(), self.track)
    }

    pub fn parse(s: &str) -> Option<Self> {
        let rest = s.strip_prefix(Self::SCHEME)?;
        let (image, track) = rest.rsplit_once('#')?;
        if image.is_empty() {
            return None;
        }
        Some(Self {
            image: std::path::PathBuf::from(image),
            track: track.parse().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_describes_a_cd_and_not_a_dsd_conversion() {
        // 10 seconds of CD-DA.
        let frames = 44_100u64 * 10;
        let h = wav_header_16(frames, 2, 44_100);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..12], b"WAVE");
        // bits per sample at offset 34 — 16, never 24.
        assert_eq!(u16::from_le_bytes([h[34], h[35]]), 16);
        assert_eq!(u32::from_le_bytes([h[24], h[25], h[26], h[27]]), 44_100);
        // byte rate = 44100 * 2ch * 2 bytes
        assert_eq!(u32::from_le_bytes([h[28], h[29], h[30], h[31]]), 176_400);
        // and the sizes agree with the standalone helper
        assert_eq!(
            wav_total_size_16(frames, 2),
            44 + u32::from_le_bytes([h[40], h[41], h[42], h[43]]) as u64
        );
    }

    #[test]
    fn a_measured_track_length_converts_to_its_real_duration() {
        // Tool — Fear Inoculum, measured off the owner's disc 2026-08-20:
        // track 1 spans 46577 sectors and track 7 spans 70833.
        assert_eq!(sectors_to_secs(46_577), 621); // 10:21
        assert_eq!(sectors_to_secs(70_833), 944); // 15:44
    }

    #[test]
    fn a_cd_reference_round_trips() {
        let r = CdRef {
            device: std::path::PathBuf::from("/dev/sr0"),
            start_lsn: 285_735,
            sectors: 70_833,
        };
        let s = r.to_path_string();
        assert_eq!(s, "cdda:/dev/sr0#285735+70833");
        assert_eq!(CdRef::parse(&s), Some(r));
        assert!(CdRef::is_cd_path(&s));
    }

    #[test]
    fn a_real_path_is_never_mistaken_for_a_disc() {
        // The whole point of the scheme: no filesystem path can collide with
        // it, so a routing test cannot send a real file down the disc path.
        for p in [
            "/home/u/Music/a.flac",
            "/home/u/cdda:weird/x.dsf",
            "plex:1234",
            "",
        ] {
            assert!(!CdRef::is_cd_path(p), "{p} was taken for a disc");
            assert_eq!(CdRef::parse(p), None);
        }
    }

    #[test]
    fn a_malformed_reference_is_rejected_rather_than_guessed() {
        for bad in [
            "cdda:",
            "cdda:/dev/sr0",
            "cdda:/dev/sr0#",
            "cdda:/dev/sr0#12",
            "cdda:/dev/sr0#12+",
            "cdda:/dev/sr0#a+b",
            "cdda:#0+1",
        ] {
            assert_eq!(CdRef::parse(bad), None, "{bad} parsed");
        }
    }

    #[test]
    fn a_sacd_reference_round_trips_even_with_a_hash_in_the_name() {
        // A filename is user data and may contain '#', which is why the split
        // is from the right rather than the left.
        let r = SacdRef {
            image: std::path::PathBuf::from("/m/Wagner #1/disc.iso"),
            track: 7,
        };
        let s = r.to_path_string();
        assert_eq!(s, "sacd:/m/Wagner #1/disc.iso#7");
        assert_eq!(SacdRef::parse(&s), Some(r));
        assert!(SacdRef::is_sacd_path(&s));
    }

    #[test]
    fn the_two_disc_schemes_never_claim_each_other() {
        let cd = "cdda:/dev/sr0#0+46577";
        let sacd = "sacd:/m/d.iso#3";
        assert!(CdRef::is_cd_path(cd) && !SacdRef::is_sacd_path(cd));
        assert!(SacdRef::is_sacd_path(sacd) && !CdRef::is_cd_path(sacd));
        assert_eq!(CdRef::parse(sacd), None);
        assert_eq!(SacdRef::parse(cd), None);
    }

    #[test]
    fn a_malformed_sacd_reference_is_rejected() {
        for bad in ["sacd:", "sacd:/x.iso", "sacd:#3", "sacd:/x.iso#", "sacd:/x.iso#abc"] {
            assert_eq!(SacdRef::parse(bad), None, "{bad} parsed");
        }
    }
}
