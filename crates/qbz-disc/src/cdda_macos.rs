//! CD-DA on macOS, where the operating system has already done the reading.
//!
//! macOS MOUNTS an audio CD. There is no raw-sector ioctl to reach for and no
//! device node worth opening: the disc appears at `/Volumes/<name>/` as one
//! AIFF per track, plus a `.TOC.plist` describing the geometry. So the "read"
//! here is a file read, and the seam that makes it fit — [`crate::CdRef`]
//! naming a device and a sector range — keeps working because the sector range
//! maps onto a byte range in the AIFF exactly.
//!
//! WHAT THE "DEVICE" IS. On Linux it is `/dev/sr0`. Here it is the mounted
//! VOLUME (`/Volumes/Fear Inoculum`). Both are "where this disc is", both are
//! a `PathBuf`, and nothing above this layer has to know which kind it holds.
//!
//! MEASURED on the Mac mini with the owner's Tool — *Fear Inoculum*
//! (2026-08-21), USB drive moved over from the Linux box:
//!
//!   - `.TOC.plist` gives `Start Block` WITH the 150-sector lead-in included
//!     (track 1 starts at 150, lead-out at 356718) where the Linux ioctl
//!     reports it without (0 and 356568). Subtracting `LEAD_IN` reproduces the
//!     kernel's numbers EXACTLY: same per-track LSNs, same sector counts, and
//!     therefore the same DiscID `BeNBMsD8Du5NO2W61Yk.B2jwwIs-` and the same
//!     fingerprint `38bef21351f7fca3`. That equality is the point — a disc
//!     corrected on one machine is found on the other.
//!   - The files are named `<n> <title>.aiff` when the Music app has looked
//!     the disc up, and `<n> Audio Track.aiff` when it has not. Only the
//!     LEADING INTEGER is dependable, so that is all this matches on. The
//!     titles in those names are deliberately ignored: QBZ does its own
//!     DiscID lookup, and taking Apple's guess would make the naming depend on
//!     whether some other app happened to have network access.
//!
//! BYTE ORDER, and it is NOT what the format's reputation suggests. AIFF is
//! big-endian by definition — Apple, 68k era — so the obvious reading is to
//! swap every pair on the way out. That is wrong here, and measuring caught it:
//! macOS writes **AIFC with compression type `sowt`**, which is `twos`
//! spelled backwards and means the samples are ALREADY little-endian. An
//! unconditional swap produced audio that looked perfectly healthy by every
//! cheap statistic (97 % non-zero, full-scale peak) and did not match the
//! Linux rip of the same track by a single byte.
//!
//! So the COMM chunk is read and the swap is CONDITIONAL. VERIFIED END TO END,
//! all seven tracks of the owner's disc: the PCM this file yields on macOS
//! md5s to exactly what `metaflac --show-md5sum` reports for the FLACs the
//! Linux ioctl path ripped from the same disc hours earlier. Two operating
//! systems, two read paths that share no code, one identical stream of samples.
//!
//! ONE TRANSIENT DISAGREEMENT, and it is worth knowing about. Reading all
//! seven tracks back to back once produced a different digest for the LAST
//! track; three further reads of that track all matched. So the read is not
//! perfectly reproducible on a marginal region — which is precisely the
//! caveat `cdda_linux` states about this whole approach (no overlap
//! synchronisation, no jitter verification, no scratch reconstruction). It is
//! also why the rip log must not claim more than it does.
//!
//! And a difference in what "0 read errors" MEANS here: on Linux an unreadable
//! sector fails the ioctl and aborts. On macOS the OS did the reading, so a
//! failure surfaces only if the file read fails — a drive that quietly hands
//! back wrong-but-valid bytes is invisible to both, and to EAC in burst mode.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::toc::{CdError, Toc, TocTrack};

/// Sectors of lead-in that macOS includes in every `Start Block` and the
/// kernel does not. It is not a fudge factor: 150 sectors is two seconds, the
/// Red Book lead-in, and the MusicBrainz DiscID is defined in terms of the
/// numbers WITH it — which is why `discid::disc_id` adds it back.
const LEAD_IN: u32 = 150;

/// Mounted audio CDs, one entry per volume that has a `.TOC.plist`.
///
/// The plist is the discriminator rather than the file extension: a volume
/// full of AIFFs somebody copied to a USB stick is not a disc, and offering to
/// rip it would be offering to copy files the user already has.
pub fn list_devices() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/Volumes") else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.join(".TOC.plist").is_file() {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// The drive behind a mounted volume, for the rip log.
///
/// `diskutil` knows, but shelling out for one cosmetic line in a log is not
/// worth the process — and on this path the OS did the reading anyway, so the
/// honest answer is that the drive is not what produced these bytes. The log's
/// non-Linux arm says so in words instead.
pub fn drive_model(_dev: &Path) -> Option<String> {
    None
}

/// Read the table of contents out of the volume's `.TOC.plist`.
pub fn read_toc(dev: &Path) -> Result<Toc, CdError> {
    let path = dev.join(".TOC.plist");
    let raw = std::fs::read(&path).map_err(|e| CdError::Open(path.clone(), e))?;
    let value: plist::Value = plist::from_bytes(&raw)
        .map_err(|e| CdError::Toc(std::io::Error::other(format!("{path:?}: {e}"))))?;

    let sessions = value
        .as_dictionary()
        .and_then(|d| d.get("Sessions"))
        .and_then(|s| s.as_array())
        .ok_or_else(|| CdError::Toc(std::io::Error::other("no Sessions in .TOC.plist")))?;

    // A hybrid disc can carry several sessions; the audio one is session 1 and
    // is what a CD player sees. Anything past it is data by construction.
    let session = sessions
        .first()
        .and_then(|s| s.as_dictionary())
        .ok_or_else(|| CdError::Toc(std::io::Error::other("empty Sessions")))?;

    let leadout = session
        .get("Leadout Block")
        .and_then(|v| v.as_unsigned_integer())
        .ok_or_else(|| CdError::Toc(std::io::Error::other("no Leadout Block")))?
        as u32;

    let array = session
        .get("Track Array")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CdError::Toc(std::io::Error::other("no Track Array")))?;

    // (number, start, is_audio), in disc order.
    let mut raw_tracks: Vec<(u8, u32, bool)> = Vec::new();
    for t in array {
        let Some(d) = t.as_dictionary() else { continue };
        let (Some(point), Some(start)) = (
            d.get("Point").and_then(|v| v.as_unsigned_integer()),
            d.get("Start Block").and_then(|v| v.as_unsigned_integer()),
        ) else {
            continue;
        };
        // `Data` true is the data track of a mixed-mode disc — the same track
        // the Linux path skips by its `cdte_ctrl` bit.
        let is_audio = !d.get("Data").and_then(|v| v.as_boolean()).unwrap_or(false);
        raw_tracks.push((point as u8, start as u32, is_audio));
    }
    raw_tracks.sort_by_key(|t| t.1);
    if raw_tracks.is_empty() {
        return Err(CdError::NotAudio);
    }
    if !raw_tracks.iter().any(|t| t.2) {
        return Err(CdError::NotAudio);
    }

    // A track's length is the distance to the NEXT track's start, and the last
    // one runs to the lead-out — computed AFTER the lead-in is removed so the
    // subtraction cannot underflow on track 1.
    let starts: Vec<u32> = raw_tracks
        .iter()
        .map(|t| t.1.saturating_sub(LEAD_IN))
        .collect();
    let leadout = leadout.saturating_sub(LEAD_IN);
    let tracks = raw_tracks
        .iter()
        .enumerate()
        .map(|(i, (number, _, is_audio))| {
            let start = starts[i];
            let end = starts.get(i + 1).copied().unwrap_or(leadout);
            TocTrack {
                number: *number,
                start_lsn: start,
                sectors: end.saturating_sub(start),
                is_audio: *is_audio,
            }
        })
        .collect();

    Ok(Toc {
        device: dev.to_path_buf(),
        tracks,
        leadout_lsn: leadout,
    })
}

/// The AIFF the OS mounted for one track number.
///
/// Matched on the LEADING INTEGER of the file name and nothing else: the rest
/// is whatever the Music app decided to call it, which depends on a lookup QBZ
/// did not make and may not have happened at all.
fn track_file(dev: &Path, number: u8) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dev).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
        if ext != "aiff" && ext != "aif" {
            continue;
        }
        let name = p.file_name()?.to_string_lossy().into_owned();
        let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.parse::<u8>().ok() == Some(number) {
            return Some(p);
        }
    }
    None
}

/// Where the audio sits inside an AIFF/AIFC, and which way round its bytes are.
///
/// A hand-rolled walk of the IFF chunks rather than a decoder dependency: the
/// payload is raw PCM and the questions are only "where does SSND's data
/// begin", "how long is it" and "is it big-endian". `offset`/`blockSize` in
/// SSND are almost always zero but are honoured anyway, because a file that
/// sets them and is read as if it had not is off by that many bytes for its
/// whole length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AudioExtent {
    at: u64,
    len: u64,
    /// The samples need swapping to reach little-endian.
    big_endian: bool,
}

fn aiff_audio_extent(file: &mut std::fs::File) -> Result<AudioExtent, std::io::Error> {
    let mut header = [0u8; 12];
    file.read_exact(&mut header)?;
    if &header[0..4] != b"FORM" {
        return Err(std::io::Error::other("not an IFF file"));
    }
    let is_aifc = &header[8..12] == b"AIFC";
    if &header[8..12] != b"AIFF" && !is_aifc {
        return Err(std::io::Error::other("not an AIFF"));
    }
    // Plain AIFF has no compression field and is big-endian by definition.
    // AIFC declares one, and macOS writes `sowt` — little-endian.
    let mut big_endian = true;
    let mut pos = 12u64;
    loop {
        let mut ch = [0u8; 8];
        file.seek(SeekFrom::Start(pos))?;
        if file.read_exact(&mut ch).is_err() {
            return Err(std::io::Error::other("no SSND chunk"));
        }
        let size = u32::from_be_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        if &ch[0..4] == b"COMM" && is_aifc && size >= 22 {
            // channels(2) frames(4) bits(2) rate(10) = 18, then the 4-byte
            // compression id.
            let mut body = [0u8; 22];
            file.read_exact(&mut body)?;
            big_endian = match &body[18..22] {
                // Little-endian two's complement: `twos` written backwards,
                // which is the joke and the definition.
                b"sowt" => false,
                // `NONE` and `twos` are the big-endian spellings; anything
                // else is a codec this reader does not decode, and the honest
                // move is to say so rather than emit noise.
                b"NONE" | b"twos" | b"raw " => true,
                other => {
                    return Err(std::io::Error::other(format!(
                        "unsupported AIFC compression {:?}",
                        String::from_utf8_lossy(other)
                    )))
                }
            };
        }
        if &ch[0..4] == b"SSND" {
            let mut ssnd = [0u8; 8];
            file.read_exact(&mut ssnd)?;
            let offset = u32::from_be_bytes([ssnd[0], ssnd[1], ssnd[2], ssnd[3]]) as u64;
            return Ok(AudioExtent {
                at: pos + 8 + 8 + offset,
                len: size.saturating_sub(8 + offset),
                big_endian,
            });
        }
        // IFF chunks are word-aligned: an odd size is followed by a pad byte.
        pos += 8 + size + (size & 1);
    }
}

/// Reads one track's audio out of the mounted AIFF, in the sector-sized
/// chunks the Linux path hands back.
///
/// The API is the Linux `TrackReader`'s, deliberately: `audible_qt` and
/// `qbz-rip` drive both platforms through the same two calls.
pub struct TrackReader {
    file: std::fs::File,
    /// Byte offset of the next read, absolute in the file.
    at: u64,
    /// One past the last audio byte of this track.
    end: u64,
    /// The file's samples are big-endian and need swapping on the way out.
    /// FALSE on every disc macOS mounts (`sowt`), and honoured anyway because
    /// a plain AIFF is a legal thing to find here.
    big_endian: bool,
}

impl TrackReader {
    /// Open a reader over the audio at this track's SECTOR RANGE.
    ///
    /// Resolved by `start_lsn`, NOT by `number`, and that is not a preference:
    /// `audible_qt::play_cd_track` rebuilds a `TocTrack` from a [`crate::CdRef`]
    /// — which carries a device and a sector range and no track number — and
    /// fills the number in as ZERO. On Linux nothing reads it. Here, matching
    /// on it meant every CD track answered "could not read the disc", and the
    /// caller was right: the sector range IS the address, so that is what this
    /// resolves.
    ///
    /// A start INSIDE a track is honoured too, so a caller that asks for a
    /// sector part-way in gets that offset rather than the track's beginning.
    pub fn open(dev: &Path, track: &TocTrack) -> Result<Self, CdError> {
        let toc = read_toc(dev)?;
        let owner = toc
            .tracks
            .iter()
            .find(|t| track.start_lsn >= t.start_lsn && track.start_lsn < t.start_lsn + t.sectors)
            // A zero-length or out-of-range request still gets the track whose
            // start matches exactly, which is what a caller passing a whole
            // track means.
            .or_else(|| toc.tracks.iter().find(|t| t.start_lsn == track.start_lsn))
            .ok_or(CdError::NoDisc)?;
        let into_track =
            (track.start_lsn - owner.start_lsn) as u64 * crate::CDDA_SECTOR_BYTES as u64;

        let path = track_file(dev, owner.number).ok_or(CdError::NoDisc)?;
        let mut file =
            std::fs::File::open(&path).map_err(|e| CdError::Open(path.clone(), e))?;
        let extent = aiff_audio_extent(&mut file).map_err(|e| CdError::Open(path.clone(), e))?;

        // The TOC's sector count is the LENGTH, and the SSND payload is only a
        // ceiling. Measured: SSND carries 570 frames MORE than COMM declares
        // on every track of this disc (2280 bytes of padding), so trusting the
        // payload size would append a fifth of a second of tail to each track
        // and break the digest match with the Linux rip. COMM's frame count
        // agrees with the TOC exactly, on all seven.
        let want = track.sectors as u64 * crate::CDDA_SECTOR_BYTES as u64;
        let available = extent.len.saturating_sub(into_track);
        let len = want.min(available);
        let at = extent.at + into_track;
        file.seek(SeekFrom::Start(at))
            .map_err(|e| CdError::Open(path.clone(), e))?;
        Ok(Self {
            file,
            at,
            end: at + len,
            big_endian: extent.big_endian,
        })
    }

    /// Append the next chunk to `out` (cleared first), returning its length.
    /// Zero means the track is done.
    pub fn next_chunk(&mut self, out: &mut Vec<u8>) -> Result<usize, CdError> {
        out.clear();
        if self.at >= self.end {
            return Ok(0);
        }
        let want = (crate::cdda::MAX_FRAMES_PER_READ as u64
            * crate::CDDA_SECTOR_BYTES as u64)
            .min(self.end - self.at) as usize;
        out.resize(want, 0);
        self.file
            .read_exact(out)
            .map_err(|e| CdError::Read { lsn: 0, source: e })?;
        self.at += want as u64;
        // Only when the FILE says so. macOS writes `sowt` (little-endian), so
        // this does nothing on a mounted CD — and doing it unconditionally,
        // which is what "AIFF is big-endian" suggests, silently produced audio
        // that passed every cheap sanity check and matched nothing.
        if self.big_endian {
            for pair in out.chunks_exact_mut(2) {
                pair.swap(0, 1);
            }
        }
        Ok(want)
    }
}

/// Sector-addressed read, for parity with the Linux entry point.
pub fn read_audio(dev: &Path, lsn: u32, frames: u32, out: &mut Vec<u8>) -> Result<(), CdError> {
    let toc = read_toc(dev)?;
    let track = toc
        .tracks
        .iter()
        .find(|t| lsn >= t.start_lsn && lsn < t.start_lsn + t.sectors)
        .ok_or(CdError::NoDisc)?;
    // `open` resolves and seeks by `start_lsn`, so asking for the requested
    // sector directly is enough — there is no second offset to apply here.
    let mut reader = TrackReader::open(
        dev,
        &TocTrack {
            number: track.number,
            start_lsn: lsn,
            sectors: frames,
            is_audio: true,
        },
    )?;
    out.clear();
    let mut chunk = Vec::new();
    while reader.next_chunk(&mut chunk)? > 0 {
        out.extend_from_slice(&chunk);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lead-in conversion, against the numbers read off the owner's disc
    /// on BOTH machines the same afternoon. If this ever drifts, a disc
    /// corrected on Linux stops being found on macOS and nothing says why.
    #[test]
    fn the_mac_toc_reproduces_the_kernels_numbers_exactly() {
        let mac_starts: [u32; 7] = [150, 46727, 100197, 157523, 218868, 264270, 285885];
        let linux_starts: [u32; 7] = [0, 46577, 100047, 157373, 218718, 264120, 285735];
        for (m, l) in mac_starts.iter().zip(linux_starts.iter()) {
            assert_eq!(m - LEAD_IN, *l);
        }
        assert_eq!(356718 - LEAD_IN, 356568);
    }

    #[test]
    fn a_track_file_is_matched_on_its_number_and_never_on_its_title() {
        let dir = std::env::temp_dir().join(format!("qbz-mac-cd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Both shapes the Music app produces, plus a decoy that starts with a
        // digit but is not a track.
        std::fs::write(dir.join("1 Fear Inoculum.aiff"), b"x").unwrap();
        std::fs::write(dir.join("2 Audio Track.aiff"), b"x").unwrap();
        std::fs::write(dir.join("3 Invincible.txt"), b"x").unwrap();
        assert!(track_file(&dir, 1).unwrap().ends_with("1 Fear Inoculum.aiff"));
        assert!(track_file(&dir, 2).unwrap().ends_with("2 Audio Track.aiff"));
        assert!(track_file(&dir, 3).is_none(), "a .txt is not a track");
        assert!(track_file(&dir, 9).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Build a volume that looks like a mounted audio CD: a `.TOC.plist` and
    /// one AIFC per track, `sowt` like the real thing.
    fn fake_volume(dir: &Path, tracks: &[(u8, u32, u32)]) {
        std::fs::create_dir_all(dir).unwrap();
        let mut plist_tracks = Vec::new();
        for (n, start, sectors) in tracks {
            let mut d = plist::Dictionary::new();
            d.insert("Point".into(), plist::Value::from(*n as u64));
            d.insert("Start Block".into(), plist::Value::from((*start + LEAD_IN) as u64));
            d.insert("Data".into(), plist::Value::from(false));
            plist_tracks.push(plist::Value::Dictionary(d));

            // One AIFC whose payload is the track number repeated, so a read
            // that lands on the wrong file is unmistakable.
            let bytes = *sectors as usize * crate::CDDA_SECTOR_BYTES;
            let mut f = Vec::new();
            f.extend_from_slice(b"FORM");
            f.extend_from_slice(&0u32.to_be_bytes());
            f.extend_from_slice(b"AIFC");
            f.extend_from_slice(b"COMM");
            f.extend_from_slice(&22u32.to_be_bytes());
            f.extend_from_slice(&2u16.to_be_bytes());
            f.extend_from_slice(&((bytes / 4) as u32).to_be_bytes());
            f.extend_from_slice(&16u16.to_be_bytes());
            f.extend_from_slice(&[0u8; 10]);
            f.extend_from_slice(b"sowt");
            f.extend_from_slice(b"SSND");
            f.extend_from_slice(&((bytes + 8) as u32).to_be_bytes());
            f.extend_from_slice(&0u32.to_be_bytes());
            f.extend_from_slice(&0u32.to_be_bytes());
            f.extend(std::iter::repeat(*n).take(bytes));
            std::fs::write(dir.join(format!("{n} Track {n}.aiff")), &f).unwrap();
        }
        let last = tracks.last().unwrap();
        let mut session = plist::Dictionary::new();
        session.insert("First Track".into(), plist::Value::from(1u64));
        session.insert("Last Track".into(), plist::Value::from(tracks.len() as u64));
        session.insert(
            "Leadout Block".into(),
            plist::Value::from((last.1 + last.2 + LEAD_IN) as u64),
        );
        session.insert("Track Array".into(), plist::Value::Array(plist_tracks));
        let mut root = plist::Dictionary::new();
        root.insert(
            "Sessions".into(),
            plist::Value::Array(vec![plist::Value::Dictionary(session)]),
        );
        plist::to_file_xml(dir.join(".TOC.plist"), &plist::Value::Dictionary(root)).unwrap();
    }

    /// THE regression guard for the 2026-08-21 defect.
    ///
    /// `audible_qt::play_cd_track` rebuilds its `TocTrack` from a `CdRef`,
    /// which has no track number, and passes ZERO. Resolving by number meant
    /// every CD track on macOS answered "could not read the disc" — while
    /// ripping, which passes a real number, worked fine. So the two paths
    /// disagreed and only one of them was ever exercised.
    #[test]
    fn a_track_opens_by_its_sector_range_even_with_no_track_number() {
        let dir = std::env::temp_dir().join(format!("qbz-mac-lsn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Two tracks, 2 and 3 sectors long.
        fake_volume(&dir, &[(1, 0, 2), (2, 2, 3)]);

        let toc = read_toc(&dir).unwrap();
        assert_eq!(toc.tracks.len(), 2);
        assert_eq!(toc.tracks[0].start_lsn, 0);
        assert_eq!(toc.tracks[1].start_lsn, 2);
        assert_eq!(toc.tracks[1].sectors, 3);

        // Exactly what playback hands over: no number.
        let as_playback_sees_it = TocTrack {
            number: 0,
            start_lsn: 2,
            sectors: 3,
            is_audio: true,
        };
        let mut reader = TrackReader::open(&dir, &as_playback_sees_it)
            .expect("a numberless track must still open");
        let mut chunk = Vec::new();
        let mut all = Vec::new();
        while reader.next_chunk(&mut chunk).unwrap() > 0 {
            all.extend_from_slice(&chunk);
        }
        assert_eq!(all.len(), 3 * crate::CDDA_SECTOR_BYTES);
        assert!(all.iter().all(|b| *b == 2), "it opened track 2's file");

        // And a start PART-WAY into a track lands at that offset, not at the
        // track's beginning.
        let mid = TocTrack {
            number: 0,
            start_lsn: 3,
            sectors: 2,
            is_audio: true,
        };
        let mut reader = TrackReader::open(&dir, &mid).unwrap();
        let mut all = Vec::new();
        while reader.next_chunk(&mut chunk).unwrap() > 0 {
            all.extend_from_slice(&chunk);
        }
        assert_eq!(all.len(), 2 * crate::CDDA_SECTOR_BYTES);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `sowt` means the samples are ALREADY little-endian. Swapping them —
    /// which "AIFF is big-endian" invites — produced audio that passed every
    /// cheap statistic and matched the Linux rip nowhere.
    #[test]
    fn a_sowt_payload_is_not_byte_swapped() {
        let dir = std::env::temp_dir().join(format!("qbz-mac-sowt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        fake_volume(&dir, &[(1, 0, 1)]);
        let mut file = std::fs::File::open(dir.join("1 Track 1.aiff")).unwrap();
        let e = aiff_audio_extent(&mut file).unwrap();
        assert!(!e.big_endian, "sowt is little-endian");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_ssnd_payload_is_found_past_the_chunks_before_it() {
        let dir = std::env::temp_dir().join(format!("qbz-mac-aiff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.aiff");
        let mut f = Vec::new();
        f.extend_from_slice(b"FORM");
        f.extend_from_slice(&0u32.to_be_bytes()); // size, unread
        f.extend_from_slice(b"AIFF");
        // A COMM chunk with an ODD size, so the pad byte has to be honoured.
        f.extend_from_slice(b"COMM");
        f.extend_from_slice(&3u32.to_be_bytes());
        f.extend_from_slice(&[1, 2, 3, 0]);
        f.extend_from_slice(b"SSND");
        f.extend_from_slice(&(8u32 + 4).to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes()); // offset
        f.extend_from_slice(&0u32.to_be_bytes()); // blockSize
        f.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        std::fs::write(&path, &f).unwrap();

        let mut file = std::fs::File::open(&path).unwrap();
        let e = aiff_audio_extent(&mut file).unwrap();
        assert_eq!(e.len, 4);
        assert!(e.big_endian, "a plain AIFF has no compression field");
        let mut buf = vec![0u8; 4];
        file.seek(SeekFrom::Start(e.at)).unwrap();
        file.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [0xAA, 0xBB, 0xCC, 0xDD]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
