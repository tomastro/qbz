//! Streaming DSD → 88.2 kHz PCM conversion chain.
//!
//! dsd2pcm decimates 8:1 (one float per DSD byte), leaving DSD64 at
//! 352.8 kHz, DSD128 at 705.6 kHz, … A chain of half-band ÷2 stages then
//! brings every rate down to a uniform 88.2 kHz.
//!
//! Why 88.2 kHz (v1 policy, revised after the first smoke): the original
//! 176.4 kHz target is NOT universally supported — the owner's DacMagic
//! Plus USB interface exposes 44.1/48/88.2/96/192 only, so the player fell
//! back to rodio's linear resampler, which has no anti-alias filter: the
//! DSD ultrasonic noise shelf (huge between 44k–88k at a 176.4k container
//! rate) folded straight into the audible band as loud hiss. At 88.2 kHz
//! every DAC that matters accepts the rate natively (no resampler in the
//! path), it is an exact 44.1k-family division, and the half-band chain —
//! which DOES anti-alias properly — removes all DSD noise above 44.1 kHz.
//! A per-device higher-rate policy can come with the Phase-2 capability
//! model (see qbz-nix-docs/dsd-support/).

use crate::demux::{DsdDemuxer, DsdError};
use crate::dsd2pcm::Dsd2Pcm;
use std::sync::OnceLock;

/// Uniform PCM output rate for converted DSD.
pub const OUTPUT_RATE: u32 = 88_200;

/// Default conversion gain. DSD program material can exceed 0 dBFS when
/// low-passed to PCM; the customary −6 dB trim prevents clipping.
pub const DEFAULT_GAIN_DB: f32 = -6.0;

/// DSD bytes requested from the demuxer per conversion block, per channel.
/// 64 KiB ≈ 0.19 s of DSD64 — small enough to stream, big enough to be cheap.
const BLOCK_BYTES_PER_CH: usize = 64 * 1024;

const HALFBAND_TAPS: usize = 63;

/// Symmetric half-band low-pass (cutoff fs/4) for ÷2 decimation, generated
/// once: windowed sinc (Blackman), odd length, even taps (except center)
/// exactly zero by construction of sinc(n/2).
fn halfband_taps() -> &'static [f32; HALFBAND_TAPS] {
    static TAPS: OnceLock<[f32; HALFBAND_TAPS]> = OnceLock::new();
    TAPS.get_or_init(|| {
        let m = (HALFBAND_TAPS - 1) as f64 / 2.0; // 31
        let mut taps = [0.0f32; HALFBAND_TAPS];
        let mut sum = 0.0f64;
        for (n, t) in taps.iter_mut().enumerate() {
            let x = n as f64 - m;
            let sinc = if x == 0.0 {
                0.5
            } else {
                (std::f64::consts::PI * 0.5 * x).sin() / (std::f64::consts::PI * x)
            };
            let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / (HALFBAND_TAPS - 1) as f64).cos()
                + 0.08 * (4.0 * std::f64::consts::PI * n as f64 / (HALFBAND_TAPS - 1) as f64).cos();
            let v = sinc * w;
            *t = v as f32;
            sum += v;
        }
        // Normalize to unity DC gain.
        let scale = (1.0 / sum) as f32;
        for t in taps.iter_mut() {
            *t *= scale;
        }
        taps
    })
}

/// Streaming FIR decimate-by-2 stage with history carry-over.
struct HalfBand {
    /// Pending input: last TAPS-1 samples of the previous call + new input.
    carry: Vec<f32>,
    /// Read cursor parity is preserved across calls via drain bookkeeping.
    next_center: usize,
}

impl HalfBand {
    fn new() -> Self {
        Self {
            // Prime with zeros so the first outputs are filter warm-up, not
            // garbage; keeps output counting deterministic (len/2 per input).
            carry: vec![0.0; HALFBAND_TAPS - 1],
            next_center: HALFBAND_TAPS - 1,
        }
    }

    /// Feed `input`, append decimated output to `out`.
    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let taps = halfband_taps();
        self.carry.extend_from_slice(input);
        let mut i = self.next_center;
        while i < self.carry.len() {
            let window = &self.carry[i + 1 - HALFBAND_TAPS..=i];
            let mut acc = 0.0f32;
            // Half-band: even-indexed taps are zero except the center.
            let mut j = 0;
            while j < HALFBAND_TAPS {
                acc += taps[j] * window[HALFBAND_TAPS - 1 - j];
                j += 2;
            }
            acc += taps[HALFBAND_TAPS / 2] * window[HALFBAND_TAPS / 2];
            out.push(acc);
            i += 2;
        }
        // Keep the last TAPS-1 samples; remember cursor parity.
        let keep_from = self.carry.len().saturating_sub(HALFBAND_TAPS - 1);
        let overshoot = i - self.carry.len(); // 0 or 1
        self.carry.drain(..keep_from);
        self.next_center = (HALFBAND_TAPS - 1) + overshoot;
    }
}

/// Whole-file streaming converter: demuxer → per-channel dsd2pcm →
/// half-band chain → interleaved f32 blocks at [`OUTPUT_RATE`].
pub struct DsdPcmConverter {
    demux: Box<dyn DsdDemuxer>,
    channels: usize,
    lsb_first: bool,
    dsd2pcm: Vec<Dsd2Pcm>,
    stages: Vec<Vec<HalfBand>>, // stages[stage][channel]
    gain: f32,
    total_frames: u64,
    frames_emitted: u64,
    finished: bool,
}

impl DsdPcmConverter {
    pub fn new(demux: Box<dyn DsdDemuxer>, gain_db: f32) -> Result<Self, DsdError> {
        let info = demux.info().clone();
        let ratio = info.dsd_rate / OUTPUT_RATE; // 32 / 64 / 128 / 256
        if info.dsd_rate % OUTPUT_RATE != 0 || !(ratio / 8).is_power_of_two() || ratio < 16 {
            return Err(DsdError::UnsupportedRate(info.dsd_rate));
        }
        let n_stages = (ratio / 8).trailing_zeros() as usize; // 2..=5
        let channels = info.channels as usize;
        let stages = (0..n_stages)
            .map(|_| (0..channels).map(|_| HalfBand::new()).collect())
            .collect();
        Ok(Self {
            demux,
            channels,
            lsb_first: info.lsb_first,
            dsd2pcm: (0..channels).map(|_| Dsd2Pcm::new()).collect(),
            stages,
            gain: 10f32.powf(gain_db / 20.0),
            total_frames: info.sample_count / ratio as u64,
            frames_emitted: 0,
            finished: false,
        })
    }

    pub fn output_rate(&self) -> u32 {
        OUTPUT_RATE
    }
    /// PCM output channels: ALWAYS stereo. Mono sources are duplicated;
    /// multichannel (up to 5.1) sources are downmixed (ITU-R BS.775
    /// coefficients, LFE discarded, normalized against clipping).
    pub fn channels(&self) -> u16 {
        2
    }
    /// Exact number of interleaved PCM frames this converter will emit in
    /// total (used to size the WAV header up front).
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Produce the next interleaved f32 block, `None` when done. The overall
    /// emitted frame count is exactly [`Self::total_frames`]: the final block
    /// is silence-padded or truncated as needed so the container size always
    /// matches the header.
    pub fn next_block(&mut self) -> Result<Option<Vec<f32>>, DsdError> {
        if self.finished {
            return Ok(None);
        }
        let mut planar: Vec<Vec<u8>> = (0..self.channels).map(|_| Vec::new()).collect();
        let got = self.demux.read_planar(&mut planar, BLOCK_BYTES_PER_CH)?;
        if got == 0 {
            // EOF: pad with silence if the filter latency left us short.
            self.finished = true;
            let missing = self.total_frames - self.frames_emitted;
            if missing == 0 {
                return Ok(None);
            }
            self.frames_emitted = self.total_frames;
            return Ok(Some(vec![0.0; (missing as usize) * 2]));
        }

        let mut per_ch: Vec<Vec<f32>> = Vec::with_capacity(self.channels);
        for ch in 0..self.channels {
            let mut buf = Vec::new();
            self.dsd2pcm[ch].translate(&planar[ch], self.lsb_first, &mut buf);
            for stage in self.stages.iter_mut() {
                let mut down = Vec::with_capacity(buf.len() / 2 + 1);
                stage[ch].process(&buf, &mut down);
                buf = down;
            }
            per_ch.push(buf);
        }

        let frames = per_ch.iter().map(|c| c.len()).min().unwrap_or(0) as u64;
        let frames = frames.min(self.total_frames - self.frames_emitted) as usize;
        if frames == 0 {
            // Nothing usable this round (filters still priming) — recurse to
            // pull more input; bounded by file size.
            return self.next_block();
        }
        // Fold to stereo. DSF/DFF positional channel order:
        //   3ch = FL FR C · 4ch = FL FR BL BR · 5ch = FL FR C BL BR ·
        //   6ch = FL FR C LFE BL BR.
        // ITU-R BS.775 downmix (center/surrounds at −3 dB, LFE discarded),
        // normalized by the worst-case coefficient sum against clipping.
        const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
        let (ci, sli, sri) = match self.channels {
            3 => (Some(2), None, None),
            4 => (None, Some(2), Some(3)),
            5 => (Some(2), Some(3), Some(4)),
            6 => (Some(2), Some(4), Some(5)),
            _ => (None, None, None),
        };
        let norm = 1.0
            / (1.0
                + if ci.is_some() { K } else { 0.0 }
                + if sli.is_some() { K } else { 0.0 });
        let mut out = Vec::with_capacity(frames * 2);
        for f in 0..frames {
            let (l, r) = match self.channels {
                1 => (per_ch[0][f], per_ch[0][f]),
                2 => (per_ch[0][f], per_ch[1][f]),
                _ => {
                    let mut l = per_ch[0][f];
                    let mut r = per_ch[1][f];
                    if let Some(i) = ci {
                        l += K * per_ch[i][f];
                        r += K * per_ch[i][f];
                    }
                    if let Some(i) = sli {
                        l += K * per_ch[i][f];
                    }
                    if let Some(i) = sri {
                        r += K * per_ch[i][f];
                    }
                    (l * norm, r * norm)
                }
            };
            out.push(l * self.gain);
            out.push(r * self.gain);
        }
        self.frames_emitted += frames as u64;
        if self.frames_emitted >= self.total_frames {
            self.finished = true;
        }
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halfband_dc_gain_is_unity() {
        let mut hb = HalfBand::new();
        let mut out = Vec::new();
        hb.process(&vec![0.5f32; 8192], &mut out);
        assert_eq!(out.len(), 8192 / 2);
        let tail = &out[256..];
        for &s in tail {
            assert!((s - 0.5).abs() < 1e-3, "DC not preserved: {s}");
        }
    }

    #[test]
    fn halfband_output_count_is_half_input_across_calls() {
        let mut hb = HalfBand::new();
        let mut out = Vec::new();
        // Odd-sized chunks exercise the parity carry-over.
        for chunk in [333usize, 1000, 77, 4096, 1] {
            hb.process(&vec![0.1f32; chunk], &mut out);
        }
        let total: usize = 333 + 1000 + 77 + 4096 + 1;
        // Centers sit on fixed parity from the zero prefix → ceil(total/2).
        assert_eq!(out.len(), total.div_ceil(2));
    }
}
