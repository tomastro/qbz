//! DoP (DSD over PCM) framing per the DoP Open Standard v1.1 (dCS).
//!
//! Each 24-bit PCM sample carries 16 DSD bits (MSB-first, temporally
//! first byte in the high bits) under a marker byte that alternates
//! 0x05 / 0xFA on successive frames. Both channels of one frame carry the
//! SAME marker. A DoP-aware DAC detects the alternation and switches to
//! DSD mode; anything that alters even one sample breaks the sequence and
//! the DAC falls back to interpreting the stream as PCM (loud noise) —
//! which is why the packed words must travel a bit-exact integer path
//! (no f32, no gain, no resampling).
//!
//! Output words are FINAL S32 samples: the 24-bit DoP word left-justified
//! (`<< 8`), ready for `snd_pcm_writei` on an S32_LE stream.

use crate::demux::{DsdDemuxer, DsdError};
use crate::dsd2pcm::bit_reverse;

/// PCM carrier rate for a DSD bit rate: 16 DSD bits per frame per channel.
/// DSD64 → 176 400 Hz, DSD128 → 352 800 Hz.
pub const fn dop_carrier_rate(dsd_rate: u32) -> u32 {
    dsd_rate / 16
}

/// Stateful DoP frame packer (keeps marker phase across calls).
pub struct DopPacker {
    marker_fa: bool,
}

impl DopPacker {
    pub fn new() -> Self {
        Self { marker_fa: false }
    }

    /// Pack planar MSB-first DSD bytes (2 bytes per channel per frame) into
    /// interleaved S32 DoP samples, appending to `out`. Consumes
    /// `min(len)/2` frames worth of every channel.
    pub fn pack(&mut self, planar: &[Vec<u8>], out: &mut Vec<i32>) {
        let ch = planar.len();
        let frames = planar.iter().map(|c| c.len()).min().unwrap_or(0) / 2;
        out.reserve(frames * ch);
        for f in 0..frames {
            let marker: i32 = if self.marker_fa { 0xFA } else { 0x05 };
            for c in planar.iter() {
                let b0 = c[f * 2] as i32;
                let b1 = c[f * 2 + 1] as i32;
                out.push(((marker << 16) | (b0 << 8) | b1) << 8);
            }
            self.marker_fa = !self.marker_fa;
        }
    }

    /// DSD silence (0x69 payload) with valid alternating markers — REQUIRED
    /// for pause/stop/tail padding: PCM zeros would break the marker
    /// sequence and pop the DAC out of DSD mode.
    pub fn silence(&mut self, n_frames: usize, channels: u16, out: &mut Vec<i32>) {
        out.reserve(n_frames * channels as usize);
        for _ in 0..n_frames {
            let marker: i32 = if self.marker_fa { 0xFA } else { 0x05 };
            for _ in 0..channels {
                out.push(((marker << 16) | 0x6969) << 8);
            }
            self.marker_fa = !self.marker_fa;
        }
    }
}

impl Default for DopPacker {
    fn default() -> Self {
        Self::new()
    }
}

/// Whole-file streaming DoP word source: demuxer → (bit reversal when the
/// container is LSB-first) → packer. Yields interleaved S32 samples at the
/// carrier rate. Stereo only — DoP receivers are 2-channel devices.
pub struct DopStream {
    demux: Box<dyn DsdDemuxer>,
    packer: DopPacker,
    lsb_first: bool,
    dsd_rate: u32,
    total_frames: u64,
    buf: Vec<i32>,
    idx: usize,
    done: bool,
    /// Set when demux I/O fails mid-stream (not clean EOF).
    io_error: Option<String>,
}

/// DSD bytes pulled from the demuxer per refill, per channel.
const REFILL_BYTES_PER_CH: usize = 32 * 1024;

impl DopStream {
    pub fn new(demux: Box<dyn DsdDemuxer>) -> Result<Self, DsdError> {
        let info = demux.info().clone();
        if info.channels != 2 {
            return Err(DsdError::UnsupportedChannels(info.channels));
        }
        Ok(Self {
            demux,
            packer: DopPacker::new(),
            lsb_first: info.lsb_first,
            dsd_rate: info.dsd_rate,
            total_frames: info.sample_count / 16,
            buf: Vec::new(),
            idx: 0,
            done: false,
            io_error: None,
        })
    }

    /// Mid-stream demux I/O error, if any. Clean EOF leaves this `None`.
    pub fn io_error(&self) -> Option<&str> {
        self.io_error.as_deref()
    }

    pub fn carrier_rate(&self) -> u32 {
        dop_carrier_rate(self.dsd_rate)
    }
    pub fn dsd_rate(&self) -> u32 {
        self.dsd_rate
    }
    /// Total DoP frames (per channel) this stream will yield.
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    fn refill(&mut self) -> bool {
        let mut planar: Vec<Vec<u8>> = vec![Vec::new(), Vec::new()];
        match self.demux.read_planar(&mut planar, REFILL_BYTES_PER_CH) {
            Ok(0) => {
                self.done = true;
                false
            }
            Err(e) => {
                log::error!("[DSD/DoP] demux I/O error (not clean EOF): {e}");
                self.io_error = Some(e.to_string());
                self.done = true;
                false
            }
            Ok(_) => {
                if self.lsb_first {
                    for chan in planar.iter_mut() {
                        for b in chan.iter_mut() {
                            *b = bit_reverse(*b);
                        }
                    }
                }
                self.buf.clear();
                self.idx = 0;
                self.packer.pack(&planar, &mut self.buf);
                !self.buf.is_empty()
            }
        }
    }
}

impl Iterator for DopStream {
    type Item = i32;
    fn next(&mut self) -> Option<i32> {
        loop {
            if self.idx < self.buf.len() {
                let v = self.buf[self.idx];
                self.idx += 1;
                return Some(v);
            }
            if self.done || !self.refill() {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packer_layout_and_marker_alternation() {
        let mut p = DopPacker::new();
        let mut out = Vec::new();
        // One channel pair, two frames.
        let l = vec![0xAB, 0xCD, 0x12, 0x34];
        let r = vec![0x55, 0xAA, 0x9C, 0x63];
        p.pack(&[l, r], &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], ((0x05 << 16) | 0xABCD) << 8);
        assert_eq!(out[1], ((0x05 << 16) | 0x55AA) << 8);
        assert_eq!(out[2], ((0xFA << 16) | 0x1234) << 8);
        assert_eq!(out[3], ((0xFA << 16) | 0x9C63) << 8);
        // Phase continues across calls.
        let mut out2 = Vec::new();
        p.pack(&[vec![0, 0], vec![0, 0]], &mut out2);
        assert_eq!(out2[0], (0x05 << 16) << 8);
    }

    #[test]
    fn silence_is_0x69_payload_with_markers() {
        let mut p = DopPacker::new();
        let mut out = Vec::new();
        p.silence(2, 2, &mut out);
        assert_eq!(out[0], ((0x05 << 16) | 0x6969) << 8);
        assert_eq!(out[2], ((0xFA << 16) | 0x6969) << 8);
    }
}

#[cfg(test)]
mod io_error_tests {
    use super::*;
    use crate::demux::{DsdDemuxer, DsdError, DsdStreamInfo};
    use std::io;

    struct FailAfter {
        left: usize,
        info: DsdStreamInfo,
    }

    impl DsdDemuxer for FailAfter {
        fn info(&self) -> &DsdStreamInfo {
            &self.info
        }
        fn seek_to_bit(&mut self, _bit_per_channel: u64) -> Result<(), DsdError> {
            self.left = 1;
            Ok(())
        }
        fn read_planar(
            &mut self,
            out: &mut [Vec<u8>],
            max_bytes_per_ch: usize,
        ) -> Result<usize, DsdError> {
            if self.left == 0 {
                return Err(DsdError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "simulated mid-file I/O error",
                )));
            }
            self.left -= 1;
            let n = max_bytes_per_ch.min(4);
            for ch in out.iter_mut().take(2) {
                ch.extend(std::iter::repeat(0x69).take(n));
            }
            Ok(n)
        }
    }

    #[test]
    fn demux_io_error_sets_sticky_flag_not_clean_eof() {
        let info = DsdStreamInfo {
            channels: 2,
            dsd_rate: 2_822_400,
            sample_count: 1_000_000,
            lsb_first: false,
            tags: Default::default(),
        };
        let demux = FailAfter { left: 1, info };
        let mut stream = DopStream::new(Box::new(demux)).unwrap();
        // First refill succeeds; drain some samples.
        let _ = stream.next();
        // Force more refills until error.
        for _ in 0..100_000 {
            if stream.next().is_none() {
                break;
            }
        }
        assert!(stream.io_error().is_some(), "I/O error must be sticky");
        assert!(stream
            .io_error()
            .unwrap()
            .contains("simulated mid-file I/O error"));
    }
}
