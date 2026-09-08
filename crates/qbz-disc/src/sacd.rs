//! Scarlet Book: the stereo audio area of a SACD image.
//!
//! Every structure and every constant below was MEASURED on the owner's two
//! Rheingold discs before a line of it was written, by a census rather than a
//! sample — 3 066 893 sectors, both discs, zero exceptions. That matters
//! because the previous attempt at this file started from a single sector and
//! concluded the per-sector header was a fixed 8 bytes. It is not, and a fixed
//! skip would have leaked packet descriptors into the DSD stream: audible
//! noise on a path whose entire promise is bit-perfection.
//!
//! WHAT IS SUPPORTED: DSD64 stereo areas, either flat or DST-compressed, in
//! raw Scarlet Book dumps and ISO/UDF hybrid images. Every pointer and audio
//! extent is bounded by the image; nothing is inferred from an ISO 9660 layer.
//! Images may store 2048-byte logical sectors directly or retain the 12-byte
//! header and 4-byte trailer of each 2064-byte physical sector.

use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::iso9660::{IsoError, SECTOR};

const MASTER_TOC_COPIES: [u64; 3] = [510, 520, 530];
const MAX_AREA_TOC_SECTORS: u16 = 96;

/// Sectors per second of SACD playing time. The area TOC states it as a
/// max_byte_rate of 716 800 B/s, and 716800 / 2048 = 350. It is the SACD's
/// answer to the CD's 75, and it is what ties a track's start LSN to its
/// start time — an identity that holds for 44 of 44 tracks on both discs:
///     start_lsn[i] == floor(area_start + start_time[i] * 350)
pub const SECTORS_PER_SEC: u32 = 350;

/// One uncompressed DSD64 STEREO frame: 2 ch x 2 822 400 bit/s / 75 frames/s
/// / 8 = 9408 bytes. Used as the proof that an area carries no DST — the sum
/// of its audio-packet lengths divides by this EXACTLY (350 495 frames on
/// Disc 1, 306 696 on Disc 2). Compressed frames could not give an integer.
pub const DSD64_STEREO_FRAME: u32 = 9408;

/// Packet data types seen across both discs: {2, 3, 7}. Only 2 is audio.
pub const DATA_TYPE_AUDIO: u16 = 2;

#[derive(Debug, thiserror::Error)]
pub enum SacdError {
    #[error(transparent)]
    Iso(#[from] IsoError),
    #[error("this image has no Scarlet Book stereo audio area")]
    NoStereoArea,
    #[error("this image has no Scarlet Book Master TOC")]
    MissingMasterToc,
    #[error("the area TOC is not a Scarlet Book stereo TOC (expected TWOCHTOC)")]
    NotAnArea,
    #[error("{0} is missing from the area TOC")]
    MissingBlock(&'static str),
    #[error("the legacy flat-DSD reader cannot expose compressed DST frames")]
    Dst,
    #[error("unsupported channel count: {0}")]
    Channels(u8),
    #[error("unsupported SACD frame format: {0}")]
    FrameFormat(u8),
    #[error("unsupported SACD sample-frequency code: {0}")]
    SampleFrequency(u8),
    #[error("the area declares {0} tracks, which cannot be right")]
    TrackCount(u8),
    #[error("the Scarlet Book TOC is malformed: {0}")]
    MalformedToc(&'static str),
    #[error("the valid Master TOC copies disagree about the disc geometry")]
    ConflictingMasterTocs,
    #[error("the valid copies of an area TOC disagree about its geometry")]
    ConflictingAreaTocs,
    #[error("sector {lsn} is malformed: {why}")]
    BadSector { lsn: u64, why: &'static str },
    #[error("track {track} frame at sector {lsn} is malformed: {why}")]
    BadAudioFrame {
        track: u8,
        lsn: u64,
        why: &'static str,
    },
    #[error("track {track} has no frame start within 32 sectors from {lsn}")]
    MissingFrameStart { track: u8, lsn: u64 },
    #[error("{0} is not a regular file")]
    NotRegularFile(PathBuf),
    #[error("SACD image length {0} contains a partial sector in the detected layout")]
    InvalidImageLength(u64),
    #[error("SACD image has Master TOC signatures in conflicting sector layouts")]
    AmbiguousSectorLayout,
    #[error("SACD image changed while it was being read: {0}")]
    ImageChangedDuringRead(PathBuf),
}

/// How audio frames in the selected stereo area are represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SacdEncoding {
    Dst,
    Dsd3In14,
    Dsd3In16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SacdTrack {
    /// 1-based, as printed on the sleeve.
    pub number: u8,
    pub start_lsn: u32,
    /// Sectors to READ. Deliberately one more than the exclusive extent for
    /// every track but the last — that is how the disc records it, and the
    /// ISO 9660 directory mirrors it byte for byte, so the one-sector overlap
    /// a naive reading sees is intentional rather than a bug to correct.
    pub length_lsn: u32,
    pub start_secs: f64,
    pub duration_secs: f64,
    /// Exact 75 Hz Scarlet Book timecode, without a float round-trip.
    pub start_frame: u32,
    pub duration_frames: u32,
    pub encoding: SacdEncoding,
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SacdArea {
    pub track_start_lsn: u32,
    pub track_end_lsn: u32,
    pub channels: u8,
    pub encoding: SacdEncoding,
    pub total_playtime_secs: f64,
    pub tracks: Vec<SacdTrack>,
    /// Album text out of the Master TOC, when it carries any.
    pub album: Option<String>,
    /// Album artist, same source.
    pub artist: Option<String>,
}

impl SacdArea {
    /// A stable identity for this disc, from its GEOMETRY alone.
    ///
    /// The twin of [`crate::cdda::Toc::fingerprint`], and the same FNV-1a for
    /// the same reason: no dependency, and it only has to tell two records
    /// apart. It exists so `crate::store` can remember a correction for a SACD
    /// too — an image names itself, but "names itself" and "names itself
    /// CORRECTLY" are different claims, and the second one is the user's to
    /// make.
    ///
    /// Deliberately NOT the file path: the same disc ripped to two folders is
    /// one record, and a `.iso` that gets moved must not lose its correction.
    /// Deliberately not the titles either — a fingerprint that changes when
    /// the user edits a title could never find the edit again.
    pub fn fingerprint(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |v: u32| {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x1000_0000_01b3);
            }
        };
        for t in &self.tracks {
            eat(t.start_lsn);
            eat(t.length_lsn);
            eat(t.number as u32);
        }
        eat(self.track_start_lsn);
        eat(self.track_end_lsn);
        eat(self.channels as u32);
        format!("sacd-{h:016x}")
    }
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn time_frames_at(b: &[u8], at: usize) -> u32 {
    (b[at] as u32 * 60 + b[at + 1] as u32) * 75 + b[at + 2] as u32
}

/// A bounded reader over the 2048-byte logical sectors shared by authored
/// SACD/ISO hybrids and raw Scarlet Book dumps. No filesystem layer is
/// required: the Master TOC and every audio extent already use absolute LSNs.
struct SectorImage {
    file: File,
    path: PathBuf,
    bytes: u64,
    layout: SectorLayout,
    master_signature_found: bool,
    stamp: ImageStamp,
    sectors_since_check: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectorLayout {
    Logical2048,
    Physical2064,
}

impl SectorLayout {
    fn stride(self) -> u64 {
        match self {
            Self::Logical2048 => SECTOR,
            Self::Physical2064 => 2064,
        }
    }

    fn payload_offset(self) -> u64 {
        match self {
            Self::Logical2048 => 0,
            Self::Physical2064 => 12,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageStamp {
    bytes: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    change_seconds: i64,
    #[cfg(unix)]
    change_nanoseconds: i64,
}

impl ImageStamp {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            bytes: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            change_seconds: metadata.ctime(),
            #[cfg(unix)]
            change_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

impl SectorImage {
    fn open(path: &Path) -> Result<Self, SacdError> {
        let file = File::open(path)
            .map_err(|error| SacdError::Iso(IsoError::Open(path.to_path_buf(), error)))?;
        let metadata = file
            .metadata()
            .map_err(|source| SacdError::Iso(IsoError::Read { lsn: 0, source }))?;
        if !metadata.is_file() {
            return Err(SacdError::NotRegularFile(path.to_path_buf()));
        }
        let bytes = metadata.len();
        let mut image = Self {
            file,
            path: path.to_path_buf(),
            bytes,
            layout: SectorLayout::Logical2048,
            master_signature_found: false,
            stamp: ImageStamp::from_metadata(&metadata),
            sectors_since_check: 0,
        };
        let layout = image.detect_layout()?;
        image.master_signature_found = layout.is_some();
        image.layout = layout.unwrap_or(SectorLayout::Logical2048);
        image.verify_unchanged()?;
        Ok(image)
    }

    /// File size alone cannot distinguish layouts: a 2064-byte dump with a
    /// multiple of 128 sectors is also divisible by 2048. Probe all three
    /// Master positions, including backups and signed but truncated images.
    /// Full geometry validation remains the responsibility of the TOC parser.
    fn detect_layout(&mut self) -> Result<Option<SectorLayout>, SacdError> {
        let mut selected = None;
        for layout in [SectorLayout::Logical2048, SectorLayout::Physical2064] {
            for lsn in MASTER_TOC_COPIES {
                let offset = lsn * layout.stride() + layout.payload_offset();
                if offset + 8 > self.bytes {
                    continue;
                }
                let mut signature = [0u8; 8];
                self.file
                    .seek(SeekFrom::Start(offset))
                    .and_then(|_| self.file.read_exact(&mut signature))
                    .map_err(|source| SacdError::Iso(IsoError::Read { lsn, source }))?;
                if &signature == b"SACDMTOC" {
                    if selected.is_some_and(|previous| previous != layout) {
                        return Err(SacdError::AmbiguousSectorLayout);
                    }
                    selected = Some(layout);
                    break;
                }
            }
        }
        Ok(selected)
    }

    fn validate_length(&self) -> Result<(), SacdError> {
        if self.bytes % self.layout.stride() != 0 {
            return Err(SacdError::InvalidImageLength(self.bytes));
        }
        Ok(())
    }

    fn sectors(&self) -> u64 {
        self.bytes / self.layout.stride()
    }

    fn has_complete_sector(&self, lsn: u64) -> bool {
        lsn < self.sectors()
    }

    fn has_sectors(&self, lsn: u64, count: u64) -> bool {
        lsn.checked_add(count)
            .is_some_and(|end| end <= self.sectors())
    }

    fn read_sectors(&mut self, lsn: u64, count: usize) -> Result<Vec<u8>, SacdError> {
        let count_u64 =
            u64::try_from(count).map_err(|_| SacdError::MalformedToc("sector count overflows"))?;
        let end_lsn = lsn
            .checked_add(count_u64)
            .ok_or(SacdError::MalformedToc("sector range overflows"))?;
        if end_lsn > self.sectors() {
            return Err(SacdError::Iso(IsoError::Read {
                lsn,
                source: std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("read past end of {}", self.path.display()),
                ),
            }));
        }
        let stride = self.layout.stride() as usize;
        let bytes = count
            .checked_mul(stride)
            .ok_or(SacdError::MalformedToc("byte count overflows"))?;
        let mut out = vec![0u8; bytes];
        self.file
            .seek(SeekFrom::Start(lsn * self.layout.stride()))
            .and_then(|_| self.file.read_exact(&mut out))
            .map_err(|source| SacdError::Iso(IsoError::Read { lsn, source }))?;
        if self.layout == SectorLayout::Physical2064 {
            // Read contiguous physical sectors once, then compact their payloads
            // in place. Headers/trailers must never reach TOC or audio parsing.
            for index in 0..count {
                let from = index * stride + self.layout.payload_offset() as usize;
                out.copy_within(from..from + SECTOR as usize, index * SECTOR as usize);
            }
            out.truncate(count * SECTOR as usize);
        }
        self.sectors_since_check = self.sectors_since_check.saturating_add(count_u64);
        // A one-second cadence avoids a metadata syscall for every 1/75 s
        // audio frame while still detecting in-place writes and path swaps
        // during long playback. TOC callers also verify explicitly at end.
        if self.sectors_since_check >= u64::from(SECTORS_PER_SEC) {
            self.verify_unchanged()?;
            self.sectors_since_check = 0;
        }
        Ok(out)
    }

    fn verify_unchanged(&self) -> Result<(), SacdError> {
        let open_stamp = self
            .file
            .metadata()
            .ok()
            .map(|metadata| ImageStamp::from_metadata(&metadata));
        let path_stamp = std::fs::metadata(&self.path)
            .ok()
            .map(|metadata| ImageStamp::from_metadata(&metadata));
        if open_stamp.as_ref() != Some(&self.stamp) || path_stamp.as_ref() != Some(&self.stamp) {
            return Err(SacdError::ImageChangedDuringRead(self.path.clone()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AreaSlot {
    primary: u32,
    backup: u32,
    size: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MasterToc {
    lsn: u64,
    version: (u8, u8),
    areas: [AreaSlot; 2],
    text_area_count: u8,
    text_charset: u8,
}

fn parse_master_toc(sector: &[u8], lsn: u64, image_sectors: u64) -> Result<MasterToc, SacdError> {
    if sector.len() < SECTOR as usize || &sector[..8] != b"SACDMTOC" {
        return Err(SacdError::MalformedToc("missing SACDMTOC signature"));
    }
    let version = (sector[8], sector[9]);
    if version.0 != 1 || version.1 > 20 {
        return Err(SacdError::MalformedToc("unsupported Master TOC version"));
    }
    let areas = [
        AreaSlot {
            primary: be32(sector, 0x40),
            backup: be32(sector, 0x44),
            size: be16(sector, 0x54),
        },
        AreaSlot {
            primary: be32(sector, 0x48),
            backup: be32(sector, 0x4c),
            size: be16(sector, 0x56),
        },
    ];
    for slot in areas {
        if slot.primary == 0 && slot.backup == 0 && slot.size == 0 {
            continue;
        }
        if slot.size == 0 || slot.size > MAX_AREA_TOC_SECTORS {
            return Err(SacdError::MalformedToc("invalid area TOC size"));
        }
        if slot.primary == 0 && slot.backup == 0 {
            return Err(SacdError::MalformedToc("area TOC has no readable copy"));
        }
        for start in [slot.primary, slot.backup]
            .into_iter()
            .filter(|start| *start != 0)
        {
            if u64::from(start)
                .checked_add(u64::from(slot.size))
                .is_none_or(|end| end > image_sectors)
            {
                return Err(SacdError::MalformedToc("area TOC points outside the image"));
            }
        }
    }
    if areas.iter().all(|slot| slot.size == 0) {
        return Err(SacdError::NoStereoArea);
    }
    Ok(MasterToc {
        lsn,
        version,
        areas,
        text_area_count: sector[0x80],
        // First of the eight locale entries at 0x88: language[2], charset,
        // reserved. QBZ deliberately selects the first text area, as the
        // reference reader does.
        text_charset: sector[0x8a],
    })
}

fn select_master_toc(image: &mut SectorImage) -> Result<Option<MasterToc>, SacdError> {
    let mut valid = Vec::new();
    let mut signed_error = None;
    for lsn in MASTER_TOC_COPIES {
        if !image.has_complete_sector(lsn) {
            continue;
        }
        let first = image.read_sectors(lsn, 1)?;
        if &first[..8] != b"SACDMTOC" {
            continue;
        }
        if !image.has_sectors(lsn, 10) {
            signed_error.get_or_insert(SacdError::MalformedToc("Master TOC copy is truncated"));
            continue;
        }
        let sector = image.read_sectors(lsn, 10)?;
        match parse_master_toc(&sector, lsn, image.sectors()) {
            Ok(master) => valid.push(master),
            Err(error) => {
                signed_error.get_or_insert(error);
            }
        };
    }
    let Some(selected) = valid.first().copied() else {
        return match signed_error {
            Some(error) => Err(error),
            None => Ok(None),
        };
    };
    if valid
        .iter()
        .skip(1)
        .any(|copy| copy.version != selected.version || copy.areas != selected.areas)
    {
        return Err(SacdError::ConflictingMasterTocs);
    }
    Ok(Some(selected))
}

/// Result of the cheap scan-time signature and Master TOC validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SacdSniff {
    NotSacd,
    Sacd,
}

/// Signature sniff for a folder scan. Scarlet Book identity is the validated
/// Master TOC, not the optional ISO 9660 compatibility layer.
pub fn sniff_sacd_image(path: &Path) -> Result<SacdSniff, SacdError> {
    let mut image = SectorImage::open(path)?;
    if !image.master_signature_found {
        image.verify_unchanged()?;
        return Ok(SacdSniff::NotSacd);
    }
    // A signed image with a truncated tail is an error, not an ordinary ISO
    // to ignore/prune. This also covers a signature in a partial Master sector.
    image.validate_length()?;
    let result = match select_master_toc(&mut image)? {
        Some(_) => SacdSniff::Sacd,
        None => SacdSniff::NotSacd,
    };
    image.verify_unchanged()?;
    Ok(result)
}

/// Compatibility convenience for callers that only need a boolean.
pub fn is_sacd_image(path: &Path) -> bool {
    matches!(sniff_sacd_image(path), Ok(SacdSniff::Sacd))
}

/// Read the stereo area's table of contents out of a disc image.
pub fn read_area(path: &Path) -> Result<SacdArea, SacdError> {
    let mut image = SectorImage::open(path)?;
    image.validate_length()?;
    let master = select_master_toc(&mut image)?.ok_or(SacdError::MissingMasterToc)?;

    let mut stereo = None;
    let mut first_area_error = None;
    for slot in master.areas.into_iter().filter(|slot| slot.size != 0) {
        match read_area_slot(&mut image, slot) {
            Ok(toc) if &toc[..8] == b"TWOCHTOC" => {
                if stereo.replace(toc).is_some() {
                    return Err(SacdError::MalformedToc(
                        "disc declares more than one stereo area",
                    ));
                }
            }
            Ok(_) => {}
            Err(error) => {
                first_area_error.get_or_insert(error);
            }
        }
    }
    let toc = match stereo {
        Some(toc) => toc,
        None => return Err(first_area_error.unwrap_or(SacdError::NoStereoArea)),
    };
    let area = parse_stereo_area(&mut image, master, &toc)?;
    image.verify_unchanged()?;
    Ok(area)
}

fn read_area_slot(image: &mut SectorImage, slot: AreaSlot) -> Result<Vec<u8>, SacdError> {
    let mut first_error = None;
    let mut valid: Vec<(u32, Vec<u8>, Vec<u8>)> = Vec::new();
    for start in [slot.primary, slot.backup]
        .into_iter()
        .filter(|start| *start != 0)
    {
        let attempt = image.read_sectors(u64::from(start), usize::from(slot.size));
        match attempt {
            Ok(mut toc)
                if toc.len() >= SECTOR as usize
                    && (&toc[..8] == b"TWOCHTOC" || &toc[..8] == b"MULCHTOC") =>
            {
                let internal_size = be16(&toc, 10);
                if internal_size == 0 || internal_size > slot.size {
                    first_error.get_or_insert(SacdError::MalformedToc(
                        "area TOC internal size exceeds its Master TOC extent",
                    ));
                    continue;
                }
                toc.truncate(usize::from(internal_size) * SECTOR as usize);
                match area_geometry_key(&toc, image.sectors()) {
                    Ok(key) => valid.push((start, toc, key)),
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
            Ok(_) => {
                first_error.get_or_insert(SacdError::NotAnArea);
            }
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    let Some((_, selected, selected_key)) = valid.first() else {
        return Err(first_error.unwrap_or(SacdError::NotAnArea));
    };
    if valid.iter().skip(1).any(|(_, _, key)| key != selected_key) {
        return Err(SacdError::ConflictingAreaTocs);
    }
    Ok(selected.clone())
}

fn area_geometry_key(toc: &[u8], image_sectors: u64) -> Result<Vec<u8>, SacdError> {
    let version = (toc[8], toc[9]);
    if version.0 != 1 || version.1 > 20 {
        return Err(SacdError::MalformedToc("unsupported area TOC version"));
    }
    if toc[0x14] != 4 {
        return Err(SacdError::SampleFrequency(toc[0x14]));
    }
    if !matches!(toc[0x15] & 0x0f, 0 | 2 | 3) {
        return Err(SacdError::FrameFormat(toc[0x15] & 0x0f));
    }
    if toc[0x45] == 0 {
        return Err(SacdError::TrackCount(0));
    }
    let start = be32(toc, 0x48);
    let end = be32(toc, 0x4c);
    if start > end || u64::from(end) >= image_sectors {
        return Err(SacdError::MalformedToc(
            "audio area points outside the image",
        ));
    }
    let trl1 = find_toc_block(toc, b"SACDTRL1", None).ok_or(SacdError::MissingBlock("SACDTRL1"))?;
    let trl2 = find_toc_block(toc, b"SACDTRL2", None).ok_or(SacdError::MissingBlock("SACDTRL2"))?;
    let count = usize::from(toc[0x45]);
    const ARR2: usize = 8 + 255 * 4;
    let mut key = Vec::with_capacity(64 + count * 16);
    key.extend_from_slice(&toc[..12]);
    key.extend_from_slice(&toc[0x10..0x16]);
    key.extend_from_slice(&toc[0x20..0x22]);
    key.extend_from_slice(&toc[0x40..0x46]);
    key.extend_from_slice(&toc[0x48..0x51]);
    key.extend_from_slice(&toc[0x80..0x86]);
    key.extend_from_slice(&trl1[8..8 + count * 4]);
    key.extend_from_slice(&trl1[ARR2..ARR2 + count * 4]);
    key.extend_from_slice(&trl2[8..8 + count * 4]);
    key.extend_from_slice(&trl2[ARR2..ARR2 + count * 4]);
    let mut previous = None;
    for i in 0..count {
        let track_start = be32(trl1, 8 + i * 4);
        let length = be32(trl1, ARR2 + i * 4);
        let track_end = u64::from(track_start)
            .checked_add(u64::from(length))
            .ok_or(SacdError::MalformedToc("track sector range overflows"))?;
        if length == 0
            || track_start < start
            || track_end > u64::from(end) + 1
            || previous.is_some_and(|prior| track_start <= prior)
            || trl2[8 + i * 4 + 1] >= 60
            || trl2[8 + i * 4 + 2] >= 75
            || trl2[ARR2 + i * 4 + 1] >= 60
            || trl2[ARR2 + i * 4 + 2] >= 75
        {
            return Err(SacdError::MalformedToc("track table is invalid"));
        }
        previous = Some(track_start);
    }
    Ok(key)
}

fn parse_stereo_area(
    image: &mut SectorImage,
    master: MasterToc,
    toc: &[u8],
) -> Result<SacdArea, SacdError> {
    let channels = toc[0x20];
    if channels != 2 || toc[0x21] & 0x1f != 0 {
        return Err(SacdError::Channels(channels));
    }
    if toc[0x14] != 4 {
        return Err(SacdError::SampleFrequency(toc[0x14]));
    }
    let encoding = match toc[0x15] & 0x0f {
        0 => SacdEncoding::Dst,
        2 => SacdEncoding::Dsd3In14,
        3 => SacdEncoding::Dsd3In16,
        other => return Err(SacdError::FrameFormat(other)),
    };
    let track_count = toc[0x45];
    if track_count == 0 {
        return Err(SacdError::TrackCount(track_count));
    }
    let total_frames = time_frames_at(toc, 0x40);
    if toc[0x41] >= 60 || toc[0x42] >= 75 {
        return Err(SacdError::MalformedToc("invalid area playtime"));
    }
    let track_start_lsn = be32(toc, 0x48);
    let track_end_lsn = be32(toc, 0x4c);
    if track_start_lsn > track_end_lsn || u64::from(track_end_lsn) >= image.sectors() {
        return Err(SacdError::MalformedToc(
            "audio area points outside the image",
        ));
    }

    let trl1 = find_toc_block(toc, b"SACDTRL1", None).ok_or(SacdError::MissingBlock("SACDTRL1"))?;
    let trl2 = find_toc_block(toc, b"SACDTRL2", None).ok_or(SacdError::MissingBlock("SACDTRL2"))?;
    const ARR2: usize = 8 + 255 * 4;

    let text_offset = be16(toc, 0x80);
    let text_charset = toc[0x5a];
    let titles = find_toc_block(toc, b"SACDTTxt", (text_offset != 0).then_some(text_offset))
        .map(|block| read_titles(block, track_count as usize, text_charset))
        .unwrap_or_else(|| vec![None; track_count as usize]);

    let mut tracks = Vec::with_capacity(track_count as usize);
    let mut previous_start = None;
    for i in 0..track_count as usize {
        let start_lsn = be32(trl1, 8 + i * 4);
        let length_lsn = be32(trl1, ARR2 + i * 4);
        let start_frame = time_frames_at(trl2, 8 + i * 4);
        let duration_frame = time_frames_at(trl2, ARR2 + i * 4);
        if trl2[8 + i * 4 + 1] >= 60
            || trl2[8 + i * 4 + 2] >= 75
            || trl2[ARR2 + i * 4 + 1] >= 60
            || trl2[ARR2 + i * 4 + 2] >= 75
        {
            return Err(SacdError::MalformedToc("invalid track timecode"));
        }
        let end = u64::from(start_lsn)
            .checked_add(u64::from(length_lsn))
            .ok_or(SacdError::MalformedToc("track sector range overflows"))?;
        if length_lsn == 0
            || start_lsn < track_start_lsn
            || u64::from(start_lsn) > u64::from(track_end_lsn)
            || end > u64::from(track_end_lsn) + 1
            || previous_start.is_some_and(|previous| start_lsn <= previous)
        {
            return Err(SacdError::MalformedToc("track sector range is invalid"));
        }
        previous_start = Some(start_lsn);
        tracks.push(SacdTrack {
            number: (i + 1) as u8,
            start_lsn,
            length_lsn,
            start_secs: start_frame as f64 / 75.0,
            duration_secs: duration_frame as f64 / 75.0,
            start_frame,
            duration_frames: duration_frame,
            encoding,
            title: titles.get(i).cloned().flatten(),
        });
    }

    let (album, artist) = read_master_text(image, master).unwrap_or((None, None));
    Ok(SacdArea {
        track_start_lsn,
        track_end_lsn,
        channels,
        encoding,
        total_playtime_secs: total_frames as f64 / 75.0,
        tracks,
        album,
        artist,
    })
}

fn find_toc_block<'a>(
    toc: &'a [u8],
    signature: &[u8; 8],
    declared: Option<u16>,
) -> Option<&'a [u8]> {
    let at_sector = |relative: usize| -> Option<&'a [u8]> {
        let at = relative.checked_mul(SECTOR as usize)?;
        let block = toc.get(at..)?;
        if block.len() < SECTOR as usize {
            return None;
        }
        (&block[..8] == signature).then_some(block)
    };
    if let Some(relative) = declared {
        return at_sector(usize::from(relative));
    }
    (0..toc.len() / SECTOR as usize).find_map(at_sector)
}

/// Album title and artist out of the Master TOC's text sector.
///
/// A SACD names itself, which is why this feature needs no network the way a
/// CD does. Measured layout, identical on both of the owner's discs:
///   LSN 510      "SACDMTOC"      (the Master TOC; backups at 520 and 530 are
///                                 byte-identical, so a failure can fall back)
///   LSN 511      "SACDText"      the text sector
///     +0x10  u16 BE  album title pointer   } offsets are relative to the
///     +0x12  u16 BE  album artist pointer  } START OF THE TEXT SECTOR
///   payload      NUL-terminated strings
///
/// Everything here is optional: an image whose Master TOC is unreadable or
/// carries no text still plays, it just falls back to the file name.
fn read_master_text(
    image: &mut SectorImage,
    master: MasterToc,
) -> Option<(Option<String>, Option<String>)> {
    if master.text_area_count == 0 {
        return None;
    }
    let text = image.read_sectors(master.lsn + 1, 1).ok()?;
    if &text[0..8] != b"SACDText" {
        return None;
    }
    let ptr = |at: usize| -> Option<usize> {
        let v = u16::from_be_bytes([text[at], text[at + 1]]) as usize;
        // Zero means "absent", not "offset zero" — offset zero is the id.
        (v != 0 && v < text.len()).then_some(v)
    };
    let string_at = |off: usize| -> Option<String> {
        let end = text[off..].iter().position(|b| *b == 0)? + off;
        decode_sacd_text(&text[off..end], master.text_charset, "Master TOC text")
    };
    Some((ptr(0x10).and_then(string_at), ptr(0x12).and_then(string_at)))
}

/// Track titles from one validated `SACDTTxt` sector. The block's pointer
/// table is relative to the block, and each pointed record can contain several
/// typed strings; type 1 is the title.
fn read_titles(block: &[u8], count: usize, charset: u8) -> Vec<Option<String>> {
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let at = 8 + i * 2;
        let ptr = match block.get(at..at + 2) {
            Some(p) => u16::from_be_bytes([p[0], p[1]]) as usize,
            None => {
                out.push(None);
                continue;
            }
        };
        out.push(track_title_at(block, ptr, charset, i + 1));
    }
    out
}

fn track_title_at(block: &[u8], ptr: usize, charset: u8, track: usize) -> Option<String> {
    let amount = *block.get(ptr)? as usize;
    let mut at = ptr.checked_add(4)?;
    for _ in 0..amount {
        let kind = *block.get(at)?;
        at = at.checked_add(2)?;
        let tail = block.get(at..)?;
        let len = tail.iter().position(|byte| *byte == 0)?;
        if kind == 1 {
            return decode_sacd_text(&tail[..len], charset, &format!("track {track} title"));
        }
        at = at.checked_add(len)?;
        while block.get(at).is_some_and(|byte| *byte == 0) {
            at += 1;
        }
    }
    None
}

/// Convert one bounded Scarlet Book string to UTF-8. Invalid input degrades
/// only that optional field: geometry and playback remain usable, but no
/// replacement characters are invented and a warning records the fallback.
fn decode_sacd_text(raw: &[u8], charset: u8, field: &str) -> Option<String> {
    use encoding_rs::{BIG5, EUC_KR, GBK, SHIFT_JIS};

    let code = charset & 0x07;
    let decoded = match code {
        // Unknown is not permission to guess. ISO 646 is 7-bit; reject a
        // high byte rather than laundering it through UTF-8 replacement.
        0 => None,
        1 => raw
            .iter()
            .all(|byte| byte.is_ascii())
            .then(|| String::from_utf8(raw.to_vec()).expect("ASCII is UTF-8")),
        // ISO-8859-1 maps every byte directly to the same Unicode scalar.
        // Code 7 permits escape sequences; the common no-escape form is the
        // same mapping, while an actual ESC requires state we do not guess.
        2 => Some(raw.iter().map(|byte| char::from(*byte)).collect()),
        3 => SHIFT_JIS
            .decode_without_bom_handling_and_without_replacement(raw)
            .map(|value| value.into_owned()),
        4 => EUC_KR
            .decode_without_bom_handling_and_without_replacement(raw)
            .map(|value| value.into_owned()),
        5 => GBK
            .decode_without_bom_handling_and_without_replacement(raw)
            .map(|value| value.into_owned()),
        6 => BIG5
            .decode_without_bom_handling_and_without_replacement(raw)
            .map(|value| value.into_owned()),
        7 if !raw.contains(&0x1b) => Some(raw.iter().map(|byte| char::from(*byte)).collect()),
        7 => None,
        _ => unreachable!("charset code is masked to three bits"),
    };
    let Some(text) = decoded else {
        log::warn!("[sacd] ignoring {field}: unknown charset {code} or invalid byte sequence");
        return None;
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

// ---------------------------------------------------------------------------
// The sector parser
// ---------------------------------------------------------------------------

/// One packet of a Scarlet Book audio sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    pub frame_start: bool,
    pub data_type: u16,
    pub len: usize,
    /// Offset of the payload within the 2048-byte sector.
    pub at: usize,
    /// Sector header bit. DST sectors carry four-byte frame-info records;
    /// uncompressed DSD sectors carry three-byte records.
    pub dst_encoded: bool,
    /// For a DST frame-start packet, the declared number of sectors occupied
    /// by this frame. Absent for continuation and uncompressed packets.
    pub frame_sector_count: Option<u8>,
    /// Absolute area timecode carried by the matching frame-info record.
    pub frame_timecode: Option<u32>,
    /// Channel count encoded in a DST frame-info record.
    pub frame_channels: Option<u8>,
}

/// Decode one sector's framing.
///
/// Measured layout, census of 3 066 893 sectors with zero exceptions:
///   byte 0:  packet_count     = (b >> 5) & 0x07
///            frame_info_count = (b >> 2) & 0x07
///   then `packet_count` BIG-ENDIAN 16-bit descriptors:
///            bit 15    frame_start
///            bits 14-11 data_type
///            bits 10-0  length IN BYTES        <- ELEVEN bits, not twelve
///   then `frame_info_count` * (DST ? 4 : 3) bytes
///
/// The eleven-bit length is the correction that matters. The widely-copied
/// layout gives the length twelve bits, and the very first descriptor on both
/// discs is 0x181B — whose low twelve bits are 2075, larger than the sector.
/// With eleven bits it is 27, and 5 + 27 + 2016 = 2048 exactly. The sum rule
/// `header_len + SUM(lengths) == 2048` is what proves the whole decoding, and
/// it is checked here on every sector rather than trusted.
pub fn parse_sector(sector: &[u8], lsn: u64) -> Result<Vec<Packet>, SacdError> {
    if sector.len() != SECTOR as usize {
        return Err(SacdError::BadSector {
            lsn,
            why: "not 2048 bytes",
        });
    }
    let b0 = sector[0];
    let dst_encoded = b0 & 0x01 != 0;
    let packet_count = ((b0 >> 5) & 0x07) as usize;
    let frame_info_count = ((b0 >> 2) & 0x07) as usize;
    let frame_info_size = if dst_encoded { 4 } else { 3 };
    let frame_info_at = 1 + packet_count * 2;
    let header_len = frame_info_at + frame_info_count * frame_info_size;
    if packet_count == 0 || header_len > sector.len() {
        return Err(SacdError::BadSector {
            lsn,
            why: "impossible packet/frame counts",
        });
    }

    let mut packets = Vec::with_capacity(packet_count);
    let mut at = header_len;
    let mut total = header_len;
    let mut frame_info = 0usize;
    for k in 0..packet_count {
        let w = u16::from_be_bytes([sector[1 + 2 * k], sector[2 + 2 * k]]);
        let len = (w & 0x07FF) as usize;
        let frame_start = w & 0x8000 != 0;
        let data_type = (w >> 11) & 0x07;
        // Lead-out sectors on real discs mark their final padding packet as a
        // frame start and pair it with an all-0xff sentinel frame-info row.
        // The row still counts toward `frame_info_count`, but it has no audio
        // semantics and its sentinel timecode must not be validated.
        let info = if frame_start {
            let info = frame_info_at + frame_info * frame_info_size;
            frame_info += 1;
            Some(info)
        } else {
            None
        };
        let (frame_sector_count, frame_timecode, frame_channels) =
            if let Some(info) = info.filter(|_| data_type == DATA_TYPE_AUDIO) {
                let time = sector.get(info..info + 3).ok_or(SacdError::BadSector {
                    lsn,
                    why: "frame-info table is truncated",
                })?;
                if time[1] >= 60 || time[2] >= 75 {
                    return Err(SacdError::BadSector {
                        lsn,
                        why: "frame-info timecode is invalid",
                    });
                }
                let timecode = (time[0] as u32 * 60 + time[1] as u32) * 75 + time[2] as u32;
                if dst_encoded {
                    let flags = sector[info + 3];
                    let channels = match (flags & 0x02 != 0, flags & 0x01 != 0) {
                        (true, false) => 6,
                        (false, true) => 5,
                        _ => 2,
                    };
                    (Some((flags >> 2) & 0x1f), Some(timecode), Some(channels))
                } else {
                    (None, Some(timecode), Some(2))
                }
            } else {
                (None, None, None)
            };
        packets.push(Packet {
            frame_start,
            data_type,
            len,
            at,
            dst_encoded,
            frame_sector_count,
            frame_timecode,
            frame_channels,
        });
        at += len;
        total += len;
    }
    // Flat DSD fills the sector exactly. Real DST authoring also permits an
    // undeclared zero tail after the last packet (1..6 bytes commonly, and a
    // larger zero tail when a compressed frame ends early). It is alignment,
    // not audio: accept only all-zero DST tails and never publish them.
    if total > SECTOR as usize
        || (total < SECTOR as usize
            && (!dst_encoded || sector[total..].iter().any(|byte| *byte != 0)))
    {
        return Err(SacdError::BadSector {
            lsn,
            why: "packet lengths do not account for the sector",
        });
    }
    if frame_info != frame_info_count {
        return Err(SacdError::BadSector {
            lsn,
            why: "frame-info count does not match frame starts",
        });
    }
    Ok(packets)
}

pub const MAX_DST_FRAME: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SacdFrameKind {
    Dsd,
    Dst,
}

/// One complete, container-validated 1/75-second audio unit. DST remains
/// compressed here; decoding belongs to qbz-dsd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SacdAudioFrame {
    pub kind: SacdFrameKind,
    pub payload: Vec<u8>,
    pub start_lsn: u64,
    pub timecode_frame: u32,
}

struct BuildingFrame {
    frame: SacdAudioFrame,
    dst_packets_left: Option<u8>,
}

/// Assemble exact Scarlet Book frames inside one track window. The reader
/// uses TRL2 timecodes as the publication boundary, so a sector shared by two
/// tracks neither duplicates nor loses its boundary frame.
pub struct SacdFrameReader {
    image: SectorImage,
    track: u8,
    encoding: SacdEncoding,
    start_lsn: u64,
    next_lsn: u64,
    end_lsn: u64,
    track_start_frame: u32,
    track_end_frame: u32,
    target_frame: u32,
    emitted: u32,
    current: Option<BuildingFrame>,
    ready: std::collections::VecDeque<SacdAudioFrame>,
    search_started_at: u64,
    searched_sectors: u8,
    saw_frame_start: bool,
}

impl SacdFrameReader {
    pub fn open(path: &Path, track: &SacdTrack) -> Result<Self, SacdError> {
        let image = SectorImage::open(path)?;
        image.validate_length()?;
        let start_lsn = u64::from(track.start_lsn);
        let end_lsn = start_lsn
            .checked_add(u64::from(track.length_lsn))
            .ok_or(SacdError::MalformedToc("track sector range overflows"))?;
        if end_lsn > image.sectors() {
            return Err(SacdError::MalformedToc("track points outside the image"));
        }
        let track_end_frame = track
            .start_frame
            .checked_add(track.duration_frames)
            .ok_or(SacdError::MalformedToc("track timecode overflows"))?;
        Ok(Self {
            image,
            track: track.number,
            encoding: track.encoding,
            start_lsn,
            next_lsn: start_lsn,
            end_lsn,
            track_start_frame: track.start_frame,
            track_end_frame,
            target_frame: track.start_frame,
            emitted: 0,
            current: None,
            ready: std::collections::VecDeque::new(),
            search_started_at: start_lsn,
            searched_sectors: 0,
            saw_frame_start: false,
        })
    }

    pub fn finished(&self) -> bool {
        self.target_frame.saturating_add(self.emitted) >= self.track_end_frame
    }

    pub fn seek_to_fraction(
        &mut self,
        offset_units: u64,
        total_units: u64,
    ) -> Result<(), SacdError> {
        let duration = self.track_end_frame - self.track_start_frame;
        let frame_offset = if total_units == 0 {
            0
        } else {
            ((u128::from(duration) * u128::from(offset_units.min(total_units)))
                / u128::from(total_units)) as u32
        };
        self.target_frame = self.track_start_frame.saturating_add(frame_offset);
        self.emitted = 0;
        self.current = None;
        self.ready.clear();
        if self.target_frame >= self.track_end_frame {
            self.next_lsn = self.end_lsn;
        } else {
            // DST is variable-rate: a proportional byte/sector estimate can
            // be thousands of sectors past the requested time even though it
            // looks convincing on constant-rate DSD. Binary-search the
            // monotonic on-disc frame timecodes instead. A probe looks ahead
            // at most the format's 31-sector DST-frame bound; the final
            // 32-sector backoff guarantees the requested frame is reacquired
            // whole rather than entered through a continuation packet.
            let mut low = self.start_lsn;
            let mut high = self.end_lsn;
            while high.saturating_sub(low) > 1 {
                let middle = low + (high - low) / 2;
                match self.frame_at_or_after(middle)? {
                    Some((_, timecode)) if timecode < self.target_frame => low = middle + 1,
                    _ => high = middle,
                }
            }
            self.next_lsn = low.saturating_sub(32).max(self.start_lsn);
        }
        self.search_started_at = self.next_lsn;
        self.searched_sectors = 0;
        self.saw_frame_start = false;
        Ok(())
    }

    fn frame_at_or_after(&mut self, start_lsn: u64) -> Result<Option<(u64, u32)>, SacdError> {
        let stop = start_lsn.saturating_add(32).min(self.end_lsn);
        for lsn in start_lsn..stop {
            let raw = self.image.read_sectors(lsn, 1)?;
            let packets = parse_sector(&raw, lsn)?;
            let sector_dst = packets.first().is_some_and(|packet| packet.dst_encoded);
            if sector_dst != (self.encoding == SacdEncoding::Dst) {
                return Err(SacdError::BadSector {
                    lsn,
                    why: "sector encoding disagrees with the area TOC",
                });
            }
            if let Some(packet) = packets
                .iter()
                .find(|packet| packet.data_type == DATA_TYPE_AUDIO && packet.frame_start)
            {
                if packet.frame_channels != Some(2) {
                    return Err(self.bad_frame(lsn, "frame is not stereo"));
                }
                return packet
                    .frame_timecode
                    .map(|timecode| Some((lsn, timecode)))
                    .ok_or(SacdError::BadSector {
                        lsn,
                        why: "frame start has no timecode",
                    });
            }
        }
        Ok(None)
    }

    pub fn next_frame(&mut self) -> Result<Option<SacdAudioFrame>, SacdError> {
        if self.finished() {
            self.image.verify_unchanged()?;
            return Ok(None);
        }
        loop {
            if let Some(frame) = self.ready.pop_front() {
                if frame.timecode_frame < self.target_frame {
                    continue;
                }
                let expected = self.target_frame.saturating_add(self.emitted);
                if frame.timecode_frame != expected {
                    return Err(self.bad_frame(
                        frame.start_lsn,
                        "frame timecode is discontinuous or seek overshot its target",
                    ));
                }
                self.emitted += 1;
                return Ok(Some(frame));
            }
            if self.next_lsn >= self.end_lsn {
                self.finish_current()?;
                if self.ready.is_empty() {
                    return Err(
                        self.bad_frame(self.end_lsn, "track ended before its TRL2 duration")
                    );
                }
                continue;
            }
            self.read_next_sector()?;
        }
    }

    fn read_next_sector(&mut self) -> Result<(), SacdError> {
        let lsn = self.next_lsn;
        let raw = self.image.read_sectors(lsn, 1)?;
        self.next_lsn += 1;
        let packets = parse_sector(&raw, lsn)?;
        let sector_dst = packets.first().is_some_and(|packet| packet.dst_encoded);
        if sector_dst != (self.encoding == SacdEncoding::Dst) {
            return Err(SacdError::BadSector {
                lsn,
                why: "sector encoding disagrees with the area TOC",
            });
        }
        self.searched_sectors = self.searched_sectors.saturating_add(1);
        for packet in packets {
            if packet.data_type != DATA_TYPE_AUDIO {
                continue;
            }
            if packet.frame_start {
                self.saw_frame_start = true;
                self.finish_current()?;
                let timecode_frame = packet.frame_timecode.ok_or(SacdError::BadSector {
                    lsn,
                    why: "frame start has no timecode",
                })?;
                if packet.frame_channels != Some(2) {
                    return Err(self.bad_frame(lsn, "frame is not stereo"));
                }
                let dst_packets_left = if sector_dst {
                    match packet.frame_sector_count {
                        Some(1..=31) => packet.frame_sector_count,
                        _ => return Err(self.bad_frame(lsn, "invalid DST sector count")),
                    }
                } else {
                    None
                };
                self.current = Some(BuildingFrame {
                    frame: SacdAudioFrame {
                        kind: if sector_dst {
                            SacdFrameKind::Dst
                        } else {
                            SacdFrameKind::Dsd
                        },
                        payload: Vec::with_capacity(if sector_dst {
                            MAX_DST_FRAME.min(16 * 1024)
                        } else {
                            DSD64_STEREO_FRAME as usize
                        }),
                        start_lsn: lsn,
                        timecode_frame,
                    },
                    dst_packets_left,
                });
            }
            let Some(building) = self.current.as_mut() else {
                continue;
            };
            if let Some(left) = building.dst_packets_left.as_mut() {
                if *left == 0 {
                    return Err(self.bad_frame(lsn, "DST sector count ended before the frame"));
                }
                *left -= 1;
            }
            let end = packet.at + packet.len;
            building
                .frame
                .payload
                .extend_from_slice(&raw[packet.at..end]);
            if building.frame.payload.len() > MAX_DST_FRAME {
                return Err(self.bad_frame(lsn, "audio frame exceeds 64 KiB"));
            }
        }
        if !self.saw_frame_start && self.searched_sectors >= 32 {
            return Err(SacdError::MissingFrameStart {
                track: self.track,
                lsn: self.search_started_at,
            });
        }
        Ok(())
    }

    fn finish_current(&mut self) -> Result<(), SacdError> {
        let Some(building) = self.current.take() else {
            return Ok(());
        };
        let valid = match building.frame.kind {
            SacdFrameKind::Dsd => building.frame.payload.len() == DSD64_STEREO_FRAME as usize,
            SacdFrameKind::Dst => building.dst_packets_left == Some(0),
        };
        if !valid {
            return Err(self.bad_frame(building.frame.start_lsn, "frame closed at the wrong size"));
        }
        if building.frame.timecode_frame >= self.target_frame
            && building.frame.timecode_frame < self.track_end_frame
        {
            self.ready.push_back(building.frame);
        }
        Ok(())
    }

    fn bad_frame(&self, lsn: u64, why: &'static str) -> SacdError {
        SacdError::BadAudioFrame {
            track: self.track,
            lsn,
            why,
        }
    }
}

/// Streams the raw DSD of one track out of an image.
/// Where the first DSD frame is taken to begin.
///
/// MEASURED on the owner's Rheingold, both discs, every track: audio frames
/// are 9408 bytes and spaced exactly 9408 apart, but the FIRST one does not
/// begin at the start of a track's payload. It begins at 0, 672 or 1344 bytes
/// in, depending on the track — the packet descriptors carry a `frame_start`
/// bit precisely so a reader can find it.
///
/// Ignoring that bit puts every 9408-byte window across two real frames, so
/// each channel is assembled from the tail of one and the head of the next.
/// It plays; it plays as noise with the music audible behind it, in EVERY
/// delivery mode, because the damage is done before PCM/DoP/native is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSync {
    /// Skip audio bytes until the first packet that declares a frame start.
    /// The correct reading, and what playback uses.
    ToFirstFrame,
    /// Take the payload as it comes. Kept because it is what shipped, and
    /// because a probe has to be able to reproduce the broken stream to show
    /// the difference.
    FromPayloadStart,
}

pub struct SacdTrackReader {
    image: SectorImage,
    start_lsn: u64,
    next_lsn: u64,
    end_lsn: u64,
    /// Set once the first frame boundary has been found; until then audio
    /// bytes are DROPPED rather than emitted. It IS the sync mode after
    /// construction — `FromPayloadStart` simply starts out true — so the mode
    /// itself is not kept around to be read twice.
    synced: bool,
    expected_dst: bool,
}

impl SacdTrackReader {
    pub fn open(path: &Path, track: &SacdTrack) -> Result<Self, SacdError> {
        Self::open_with(path, track, FrameSync::ToFirstFrame)
    }

    pub fn open_with(path: &Path, track: &SacdTrack, sync: FrameSync) -> Result<Self, SacdError> {
        let start_lsn = track.start_lsn as u64;
        let image = SectorImage::open(path)?;
        image.validate_length()?;
        let end_lsn = start_lsn
            .checked_add(u64::from(track.length_lsn))
            .ok_or(SacdError::MalformedToc("track sector range overflows"))?;
        if end_lsn > image.sectors() {
            return Err(SacdError::MalformedToc("track points outside the image"));
        }
        Ok(Self {
            image,
            start_lsn,
            next_lsn: start_lsn,
            // `length_lsn` is how many sectors to read, including the shared
            // boundary sector — the disc's own accounting, not ours.
            end_lsn,
            synced: matches!(sync, FrameSync::FromPayloadStart),
            expected_dst: track.encoding == SacdEncoding::Dst,
        })
    }

    pub fn finished(&self) -> bool {
        self.next_lsn >= self.end_lsn
    }

    /// Reposition proportionally inside the track and re-acquire the next
    /// declared DSD frame boundary. Mapping against the TOC's sector span is
    /// important: 75 Hz is the DSD audio-frame cadence, not the ISO sector
    /// cadence (several sectors contribute to each audio frame).
    pub fn seek_to_fraction(&mut self, offset_units: u64, total_units: u64) {
        let span = self.end_lsn - self.start_lsn;
        let sector_offset = if total_units == 0 {
            0
        } else {
            ((span as u128 * offset_units.min(total_units) as u128) / total_units as u128) as u64
        };
        self.next_lsn = self.start_lsn + sector_offset;
        self.synced = false;
    }

    /// Append the next chunk of AUDIO payload to `out`, skipping every
    /// non-audio packet — and, until the first frame boundary is found, every
    /// audio byte too (see [`FrameSync`]). Returns bytes appended.
    ///
    /// Zero appended does NOT mean the track is done while the reader is still
    /// hunting for its first frame: the caller must ask [`Self::finished`].
    ///
    /// Supplementary and padding packets (`data_type` 3 and 7 on these discs)
    /// are dropped rather than passed through: they are not audio, and a DSD
    /// stream with them in it is noise.
    pub fn next_chunk(&mut self, out: &mut Vec<u8>, sectors: usize) -> Result<usize, SacdError> {
        out.clear();
        if self.finished() {
            return Ok(0);
        }
        let want = sectors.min((self.end_lsn - self.next_lsn) as usize);
        let raw = self.image.read_sectors(self.next_lsn, want)?;
        for s in 0..want {
            let lsn = self.next_lsn + s as u64;
            let sector = &raw[s * SECTOR as usize..(s + 1) * SECTOR as usize];
            let packets = parse_sector(sector, lsn)?;
            if packets
                .first()
                .is_some_and(|packet| packet.dst_encoded != self.expected_dst)
            {
                return Err(SacdError::BadSector {
                    lsn,
                    why: "sector encoding disagrees with the area TOC",
                });
            }
            if self.expected_dst {
                return Err(SacdError::Dst);
            }
            for p in packets {
                if p.data_type != DATA_TYPE_AUDIO {
                    continue;
                }
                // Everything before the first declared frame start belongs to
                // the previous track's last frame — the areas share boundary
                // sectors — and emitting it shifts the whole stream.
                if !self.synced {
                    if !p.frame_start {
                        continue;
                    }
                    self.synced = true;
                    log::debug!("[sacd] frame sync at lsn {lsn} offset {}", p.at);
                }
                out.extend_from_slice(&sector[p.at..p.at + p.len]);
            }
        }
        self.next_lsn += want as u64;
        if self.finished() {
            self.image.verify_unchanged()?;
        }
        Ok(out.len())
    }
}

#[cfg(test)]
#[path = "sacd_raw_tests.rs"]
mod raw_sector_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a sector from descriptors read off the owner's Disc 1.
    fn sector(b0: u8, descs: &[u16]) -> Vec<u8> {
        let mut s = vec![0u8; 2048];
        s[0] = b0;
        for (k, w) in descs.iter().enumerate() {
            s[1 + 2 * k..3 + 2 * k].copy_from_slice(&w.to_be_bytes());
        }
        s
    }

    /// The first three DISTINCT sector shapes of Disc 1's audio area, with the
    /// exact descriptor words read off the image at LSN 1253 / 1254 / 1257.
    /// Each one adds up to 2048 — which IS the proof that the decoding is
    /// right, so the test asserts the sum rather than trusting the parser.
    #[test]
    fn the_three_real_sector_shapes_decode_and_fill_exactly() {
        // LSN 1253: 0x44 -> header 8, then (type 3, 24) + (type 2, 2016, start)
        let p = parse_sector(&sector(0x44, &[0x1818, 0x97E0]), 1253).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].data_type, p[0].len, p[0].at), (3, 24, 8));
        assert_eq!((p[1].data_type, p[1].len), (DATA_TYPE_AUDIO, 2016));
        assert!(p[1].frame_start, "0x97E0 has bit 15 set");
        assert_eq!(8 + 24 + 2016, 2048);

        // LSN 1254: 0x40 -> header 5, then (type 3, 27) + (type 2, 2016)
        let p = parse_sector(&sector(0x40, &[0x181B, 0x17E0]), 1254).unwrap();
        assert_eq!((p[0].len, p[0].at), (27, 5));
        assert!(
            !p[0].frame_start && !p[1].frame_start,
            "0x40 declares no frame info"
        );
        assert_eq!(5 + 27 + 2016, 2048);

        // LSN 1257: 0x64 -> header 10, three packets, the last starting a frame
        let p = parse_sector(&sector(0x64, &[0x1816, 0x1540, 0x92A0]), 1257).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!((p[0].len, p[1].len, p[2].len), (22, 1344, 672));
        assert!(p[2].frame_start);
        assert_eq!(10 + 22 + 1344 + 672, 2048);
    }

    #[test]
    fn an_eleven_bit_length_is_what_makes_a_sector_add_up() {
        // 0x181B, read off LSN 1254. Twelve bits give a length larger than the
        // sector it lives in, which is how the widely-copied layout refutes
        // itself the first time anyone checks it against a disc.
        let w = 0x181Bu16;
        assert_eq!((w & 0x0FFF) as usize, 2075, "the 12-bit reading");
        assert!(2075 > 2048, "cannot fit in a sector — hence 11 bits");
        assert_eq!((w & 0x07FF) as usize, 27);
    }

    #[test]
    fn the_three_first_bytes_that_exist_give_the_three_header_lengths() {
        for (b0, pc, fic, header) in [(0x40u8, 2, 0, 5), (0x44, 2, 1, 8), (0x64, 3, 1, 10)] {
            assert_eq!(((b0 >> 5) & 0x07) as usize, pc, "packet count of {b0:#04x}");
            assert_eq!(((b0 >> 2) & 0x07) as usize, fic, "frame infos of {b0:#04x}");
            assert_eq!(1 + pc * 2 + fic * 3, header);
        }
    }

    #[test]
    fn a_dst_sector_uses_four_byte_frame_info_and_exposes_its_boundary() {
        let mut raw = sector(0x25, &[0x97f9]);
        raw[3..7].copy_from_slice(&[1, 2, 3, 0x04]);
        let packets = parse_sector(&raw, 77).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].at, 7);
        assert_eq!(packets[0].len, 2041);
        assert!(packets[0].dst_encoded);
        assert_eq!(packets[0].frame_sector_count, Some(1));
        assert_eq!(packets[0].frame_timecode, Some((60 + 2) * 75 + 3));
        assert_eq!(packets[0].frame_channels, Some(2));
    }

    #[test]
    fn frame_info_count_must_equal_the_number_of_starts() {
        let raw = sector(0x25, &[0x17f9]);
        assert!(matches!(
            parse_sector(&raw, 78),
            Err(SacdError::BadSector { .. })
        ));
    }

    #[test]
    fn a_lead_out_padding_start_consumes_but_does_not_validate_its_sentinel_info() {
        // Exact shape of the last Bowie and Ritchie area sector: a normal
        // supplementary packet followed by padding whose frame-start bit is
        // paired with an all-0xff sentinel rather than an audio timecode.
        let mut raw = sector(0x44, &[0x1818, 0xbfe0]);
        raw[5..8].fill(0xff);
        let packets = parse_sector(&raw, 959_402).unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[1].data_type, 7, "Scarlet Book padding");
        assert!(packets[1].frame_start);
        assert_eq!(packets[1].frame_timecode, None);
        assert_eq!(packets[1].at + packets[1].len, SECTOR as usize);
    }

    #[test]
    fn dst_zero_fill_is_alignment_not_audio() {
        // Fidelio Disc 2 LSN 969 has this exact header: one 2041-byte audio
        // continuation followed by four zero alignment bytes.
        let raw = sector(0x21, &[0x17f9]);
        let packets = parse_sector(&raw, 969).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!((packets[0].at, packets[0].len), (3, 2041));
        assert_eq!(packets[0].at + packets[0].len, 2044);

        let mut nonzero_tail = raw;
        nonzero_tail[2047] = 1;
        assert!(matches!(
            parse_sector(&nonzero_tail, 969),
            Err(SacdError::BadSector { .. })
        ));
    }

    #[test]
    fn scarlet_book_text_is_converted_without_replacement_characters() {
        assert_eq!(
            decode_sacd_text(b"Ouvert\xfcre", 2, "test"),
            Some("Ouvertüre".to_string())
        );
        assert_eq!(
            decode_sacd_text(b"plain ASCII", 1, "test").as_deref(),
            Some("plain ASCII")
        );
        assert_eq!(decode_sacd_text(b"bad\xff", 1, "test"), None);
        assert_eq!(decode_sacd_text(b"do not guess", 0, "test"), None);
        assert_eq!(decode_sacd_text(b"escape\x1bsequence", 7, "test"), None);
    }

    #[test]
    fn a_changed_image_is_detected_before_more_data_is_trusted() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("qbz-sacd-change-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("changed.iso");
        std::fs::write(&path, vec![0u8; SECTOR as usize]).unwrap();
        let image = SectorImage::open(&path).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[0u8; 1])
            .unwrap();
        assert!(matches!(
            image.verify_unchanged(),
            Err(SacdError::ImageChangedDuringRead(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_partial_sector_has_its_own_error_category() {
        let dir = std::env::temp_dir().join(format!("qbz-sacd-length-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.iso");
        std::fs::write(&path, [0u8; 1]).unwrap();
        assert!(matches!(
            read_area(&path),
            Err(SacdError::InvalidImageLength(1))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dst_track_boundaries_are_taken_from_timecodes_not_shared_sectors() {
        use std::io::{Seek, SeekFrom, Write};

        let dir = std::env::temp_dir().join(format!("qbz-sacd-frame-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frames.iso");
        let mut file = std::fs::File::create(&path).unwrap();
        for (lsn, timecode) in [(1u64, [0u8, 0, 0]), (2, [0, 0, 1])] {
            let mut raw = sector(0x25, &[0x97f9]);
            raw[3..6].copy_from_slice(&timecode);
            raw[6] = 0x04;
            file.seek(SeekFrom::Start(lsn * SECTOR)).unwrap();
            file.write_all(&raw).unwrap();
        }
        let track = SacdTrack {
            number: 1,
            start_lsn: 1,
            length_lsn: 2,
            start_secs: 0.0,
            duration_secs: 1.0 / 75.0,
            start_frame: 0,
            duration_frames: 1,
            encoding: SacdEncoding::Dst,
            title: None,
        };
        let mut reader = SacdFrameReader::open(&path, &track).unwrap();
        let frame = reader.next_frame().unwrap().unwrap();
        assert_eq!(frame.kind, SacdFrameKind::Dst);
        assert_eq!(frame.timecode_frame, 0);
        assert_eq!(frame.start_lsn, 1);
        assert_eq!(frame.payload.len(), 2041);
        assert!(reader.next_frame().unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dst_seek_uses_timecodes_instead_of_assuming_a_constant_compression_ratio() {
        use std::io::{Seek, SeekFrom, Write};

        let dir = std::env::temp_dir().join(format!("qbz-sacd-seek-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("variable-rate.iso");
        let mut file = std::fs::File::create(&path).unwrap();
        let mut lsn = 1u64;
        let mut middle_lsn = 0u64;
        for frame in 0..100u32 {
            if frame == 50 {
                middle_lsn = lsn;
            }
            let sectors = if frame < 50 { 31u8 } else { 1u8 };
            let mut first = sector(0x25, &[0x97f9]);
            first[3..6].copy_from_slice(&[0, (frame / 75) as u8, (frame % 75) as u8]);
            first[6] = sectors << 2;
            file.seek(SeekFrom::Start(lsn * SECTOR)).unwrap();
            file.write_all(&first).unwrap();
            lsn += 1;
            for _ in 1..sectors {
                let continuation = sector(0x21, &[0x17fd]);
                file.seek(SeekFrom::Start(lsn * SECTOR)).unwrap();
                file.write_all(&continuation).unwrap();
                lsn += 1;
            }
        }
        let track = SacdTrack {
            number: 1,
            start_lsn: 1,
            length_lsn: (lsn - 1) as u32,
            start_secs: 0.0,
            duration_secs: 100.0 / 75.0,
            start_frame: 0,
            duration_frames: 100,
            encoding: SacdEncoding::Dst,
            title: None,
        };
        let mut reader = SacdFrameReader::open(&path, &track).unwrap();
        reader.seek_to_fraction(50, 100).unwrap();
        let frame = reader.next_frame().unwrap().unwrap();
        assert_eq!(frame.timecode_frame, 50);
        assert_eq!(frame.start_lsn, middle_lsn);

        reader.seek_to_fraction(99, 100).unwrap();
        assert_eq!(reader.next_frame().unwrap().unwrap().timecode_frame, 99);
        assert!(reader.next_frame().unwrap().is_none());

        reader.seek_to_fraction(100, 100).unwrap();
        assert!(reader.next_frame().unwrap().is_none());

        reader.seek_to_fraction(0, 100).unwrap();
        assert_eq!(reader.next_frame().unwrap().unwrap().timecode_frame, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sector_whose_lengths_do_not_fill_it_is_refused() {
        // Decoding one wrong is exactly how a fixed-size header skip corrupts
        // a DSD stream, so it must be an error and not a shrug.
        let s = sector(0x44, &[0x1818, 0x1000]); // second packet length 0
        assert!(matches!(
            parse_sector(&s, 1253),
            Err(SacdError::BadSector { .. })
        ));
    }

    #[test]
    fn the_start_lsn_and_start_time_identity_holds() {
        // Verified 44/44 on the owner's discs: this is the relation that
        // proves SECTORS_PER_SEC and ties the two TOC blocks together.
        // The four values below were read out of SACDTRL1 and SACDTRL2 on
        // Disc 1; the identity reproduces every start LSN exactly, which is
        // what pins SECTORS_PER_SEC at 350 rather than leaving it a guess.
        let area_start = 553u32;
        for (start_lsn, start_secs) in [
            (1253u32, 2.0f64),
            (97_890, 278.106_666_7),
            (123_337, 350.813_333_3),
            (150_665, 428.893_333_3),
        ] {
            let predicted = area_start + (start_secs * SECTORS_PER_SEC as f64) as u32;
            assert_eq!(predicted, start_lsn, "track starting at {start_secs}s");
        }
    }

    #[test]
    fn an_uncompressed_stereo_frame_is_the_size_that_proves_no_dst() {
        assert_eq!(DSD64_STEREO_FRAME, 2 * 2_822_400 / 75 / 8);
        // The measured audio-payload totals divide by it exactly.
        assert_eq!(
            350_495u64 * DSD64_STEREO_FRAME as u64 % DSD64_STEREO_FRAME as u64,
            0
        );
    }

    /// The scan sniff: a bounded, internally plausible Master TOC. The ISO
    /// compatibility layer is deliberately irrelevant.
    #[test]
    fn sniff_accepts_raw_and_hybrid_images_with_a_master_toc() {
        use std::io::{Seek, SeekFrom, Write};

        let dir = std::env::temp_dir().join(format!("qbz-sacd-sniff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, toc_lsn: Option<u64>, pvd: bool| {
            let path = dir.join(name);
            let mut file = std::fs::File::create(&path).unwrap();
            if pvd {
                let mut sector = [0u8; 2048];
                sector[0] = 1;
                sector[1..6].copy_from_slice(b"CD001");
                file.seek(SeekFrom::Start(16 * 2048)).unwrap();
                file.write_all(&sector).unwrap();
            }
            if let Some(lsn) = toc_lsn {
                let mut master = [0u8; 2048];
                master[..8].copy_from_slice(b"SACDMTOC");
                master[8] = 1;
                master[9] = 20;
                master[0x40..0x44].copy_from_slice(&544u32.to_be_bytes());
                master[0x54..0x56].copy_from_slice(&1u16.to_be_bytes());
                file.seek(SeekFrom::Start(lsn * 2048)).unwrap();
                file.write_all(&master).unwrap();
            }
            file.seek(SeekFrom::Start(545 * 2048)).unwrap();
            file.write_all(&[0u8; 2048]).unwrap();
            path
        };

        for path in [
            write("sacd.iso", Some(510), true),
            write("backup.iso", Some(530), true),
            write("raw.iso", Some(510), false),
        ] {
            assert_eq!(
                super::sniff_sacd_image(&path).unwrap(),
                SacdSniff::Sacd,
                "{}",
                path.display()
            );
        }
        assert!(!super::is_sacd_image(&write("dvd.iso", None, true)));
        assert!(!super::is_sacd_image(&dir.join("missing.iso")));

        let truncated = write("truncated.iso", Some(510), false);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&truncated)
            .unwrap()
            .write_all(&[0])
            .unwrap();
        assert!(matches!(
            super::sniff_sacd_image(&truncated),
            Err(SacdError::InvalidImageLength(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
