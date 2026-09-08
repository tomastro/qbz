//! A SACD track presented as a [`DsdDemuxer`], so every DSD delivery this
//! player already has — PCM conversion, DoP, native — plays a disc image
//! without knowing what one is.
//!
//! The dependency runs qbz-dsd -> qbz-disc and never the other way: qbz-disc
//! owns bytes and geometry, this file owns the audio contract.

use dst_decoder::decoder::DstDecoder;
use qbz_disc::sacd::{SacdFrameKind, SacdFrameReader, SacdTrack, SacdTrackReader};

use crate::demux::{DsdDemuxer, DsdError, DsdStreamInfo, DsdTags};

/// DSD64: the only rate the Scarlet Book stereo area uses.
const DSD64: u32 = 2_822_400;
/// One uncompressed stereo frame, and 1/75 s of audio.
const FRAME: usize = 9408;
/// Bytes ONE channel contributes to a frame.
const FRAME_PER_CH: usize = FRAME / 2;
/// Sectors pulled from the image per legacy diagnostic refill.
const REFILL_SECTORS: usize = 200;

fn decode_dst_frame(
    decoder: &mut DstDecoder,
    scratch: &mut [u8],
    payload: &[u8],
    pending: &mut Vec<u8>,
    track: u8,
    frame_number: u32,
    start_lsn: u64,
) -> Result<(), DsdError> {
    scratch.fill(0);
    let written = decoder.decode_frame(payload, scratch).map_err(|error| {
        DsdError::Corrupt(format!(
            "SACD track {track} DST frame {frame_number} at LSN {start_lsn}: {error}"
        ))
    })?;
    if written != FRAME {
        return Err(DsdError::Corrupt(format!(
            "SACD track {track} DST frame {frame_number} at LSN {start_lsn} decoded {written} bytes, expected {FRAME}"
        )));
    }
    // Upstream writes 0x55 into `scratch` on Err. Publication is deliberately
    // after both Ok and the exact-size check, so synthetic silence cannot
    // escape into PCM, DoP or native output.
    pending.extend_from_slice(&scratch[..written]);
    Ok(())
}

/// How the two channels sit inside one frame.
///
/// Both schemes exist in the wild — DFF interleaves per BYTE, DSF per block —
/// so this was not obvious from first principles and was not left to a guess.
///
/// IT WAS ANSWERED WRONG ONCE, AND THE WRONG ANSWER SHIPPED. The first
/// measurement converted twelve seconds of track 4 and looked at where the
/// ENERGY landed:
///
///   BlockPerChannel   77 % of energy in the audible band, 23 % above it
///   ByteInterleaved   99.8 % audible, 0.2 % above
///
/// and reasoned that a real DSD64 stream must carry a large ultrasonic
/// component, so the layout with one had to be right. **That argument is
/// backwards for this pipeline.** `DsdPcmConverter` low-passes and decimates
/// to 88.2 kHz, so it REMOVES sigma-delta noise shaping by design — a correct
/// decode has nothing above 24 kHz. The 23 % was aliased garbage from bits
/// read out of order, which is to say the measurement was reading the
/// symptom as the proof. Neither does the ratio discriminate: scrambled bits
/// and shaped noise both put energy up high.
///
/// RE-MEASURED with a statistic that CAN falsify — decode with the real
/// converter, then look at the shape of the audible band and at whether the
/// two channels are a stereo pair at all. Music has a peaky spectrum (low
/// spectral flatness) and two channels of one orchestra are positively
/// correlated; two independent noise streams are flat and uncorrelated.
/// Tracks 1, 4 and 13 of disc 1 and track 5 of disc 2:
///
///   layout             spectral flatness        L/R correlation
///   BlockPerChannel    0.038 … 0.107            -0.17 … -0.06
///   ByteInterleaved    0.0036 … 0.0093          +0.24 … +0.48
///
/// A negative correlation between the channels of an orchestral recording is
/// not a subtle hint, and a spectrum 10-20x flatter is noise. The layout is
/// `ByteInterleaved`, on every track of both discs.
///
/// (Three statistical probes tried before any of this — per-frame level
/// variance, L/R envelope correlation and a crude spectral centroid — came
/// back inconclusive. So did a fourth, which measured how far the 64-bit local
/// mean of the raw bit stream swings: it answers the same for clean and
/// scrambled bits, because the local mean of a byte-level mix of two
/// correlated channels is still roughly the audio. A measurement that cannot
/// come out differently is not a measurement.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayout {
    /// `[ch0 4704][ch1 4704]` — one contiguous block per channel per frame.
    BlockPerChannel,
    /// `ch0 ch1 ch0 ch1 …` one byte at a time.
    ByteInterleaved,
}

pub struct SacdDemuxer {
    reader: SacdReader,
    info: DsdStreamInfo,
    layout: ChannelLayout,
    /// Audio payload read but not yet split into channels. Only whole frames
    /// are consumed, so a partial frame waits here for the next refill.
    pending: Vec<u8>,
    dst_decoder: Option<DstDecoder>,
    decode_scratch: Vec<u8>,
    track_number: u8,
    frame_number: u32,
    done: bool,
}

enum SacdReader {
    /// Playback: exact TRL2-bounded frames for both flat DSD and DST.
    Framed(SacdFrameReader),
    /// Kept only for the `sync=false` diagnostic that reproduces the old bug.
    Legacy(SacdTrackReader),
}

impl Default for ChannelLayout {
    /// The measured layout. A caller that does not care gets the right one.
    fn default() -> Self {
        Self::ByteInterleaved
    }
}

impl SacdDemuxer {
    /// Open with the measured layout.
    pub fn open_default(image: &std::path::Path, track: &SacdTrack) -> Result<Self, DsdError> {
        Self::open(image, track, ChannelLayout::default())
    }

    pub fn open(
        image: &std::path::Path,
        track: &SacdTrack,
        layout: ChannelLayout,
    ) -> Result<Self, DsdError> {
        Self::open_with(image, track, layout, true)
    }

    /// The probe's door: `sync = false` reproduces the stream that shipped
    /// before the frame boundary was read, which is what lets a measurement
    /// show the difference instead of asserting it.
    pub fn open_with(
        image: &std::path::Path,
        track: &SacdTrack,
        layout: ChannelLayout,
        sync: bool,
    ) -> Result<Self, DsdError> {
        let reader = if sync {
            SacdReader::Framed(
                SacdFrameReader::open(image, track)
                    .map_err(|error| DsdError::Corrupt(error.to_string()))?,
            )
        } else {
            SacdReader::Legacy(
                SacdTrackReader::open_with(
                    image,
                    track,
                    qbz_disc::sacd::FrameSync::FromPayloadStart,
                )
                .map_err(|error| DsdError::Corrupt(error.to_string()))?,
            )
        };
        let dst_decoder = if track.encoding == qbz_disc::sacd::SacdEncoding::Dst {
            Some(
                DstDecoder::new(2, DSD64 as usize)
                    .map_err(|error| DsdError::Corrupt(error.to_string()))?,
            )
        } else {
            None
        };
        Ok(Self {
            reader,
            info: DsdStreamInfo {
                dsd_rate: DSD64,
                channels: 2,
                // Bits per channel. The area TOC gives a duration, and DSD64
                // is exactly 2 822 400 bits/s, so this is the count rather
                // than an estimate.
                sample_count: u64::from(track.duration_frames) * u64::from(DSD64 / 75),
                // SACD is MSB-first, like DFF and unlike DSF's usual LSB-first.
                // Getting this backwards does not fail — it plays, as noise.
                lsb_first: false,
                tags: DsdTags {
                    title: track.title.clone(),
                    track_number: Some(track.number as u32),
                    ..Default::default()
                },
            },
            layout,
            pending: Vec::new(),
            dst_decoder,
            decode_scratch: vec![0u8; FRAME],
            track_number: track.number,
            frame_number: 0,
            done: false,
        })
    }

    fn refill(&mut self) -> Result<(), DsdError> {
        if self.done {
            return Ok(());
        }
        match &mut self.reader {
            SacdReader::Legacy(reader) => {
                let mut chunk = Vec::new();
                reader
                    .next_chunk(&mut chunk, REFILL_SECTORS)
                    .map_err(|error| DsdError::Corrupt(error.to_string()))?;
                self.pending.extend_from_slice(&chunk);
                self.done = reader.finished();
            }
            SacdReader::Framed(reader) => match reader
                .next_frame()
                .map_err(|error| DsdError::Corrupt(error.to_string()))?
            {
                Some(frame) => {
                    match frame.kind {
                        SacdFrameKind::Dsd => {
                            if frame.payload.len() != FRAME {
                                return Err(DsdError::Corrupt(format!(
                                    "SACD track {} frame {} at LSN {} produced {} DSD bytes, expected {FRAME}",
                                    self.track_number,
                                    self.frame_number,
                                    frame.start_lsn,
                                    frame.payload.len(),
                                )));
                            }
                            self.pending.extend_from_slice(&frame.payload);
                        }
                        SacdFrameKind::Dst => {
                            let decoder = self.dst_decoder.as_mut().ok_or_else(|| {
                                DsdError::Corrupt(
                                    "DST frame reached a stream without a decoder".to_string(),
                                )
                            })?;
                            decode_dst_frame(
                                decoder,
                                &mut self.decode_scratch,
                                &frame.payload,
                                &mut self.pending,
                                self.track_number,
                                self.frame_number,
                                frame.start_lsn,
                            )?;
                        }
                    }
                    self.frame_number = self.frame_number.saturating_add(1);
                }
                None => self.done = true,
            },
        }
        Ok(())
    }
}

impl DsdDemuxer for SacdDemuxer {
    fn info(&self) -> &DsdStreamInfo {
        &self.info
    }

    fn seek_to_bit(&mut self, bit_per_channel: u64) -> Result<(), DsdError> {
        let target = bit_per_channel.min(self.info.sample_count);
        match &mut self.reader {
            SacdReader::Framed(reader) => reader
                .seek_to_fraction(target, self.info.sample_count)
                .map_err(|error| DsdError::Corrupt(error.to_string()))?,
            SacdReader::Legacy(reader) => reader.seek_to_fraction(target, self.info.sample_count),
        }
        if self.dst_decoder.is_some() {
            self.dst_decoder = Some(
                DstDecoder::new(2, DSD64 as usize)
                    .map_err(|error| DsdError::Corrupt(error.to_string()))?,
            );
        }
        self.pending.clear();
        self.frame_number = ((u128::from(target) * 75) / u128::from(DSD64)) as u32;
        self.done = match &self.reader {
            SacdReader::Framed(reader) => reader.finished(),
            SacdReader::Legacy(reader) => reader.finished(),
        };
        Ok(())
    }

    fn read_planar(
        &mut self,
        out: &mut [Vec<u8>],
        max_bytes_per_ch: usize,
    ) -> Result<usize, DsdError> {
        if out.len() < 2 {
            return Err(DsdError::UnsupportedChannels(out.len() as u16));
        }
        // Work in whole frames: a frame is the unit the channels are split on,
        // and splitting a partial one puts the two channels out of step for
        // the rest of the track.
        let want_frames = (max_bytes_per_ch / FRAME_PER_CH).max(1);
        while self.pending.len() < want_frames * FRAME && !self.done {
            self.refill()?;
        }
        let have = (self.pending.len() / FRAME).min(want_frames);
        if have == 0 {
            return Ok(0);
        }

        for i in 0..have {
            let frame = &self.pending[i * FRAME..(i + 1) * FRAME];
            match self.layout {
                ChannelLayout::BlockPerChannel => {
                    out[0].extend_from_slice(&frame[..FRAME_PER_CH]);
                    out[1].extend_from_slice(&frame[FRAME_PER_CH..]);
                }
                ChannelLayout::ByteInterleaved => {
                    for pair in frame.chunks_exact(2) {
                        out[0].push(pair[0]);
                        out[1].push(pair[1]);
                    }
                }
            }
        }
        self.pending.drain(..have * FRAME);
        Ok(have * FRAME_PER_CH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_splits_into_equal_channel_halves() {
        assert_eq!(FRAME, 9408);
        assert_eq!(FRAME_PER_CH, 4704);
        // Each channel contributes exactly 1/75 s of DSD64: 2822400/75 bits
        // = 37632 bits = 4704 bytes. If this ever stops holding, the frame
        // size or the rate has changed and the split is wrong.
        assert_eq!(FRAME_PER_CH * 8, (DSD64 / 75) as usize);
    }

    #[test]
    fn the_two_layouts_disagree_which_is_why_one_had_to_be_measured() {
        // A deliberately asymmetric frame: the first half all 0xAA, the
        // second all 0x55.
        let mut frame = vec![0xAAu8; FRAME_PER_CH];
        frame.extend(std::iter::repeat(0x55).take(FRAME_PER_CH));

        let block0: Vec<u8> = frame[..FRAME_PER_CH].to_vec();
        let inter0: Vec<u8> = frame.chunks_exact(2).map(|p| p[0]).collect();
        assert!(block0.iter().all(|b| *b == 0xAA));
        // Byte-interleaving the same frame gives a channel that is half 0xAA
        // and half 0x55 — a completely different signal, which is why picking
        // the wrong one is audible rather than subtle.
        assert!(inter0.iter().any(|b| *b == 0x55));
    }

    /// THE regression guard for the 2026-08-21 correction.
    ///
    /// A default is not a detail here: `open_default` is what playback uses,
    /// and getting it wrong does not fail — it plays, as noise with the music
    /// audible behind it, in every delivery mode at once. The measurement
    /// that settled it is in `ChannelLayout`'s own doc; this pins the answer
    /// so a future tidy-up cannot quietly put the old one back.
    #[test]
    fn the_default_layout_is_the_one_that_was_measured() {
        assert_eq!(ChannelLayout::default(), ChannelLayout::ByteInterleaved);
    }

    /// The split has to be exact in BOTH directions, or one channel silently
    /// carries the other's bits.
    #[test]
    fn byte_interleaving_separates_the_channels_without_losing_a_byte() {
        // Even bytes are channel 0, odd bytes channel 1 — build a frame that
        // says so unambiguously.
        let frame: Vec<u8> = (0..FRAME)
            .map(|i| if i % 2 == 0 { 0x11 } else { 0x22 })
            .collect();
        let l: Vec<u8> = frame.chunks_exact(2).map(|p| p[0]).collect();
        let r: Vec<u8> = frame.chunks_exact(2).map(|p| p[1]).collect();
        assert_eq!(l.len(), FRAME_PER_CH);
        assert_eq!(r.len(), FRAME_PER_CH);
        assert!(l.iter().all(|b| *b == 0x11), "channel 0 is the even bytes");
        assert!(r.iter().all(|b| *b == 0x22), "channel 1 is the odd bytes");
        assert_eq!(l.len() + r.len(), FRAME, "no byte is dropped or doubled");
    }

    #[test]
    fn a_corrupt_dst_frame_publishes_none_of_upstreams_synthetic_silence() {
        let mut decoder = DstDecoder::new(2, DSD64 as usize).unwrap();
        let mut scratch = vec![0u8; FRAME];
        let mut pending = vec![0x12, 0x34];
        let before = pending.clone();

        let result = decode_dst_frame(&mut decoder, &mut scratch, &[], &mut pending, 1, 7, 1234);

        assert!(result.is_err());
        assert_eq!(pending, before, "no decoded bytes may be appended on Err");
        assert!(
            scratch.iter().all(|byte| *byte == 0x55),
            "the test must exercise upstream's error-fill behavior"
        );
    }
}
