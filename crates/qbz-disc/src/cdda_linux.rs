//! CD-DA off a real drive on LINUX, through the kernel's own ioctls.
//!
//! No libcdio. Not because libcdio is bad — it is better at RIPPING than this
//! will ever be — but because this path PLAYS a clean disc, and for that the
//! kernel interface is enough while a native library would have to be built
//! into Flatpak, Snap and AppImage. Both approaches need the same `--device`
//! permission in a sandbox; only one needs the library shipped.
//!
//! WHAT THIS IS NOT: accurate extraction. There is no overlap synchronisation,
//! no cache modelling, no jitter verification, no scratch reconstruction. On a
//! clean disc that is fine for playback; on a marginal one it can click, and
//! an unreadable sector surfaces as an ERROR rather than a hole full of zeros.
//! Silence that pretends to be audio is the one outcome bit-perfect playback
//! must never produce.
//!
//! MEASURED on the owner's USB drive with Tool — *Fear Inoculum* (2026-08-20),
//! before any of this existed:
//!   - `cd-info` reports access mode IOCTL; the TOC reads back 7 audio tracks,
//!     lead-out at LSN 356568;
//!   - `CDROMREADAUDIO` over 20 frames from LSN 4000 returned 47 040 bytes,
//!     99 % non-zero samples, peak 14559 — real audio, not silence;
//!   - the drive hands back LITTLE-endian samples. Neighbouring samples差
//!     average 73.5 read as LE against 20 971 read as BE, a 285x margin. So
//!     the bytes go straight into a little-endian WAV with no swap. That is
//!     measured for THIS drive; `looks_byte_swapped` below keeps the check
//!     available as a diagnostic rather than as an article of faith.

use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use crate::toc::{CdError, Toc, TocTrack};

// --- ioctl numbers and layouts (uapi/linux/cdrom.h) -------------------------
const CDROMREADTOCHDR: libc::c_ulong = 0x5305;
const CDROMREADTOCENTRY: libc::c_ulong = 0x5306;
const CDROMREADAUDIO: libc::c_ulong = 0x530e;
const CDROM_DRIVE_STATUS: libc::c_ulong = 0x5326;

const CDROM_LBA: u8 = 0x01;
/// The lead-out's pseudo track number: its start is the end of the last track.
const CDROM_LEADOUT: u8 = 0xAA;
const CDS_DISC_OK: libc::c_int = 4;
/// `cdte_ctrl` bit 2 marks a DATA track. A mixed-mode disc carries one and it
/// must be skipped: handing its bytes to an audio path produces noise.
const CTRL_DATA: u8 = 0x04;
/// The kernel accepts 1..=75 frames per `CDROMREADAUDIO` call.
pub const MAX_FRAMES_PER_READ: u32 = 75;

#[repr(C)]
#[derive(Default)]
struct CdromTocHdr {
    trk0: u8,
    trk1: u8,
}

#[repr(C)]
#[derive(Default)]
struct CdromTocEntry {
    track: u8,
    /// adr in the low nibble, ctrl in the high one — a C bitfield, read whole.
    adr_ctrl: u8,
    format: u8,
    _pad: u8,
    addr_lba: i32,
    datamode: u8,
    _pad2: [u8; 3],
}

#[repr(C)]
struct CdromReadAudio {
    addr_lba: i32,
    addr_format: u8,
    _pad: [u8; 3],
    nframes: libc::c_int,
    buf: *mut u8,
}

/// Optical devices the kernel is showing, most conventional first.
///
/// `/dev/sr0` is NOT hardcoded anywhere in this crate: a box can have several
/// drives, and `/dev/cdrom` is a udev symlink that may point at any of them.
pub fn list_devices() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for n in 0..8 {
        let p = PathBuf::from(format!("/dev/sr{n}"));
        if p.exists() {
            out.push(p);
        }
    }
    if out.is_empty() {
        let alt = PathBuf::from("/dev/cdrom");
        if alt.exists() {
            out.push(alt);
        }
    }
    out
}

/// "HL-DT-ST BD-RE BU40N" — the drive as the kernel names it, or `None`.
///
/// A rip log without the drive in it cannot answer the question people
/// actually ask of one two years later ("which reader produced this?"), and
/// the answer is sitting in sysfs. Best-effort by design: a missing file, a
/// container with no `/sys`, or a device name that is a symlink somewhere
/// unexpected all mean "unknown", never an error.
pub fn drive_model(dev: &Path) -> Option<String> {
    // `/dev/cdrom` is a symlink; resolve it or the sysfs name is wrong.
    let real = std::fs::canonicalize(dev).unwrap_or_else(|_| dev.to_path_buf());
    let name = real.file_name()?.to_string_lossy().into_owned();
    let base = PathBuf::from("/sys/block").join(&name).join("device");
    let read = |f: &str| {
        std::fs::read_to_string(base.join(f))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    match (read("vendor"), read("model")) {
        (Some(v), Some(m)) => Some(format!("{v} {m}")),
        (None, Some(m)) => Some(m),
        (Some(v), None) => Some(v),
        (None, None) => None,
    }
}

fn open_device(dev: &Path) -> Result<OwnedFd, CdError> {
    // O_NONBLOCK is required, not optional: without it `open` on an optical
    // device BLOCKS until a disc is loaded and spun up, which on an empty tray
    // means hanging forever.
    let c = std::ffi::CString::new(dev.as_os_str().as_encoded_bytes())
        .map_err(|_| CdError::Open(dev.to_path_buf(), std::io::Error::other("bad path")))?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
    if fd < 0 {
        return Err(CdError::Open(
            dev.to_path_buf(),
            std::io::Error::last_os_error(),
        ));
    }
    Ok(unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) })
}

/// Read the table of contents, skipping data tracks by their control bits.
pub fn read_toc(dev: &Path) -> Result<Toc, CdError> {
    let fd = open_device(dev)?;
    let raw = fd.as_raw_fd();

    let status = unsafe { libc::ioctl(raw, CDROM_DRIVE_STATUS, 0) };
    if status != CDS_DISC_OK {
        return Err(CdError::NoDisc);
    }

    let mut hdr = CdromTocHdr::default();
    if unsafe { libc::ioctl(raw, CDROMREADTOCHDR, &mut hdr) } < 0 {
        return Err(CdError::Toc(std::io::Error::last_os_error()));
    }

    let entry_lba = |n: u8| -> Result<(u32, u8), CdError> {
        let mut e = CdromTocEntry {
            track: n,
            format: CDROM_LBA,
            ..Default::default()
        };
        if unsafe { libc::ioctl(raw, CDROMREADTOCENTRY, &mut e) } < 0 {
            return Err(CdError::Toc(std::io::Error::last_os_error()));
        }
        Ok((e.addr_lba.max(0) as u32, e.adr_ctrl >> 4))
    };

    let (leadout_lsn, _) = entry_lba(CDROM_LEADOUT)?;

    let mut raw_tracks: Vec<(u8, u32, bool)> = Vec::new();
    for n in hdr.trk0..=hdr.trk1 {
        let (lba, ctrl) = entry_lba(n)?;
        raw_tracks.push((n, lba, ctrl & CTRL_DATA == 0));
    }

    // A track's length is the gap to whatever starts next — the following
    // track, or the lead-out for the last one. The TOC does not carry lengths.
    let mut tracks = Vec::with_capacity(raw_tracks.len());
    for (i, &(number, start_lsn, is_audio)) in raw_tracks.iter().enumerate() {
        let next = raw_tracks
            .get(i + 1)
            .map(|t| t.1)
            .unwrap_or(leadout_lsn);
        tracks.push(TocTrack {
            number,
            start_lsn,
            sectors: next.saturating_sub(start_lsn),
            is_audio,
        });
    }

    if !tracks.iter().any(|t| t.is_audio) {
        return Err(CdError::NotAudio);
    }

    Ok(Toc {
        device: dev.to_path_buf(),
        tracks,
        leadout_lsn,
    })
}

/// Read `frames` sectors starting at `lsn` into `out`, which is resized to fit.
///
/// Retries a few times before giving up: a marginal sector often reads on a
/// second attempt, and the alternative to a retry here is a click. What it
/// will NOT do is return zeros — an unrecoverable sector is an error, and the
/// caller stops the track rather than playing silence over it.
pub fn read_audio(dev: &Path, lsn: u32, frames: u32, out: &mut Vec<u8>) -> Result<(), CdError> {
    let frames = frames.clamp(1, MAX_FRAMES_PER_READ);
    out.resize(frames as usize * crate::CDDA_SECTOR_BYTES, 0);
    let fd = open_device(dev)?;
    read_audio_fd(fd.as_raw_fd(), dev, lsn, frames, out)
}

fn read_audio_fd(
    raw: libc::c_int,
    dev: &Path,
    lsn: u32,
    frames: u32,
    out: &mut [u8],
) -> Result<(), CdError> {
    let mut last = std::io::Error::other("no attempt made");
    for attempt in 0..3 {
        let mut arg = CdromReadAudio {
            addr_lba: lsn as i32,
            addr_format: CDROM_LBA,
            _pad: [0; 3],
            nframes: frames as libc::c_int,
            buf: out.as_mut_ptr(),
        };
        if unsafe { libc::ioctl(raw, CDROMREADAUDIO, &mut arg) } >= 0 {
            if attempt > 0 {
                log::info!("[cdda] sector {lsn} read on attempt {}", attempt + 1);
            }
            return Ok(());
        }
        last = std::io::Error::last_os_error();
    }
    log::warn!(
        "[cdda] {} refused sector {lsn} after 3 attempts: {last}",
        dev.display()
    );
    Err(CdError::Read { lsn, source: last })
}

/// An open reader positioned on one track, so a whole track is read through a
/// SINGLE file descriptor instead of re-opening the device 4000 times.
pub struct TrackReader {
    fd: OwnedFd,
    device: PathBuf,
    next_lsn: u32,
    end_lsn: u32,
}

impl TrackReader {
    pub fn open(dev: &Path, track: &TocTrack) -> Result<Self, CdError> {
        Ok(Self {
            fd: open_device(dev)?,
            device: dev.to_path_buf(),
            next_lsn: track.start_lsn,
            end_lsn: track.start_lsn + track.sectors,
        })
    }

    pub fn finished(&self) -> bool {
        self.next_lsn >= self.end_lsn
    }

    /// Next chunk of raw CD-DA bytes, up to `MAX_FRAMES_PER_READ` sectors.
    /// `Ok(0)` means the track is done.
    pub fn next_chunk(&mut self, out: &mut Vec<u8>) -> Result<usize, CdError> {
        if self.finished() {
            out.clear();
            return Ok(0);
        }
        let frames = (self.end_lsn - self.next_lsn).min(MAX_FRAMES_PER_READ);
        out.resize(frames as usize * crate::CDDA_SECTOR_BYTES, 0);
        read_audio_fd(
            self.fd.as_raw_fd(),
            &self.device,
            self.next_lsn,
            frames,
            out,
        )?;
        self.next_lsn += frames;
        Ok(out.len())
    }
}

/// Diagnostic: does this buffer look byte-swapped?
///
/// Audio is continuous, so neighbouring samples differ by little; a wrong
/// endianness turns that into noise. Measured on the owner's drive the ratio
/// was 285x in favour of little-endian, so this returns false there. It exists
/// because "every drive is little-endian" is a claim about ONE drive, and the
/// honest way to hold it is as something checkable rather than assumed.
pub fn looks_byte_swapped(pcm: &[u8]) -> bool {
    let step = |swap: bool| -> f64 {
        let mut total = 0f64;
        let mut n = 0u32;
        let mut prev: Option<i16> = None;
        // Left channel only: 4 bytes per frame, take the first pair.
        for f in pcm.chunks_exact(4) {
            let s = if swap {
                i16::from_be_bytes([f[0], f[1]])
            } else {
                i16::from_le_bytes([f[0], f[1]])
            };
            if let Some(p) = prev {
                total += (s as i32 - p as i32).unsigned_abs() as f64;
                n += 1;
            }
            prev = Some(s);
        }
        if n == 0 {
            0.0
        } else {
            total / n as f64
        }
    };
    let le = step(false);
    let be = step(true);
    be > 0.0 && le > be
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toc(entries: &[(u8, u32, bool)], leadout: u32) -> Toc {
        let mut tracks = Vec::new();
        for (i, &(number, start_lsn, is_audio)) in entries.iter().enumerate() {
            let next = entries.get(i + 1).map(|e| e.1).unwrap_or(leadout);
            tracks.push(TocTrack {
                number,
                start_lsn,
                sectors: next - start_lsn,
                is_audio,
            });
        }
        Toc {
            device: PathBuf::from("/dev/null"),
            tracks,
            leadout_lsn: leadout,
        }
    }

    /// The REAL table of contents of the owner's disc, read 2026-08-20.
    fn fear_inoculum() -> Toc {
        toc(
            &[
                (1, 0, true),
                (2, 46_577, true),
                (3, 100_047, true),
                (4, 157_373, true),
                (5, 218_718, true),
                (6, 264_120, true),
                (7, 285_735, true),
            ],
            356_568,
        )
    }

    #[test]
    fn track_lengths_come_from_the_gaps_and_the_leadout() {
        let t = fear_inoculum();
        // Track 1 = 10:21, and the LAST track has to reach the lead-out or the
        // longest song on the disc would be cut short.
        assert_eq!(t.tracks[0].sectors, 46_577);
        assert_eq!(t.tracks[0].duration_secs(), 621);
        assert_eq!(t.tracks[6].sectors, 70_833);
        assert_eq!(t.tracks[6].duration_secs(), 944);
        assert_eq!(t.tracks.len(), 7);
    }

    #[test]
    fn a_data_track_is_never_offered_as_audio() {
        // Enhanced CD: three songs then a data track.
        let t = toc(&[(1, 0, true), (2, 20_000, true), (3, 40_000, false)], 60_000);
        assert_eq!(t.tracks.len(), 3);
        assert_eq!(t.audio_tracks().count(), 2);
        assert!(t.audio_tracks().all(|x| x.number != 3));
    }

    #[test]
    fn the_fingerprint_identifies_the_medium_not_the_drive() {
        let a = fear_inoculum();
        let mut b = fear_inoculum();
        b.device = PathBuf::from("/dev/sr3");
        // Same disc in a different drive: same identity.
        assert_eq!(a.fingerprint(), b.fingerprint());

        // A different disc with the same track COUNT must not collide — this
        // is the case that would resolve a persisted queue onto the wrong
        // album.
        let other = toc(
            &[
                (1, 0, true),
                (2, 46_500, true),
                (3, 100_047, true),
                (4, 157_373, true),
                (5, 218_718, true),
                (6, 264_120, true),
                (7, 285_735, true),
            ],
            356_568,
        );
        assert_ne!(a.fingerprint(), other.fingerprint());
    }

    #[test]
    fn frames_per_track_match_the_wav_the_player_will_be_promised() {
        let t = fear_inoculum();
        let first = &t.tracks[0];
        assert_eq!(first.frames(), 46_577 * 588);
        // 10:21 of 44.1k stereo 16-bit — the size the streaming path declares.
        assert_eq!(
            crate::wav_total_size_16(first.frames(), 2),
            44 + 46_577 * 588 * 4
        );
    }

    #[test]
    fn a_read_is_clamped_to_what_the_kernel_accepts() {
        // The ioctl takes 1..=75 frames; asking for more is not a silent
        // truncation somewhere deep, it is clamped here where it is visible.
        assert_eq!(MAX_FRAMES_PER_READ, 75);
    }

    #[test]
    fn the_endianness_probe_agrees_with_the_measured_drive() {
        // A smooth ramp is what real audio looks like read the RIGHT way.
        let mut pcm = Vec::new();
        for i in 0..2000i16 {
            let s = (i.wrapping_mul(7)) % 3000;
            pcm.extend_from_slice(&s.to_le_bytes()); // L
            pcm.extend_from_slice(&s.to_le_bytes()); // R
        }
        assert!(!looks_byte_swapped(&pcm), "little-endian audio flagged as swapped");

        let swapped: Vec<u8> = pcm
            .chunks_exact(2)
            .flat_map(|p| [p[1], p[0]])
            .collect();
        assert!(looks_byte_swapped(&swapped), "byte-swapped audio not detected");
    }
}
