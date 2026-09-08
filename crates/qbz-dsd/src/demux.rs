//! DSF and DFF (DSDIFF) demuxers.
//!
//! Both containers carry raw 1-bit DSD grouped in bytes; they differ in
//! layout and bit order:
//! - DSF (Sony spec): per-channel BLOCKS of `block_size` (4096) bytes,
//!   `[ch0 block][ch1 block][ch0 block]…`; bit order declared in the header
//!   (`bits_per_sample`: 1 = LSB-first, 8 = MSB-first; LSB-first in practice).
//!   Standard ID3v2 tag at the header-declared `metadata_ptr` offset.
//! - DFF (Philips DSDIFF 1.5): frame-interleaved, ONE byte per channel round
//!   robin, always MSB-first. Big-endian chunk sizes, chunks padded to even
//!   offsets. No standard tagging; a trailing nonstandard "ID3 " chunk is
//!   honored when present. DST-compressed DFF is detected and rejected.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Errors from DSD demuxing/conversion. `UnsupportedDst` / `UnsupportedChannels`
/// are expected user-facing cases (toast + skip), not bugs.
#[derive(Debug, thiserror::Error)]
pub enum DsdError {
    #[error("DST-compressed DFF is not supported")]
    UnsupportedDst,
    #[error("unsupported channel count: {0} (mono, stereo and up to 5.1 supported)")]
    UnsupportedChannels(u16),
    #[error("unsupported DSD rate: {0} Hz")]
    UnsupportedRate(u32),
    #[error("corrupt or invalid DSD file: {0}")]
    Corrupt(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Tag subset read from the container (DSF: embedded ID3v2; DFF: trailing
/// "ID3 " chunk when present, otherwise empty — callers fall back to
/// filename-derived metadata).
#[derive(Debug, Clone, Default)]
pub struct DsdTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<i32>,
    /// First embedded picture (front cover preferred), raw bytes.
    pub artwork: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct DsdStreamInfo {
    /// DSD bit rate per channel (2 822 400 = DSD64, …).
    pub dsd_rate: u32,
    pub channels: u16,
    /// Total DSD bits per channel.
    pub sample_count: u64,
    /// Bit order inside each byte: true = LSB is temporally first (DSF
    /// default), false = MSB first (DFF, and DSF with bits_per_sample = 8).
    pub lsb_first: bool,
    pub tags: DsdTags,
}

impl DsdStreamInfo {
    pub fn duration_secs(&self) -> u64 {
        if self.dsd_rate == 0 {
            0
        } else {
            self.sample_count / self.dsd_rate as u64
        }
    }
}

pub trait DsdDemuxer: Send {
    fn info(&self) -> &DsdStreamInfo;
    /// Reposition to a DSD bit offset per channel. Container alignment may
    /// round the request down by less than one byte/frame; UI seeks are whole
    /// seconds and therefore land exactly on byte boundaries at every
    /// supported DSD rate.
    fn seek_to_bit(&mut self, bit_per_channel: u64) -> Result<(), DsdError>;
    /// Append up to `max_bytes_per_ch` DSD bytes per channel to `out[ch]`
    /// (planar). Returns the byte count appended to EACH channel (always
    /// equal across channels); 0 means end of stream.
    fn read_planar(
        &mut self,
        out: &mut [Vec<u8>],
        max_bytes_per_ch: usize,
    ) -> Result<usize, DsdError>;
}

const VALID_RATES: [u32; 4] = [2_822_400, 5_644_800, 11_289_600, 22_579_200];

/// Open a DSD source: a DSF or DFF file, or a track inside a SACD image.
///
/// The SACD arm is answered FIRST, before `File::open`. A `sacd:` reference is
/// not a filesystem path — opening it would fail, and sniffing DSF magic at
/// its first byte is meaningless. Routing here rather than at every call site
/// is what lets the PCM, DoP, native and gapless paths inherit disc images
/// without one line changing in the player: all six of them already call this.
pub fn open_dsd(path: &Path) -> Result<Box<dyn DsdDemuxer>, DsdError> {
    let as_str = path.to_string_lossy();
    if qbz_disc::SacdRef::is_sacd_path(&as_str) {
        let r = qbz_disc::SacdRef::parse(&as_str)
            .ok_or_else(|| DsdError::Corrupt(format!("malformed sacd reference: {as_str}")))?;
        let area = qbz_disc::sacd::read_area(&r.image)
            .map_err(|e| DsdError::Corrupt(e.to_string()))?;
        let track = area
            .tracks
            .iter()
            .find(|t| t.number == r.track)
            .ok_or_else(|| DsdError::Corrupt(format!("no track {} on this image", r.track)))?;
        return Ok(Box::new(crate::sacd_source::SacdDemuxer::open_default(
            &r.image, track,
        )?));
    }

    let mut file = File::open(path)?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;
    match &magic {
        b"DSD " => Ok(Box::new(DsfReader::open(file)?)),
        b"FRM8" => Ok(Box::new(DffReader::open(file)?)),
        _ => Err(DsdError::Corrupt("not a DSF or DFF file".into())),
    }
}

fn read_u32_le(f: &mut File) -> Result<u32, DsdError> {
    let mut b = [0u8; 4];
    f.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64_le(f: &mut File) -> Result<u64, DsdError> {
    let mut b = [0u8; 8];
    f.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn read_u64_be(f: &mut File) -> Result<u64, DsdError> {
    let mut b = [0u8; 8];
    f.read_exact(&mut b)?;
    Ok(u64::from_be_bytes(b))
}
fn read_id(f: &mut File) -> Result<[u8; 4], DsdError> {
    let mut b = [0u8; 4];
    f.read_exact(&mut b)?;
    Ok(b)
}

fn validate_rate(rate: u32) -> Result<(), DsdError> {
    if VALID_RATES.contains(&rate) {
        Ok(())
    } else {
        Err(DsdError::UnsupportedRate(rate))
    }
}

fn read_id3_tags(file: &mut File, offset: u64) -> DsdTags {
    let mut tags = DsdTags::default();
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return tags;
    }
    match id3::Tag::read_from2(&mut *file) {
        Ok(tag) => {
            use id3::TagLike;
            tags.title = tag.title().map(str::to_string);
            tags.artist = tag.artist().map(str::to_string);
            tags.album = tag.album().map(str::to_string);
            tags.album_artist = tag.album_artist().map(str::to_string);
            tags.genre = tag.genre_parsed().map(|g| g.into_owned());
            tags.track_number = tag.track();
            tags.disc_number = tag.disc();
            tags.year = tag.year().or_else(|| tag.date_recorded().map(|d| d.year));
            tags.artwork = tag
                .pictures()
                .find(|p| p.picture_type == id3::frame::PictureType::CoverFront)
                .or_else(|| tag.pictures().next())
                .map(|p| p.data.clone());
        }
        Err(e) => log::debug!("[qbz-dsd] ID3 read failed (non-fatal): {e}"),
    }
    tags
}

// ---------------------------------------------------------------------------
// DSF
// ---------------------------------------------------------------------------

struct DsfReader {
    file: File,
    info: DsdStreamInfo,
    block_size: usize,
    data_start: u64,
    total_bytes_per_ch: u64,
    /// Valid (non-padding) DSD bytes remaining per channel.
    remaining_per_ch: u64,
    /// Bytes to discard from each channel's first physical block after seek.
    first_block_skip: usize,
}

impl DsfReader {
    fn open(mut file: File) -> Result<Self, DsdError> {
        // "DSD " chunk: magic + size(28) + total file size + metadata ptr.
        let id = read_id(&mut file)?;
        if &id != b"DSD " {
            return Err(DsdError::Corrupt("missing DSD chunk".into()));
        }
        let dsd_chunk_size = read_u64_le(&mut file)?;
        if dsd_chunk_size != 28 {
            return Err(DsdError::Corrupt(format!(
                "bad DSD chunk size {dsd_chunk_size}"
            )));
        }
        let _total_size = read_u64_le(&mut file)?;
        let metadata_ptr = read_u64_le(&mut file)?;

        // "fmt " chunk.
        let id = read_id(&mut file)?;
        if &id != b"fmt " {
            return Err(DsdError::Corrupt("missing fmt chunk".into()));
        }
        let fmt_size = read_u64_le(&mut file)?;
        if fmt_size < 52 {
            return Err(DsdError::Corrupt(format!("bad fmt chunk size {fmt_size}")));
        }
        let format_version = read_u32_le(&mut file)?;
        let format_id = read_u32_le(&mut file)?;
        let _channel_type = read_u32_le(&mut file)?;
        let channel_num = read_u32_le(&mut file)?;
        let sampling_frequency = read_u32_le(&mut file)?;
        let bits_per_sample = read_u32_le(&mut file)?;
        let sample_count = read_u64_le(&mut file)?;
        let block_size = read_u32_le(&mut file)?;
        let _reserved = read_u32_le(&mut file)?;

        if format_version != 1 || format_id != 0 {
            return Err(DsdError::Corrupt(format!(
                "unsupported DSF format version {format_version} / id {format_id}"
            )));
        }
        if !(1..=6).contains(&channel_num) {
            return Err(DsdError::UnsupportedChannels(channel_num as u16));
        }
        validate_rate(sampling_frequency)?;
        let lsb_first = match bits_per_sample {
            1 => true,
            8 => false,
            other => {
                return Err(DsdError::Corrupt(format!(
                    "bad DSF bits_per_sample {other}"
                )))
            }
        };
        if block_size == 0 || block_size > (1 << 20) {
            return Err(DsdError::Corrupt(format!("bad DSF block size {block_size}")));
        }

        // "data" chunk header; sample data starts right after.
        let id = read_id(&mut file)?;
        if &id != b"data" {
            return Err(DsdError::Corrupt("missing data chunk".into()));
        }
        let _data_chunk_size = read_u64_le(&mut file)?;
        let data_start = file.stream_position()?;

        let tags = if metadata_ptr != 0 {
            let t = read_id3_tags(&mut file, metadata_ptr);
            file.seek(SeekFrom::Start(data_start))?;
            t
        } else {
            DsdTags::default()
        };

        let total_bytes_per_ch = sample_count.div_ceil(8);
        Ok(Self {
            file,
            block_size: block_size as usize,
            data_start,
            total_bytes_per_ch,
            remaining_per_ch: total_bytes_per_ch,
            first_block_skip: 0,
            info: DsdStreamInfo {
                dsd_rate: sampling_frequency,
                channels: channel_num as u16,
                sample_count,
                lsb_first,
                tags,
            },
        })
    }
}

impl DsdDemuxer for DsfReader {
    fn info(&self) -> &DsdStreamInfo {
        &self.info
    }

    fn seek_to_bit(&mut self, bit_per_channel: u64) -> Result<(), DsdError> {
        let target_byte = (bit_per_channel / 8).min(self.total_bytes_per_ch);
        let block = target_byte / self.block_size as u64;
        self.first_block_skip = (target_byte % self.block_size as u64) as usize;
        let physical = self.data_start.saturating_add(
            block.saturating_mul(self.block_size as u64 * self.info.channels as u64),
        );
        self.file.seek(SeekFrom::Start(physical))?;
        self.remaining_per_ch = self.total_bytes_per_ch - target_byte;
        Ok(())
    }

    fn read_planar(
        &mut self,
        out: &mut [Vec<u8>],
        max_bytes_per_ch: usize,
    ) -> Result<usize, DsdError> {
        debug_assert_eq!(out.len(), self.info.channels as usize);
        if self.remaining_per_ch == 0 {
            return Ok(0);
        }
        let mut appended = 0usize;
        let mut block = vec![0u8; self.block_size];
        while appended < max_bytes_per_ch && self.remaining_per_ch > 0 {
            // One block group: block_size bytes for each channel in order.
            let available = self.block_size - self.first_block_skip;
            let valid = (self.remaining_per_ch as usize).min(available);
            for ch in 0..self.info.channels as usize {
                match self.file.read_exact(&mut block) {
                    Ok(()) => out[ch].extend_from_slice(
                        &block[self.first_block_skip..self.first_block_skip + valid],
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        // Truncated file: stop at what we got.
                        self.remaining_per_ch = 0;
                        return Ok(appended);
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            self.first_block_skip = 0;
            self.remaining_per_ch -= valid as u64;
            appended += valid;
        }
        Ok(appended)
    }
}

// ---------------------------------------------------------------------------
// DFF (DSDIFF)
// ---------------------------------------------------------------------------

struct DffReader {
    file: File,
    info: DsdStreamInfo,
    data_offset: u64,
    data_size: u64,
    /// Bytes (all channels interleaved) remaining in the DSD data chunk.
    remaining_total: u64,
}

impl DffReader {
    fn open(mut file: File) -> Result<Self, DsdError> {
        let id = read_id(&mut file)?;
        if &id != b"FRM8" {
            return Err(DsdError::Corrupt("missing FRM8 chunk".into()));
        }
        let _form_size = read_u64_be(&mut file)?;
        let form_type = read_id(&mut file)?;
        if &form_type != b"DSD " {
            return Err(DsdError::Corrupt("FRM8 form type is not DSD".into()));
        }

        let mut dsd_rate: Option<u32> = None;
        let mut channels: Option<u16> = None;
        let mut data: Option<(u64, u64)> = None; // (offset, size)
        let mut id3_offset: Option<u64> = None;

        // Top-level chunk scan (seek past payloads; chunks are even-padded).
        loop {
            let mut idbuf = [0u8; 4];
            match file.read_exact(&mut idbuf) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
            let size = read_u64_be(&mut file)?;
            let payload_start = file.stream_position()?;
            match &idbuf {
                b"PROP" => {
                    // Property container: "SND " + subchunks.
                    let prop_type = read_id(&mut file)?;
                    if &prop_type != b"SND " {
                        return Err(DsdError::Corrupt("PROP type is not SND".into()));
                    }
                    let prop_end = payload_start + size;
                    while file.stream_position()? + 12 <= prop_end {
                        let sub_id = read_id(&mut file)?;
                        let sub_size = read_u64_be(&mut file)?;
                        let sub_start = file.stream_position()?;
                        match &sub_id {
                            b"FS  " => {
                                let mut b = [0u8; 4];
                                file.read_exact(&mut b)?;
                                dsd_rate = Some(u32::from_be_bytes(b));
                            }
                            b"CHNL" => {
                                let mut b = [0u8; 2];
                                file.read_exact(&mut b)?;
                                channels = Some(u16::from_be_bytes(b));
                            }
                            b"CMPR" => {
                                let cmpr = read_id(&mut file)?;
                                if &cmpr == b"DST " {
                                    return Err(DsdError::UnsupportedDst);
                                }
                                if &cmpr != b"DSD " {
                                    return Err(DsdError::Corrupt(format!(
                                        "unknown DFF compression {:?}",
                                        String::from_utf8_lossy(&cmpr)
                                    )));
                                }
                            }
                            _ => {}
                        }
                        // Subchunks are even-padded too.
                        let padded = sub_size + (sub_size & 1);
                        file.seek(SeekFrom::Start(sub_start + padded))?;
                    }
                }
                b"DSD " => {
                    data = Some((payload_start, size));
                }
                b"DST " => return Err(DsdError::UnsupportedDst),
                b"ID3 " => {
                    id3_offset = Some(payload_start);
                }
                _ => {}
            }
            let padded = size + (size & 1);
            file.seek(SeekFrom::Start(payload_start + padded))?;
        }

        let dsd_rate = dsd_rate.ok_or_else(|| DsdError::Corrupt("DFF missing FS".into()))?;
        let channels = channels.ok_or_else(|| DsdError::Corrupt("DFF missing CHNL".into()))?;
        let (data_offset, data_size) =
            data.ok_or_else(|| DsdError::Corrupt("DFF missing DSD data".into()))?;
        validate_rate(dsd_rate)?;
        if !(1..=6).contains(&channels) {
            return Err(DsdError::UnsupportedChannels(channels));
        }

        let tags = match id3_offset {
            Some(off) => read_id3_tags(&mut file, off),
            None => DsdTags::default(),
        };

        file.seek(SeekFrom::Start(data_offset))?;
        let sample_count = data_size / channels as u64 * 8;

        Ok(Self {
            file,
            data_offset,
            data_size,
            remaining_total: data_size,
            info: DsdStreamInfo {
                dsd_rate,
                channels,
                sample_count,
                lsb_first: false,
                tags,
            },
        })
    }
}

impl DsdDemuxer for DffReader {
    fn info(&self) -> &DsdStreamInfo {
        &self.info
    }

    fn seek_to_bit(&mut self, bit_per_channel: u64) -> Result<(), DsdError> {
        let channels = self.info.channels as u64;
        let total_per_channel = self.data_size / channels;
        let target_per_channel = (bit_per_channel / 8).min(total_per_channel);
        let target_total = target_per_channel.saturating_mul(channels);
        self.file
            .seek(SeekFrom::Start(self.data_offset + target_total))?;
        self.remaining_total = self.data_size - target_total;
        Ok(())
    }

    fn read_planar(
        &mut self,
        out: &mut [Vec<u8>],
        max_bytes_per_ch: usize,
    ) -> Result<usize, DsdError> {
        let ch = self.info.channels as usize;
        debug_assert_eq!(out.len(), ch);
        if self.remaining_total == 0 {
            return Ok(0);
        }
        // Whole frames only (one byte per channel).
        let want_total = (max_bytes_per_ch * ch).min(self.remaining_total as usize);
        let want_total = want_total - (want_total % ch);
        if want_total == 0 {
            self.remaining_total = 0;
            return Ok(0);
        }
        let mut buf = vec![0u8; want_total];
        let mut filled = 0usize;
        while filled < want_total {
            match self.file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        let frames = filled / ch;
        for f in 0..frames {
            for c in 0..ch {
                out[c].push(buf[f * ch + c]);
            }
        }
        self.remaining_total = if filled < want_total {
            0
        } else {
            self.remaining_total - filled as u64
        };
        Ok(frames)
    }
}
