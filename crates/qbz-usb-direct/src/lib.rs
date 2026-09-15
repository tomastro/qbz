//! Platform-neutral core of QBZ's Android USB Audio direct backend.
//!
//! Android-specific USB ownership and `usbfs` URBs live at the edge. This
//! crate owns the parts that must remain deterministic and testable on a
//! desktop host: native-rate validation, PCM packing, nominal microframe
//! scheduling, and asynchronous feedback decoding.

use thiserror::Error;

#[cfg(target_os = "android")]
mod android_usbfs;
#[cfg(target_os = "android")]
mod jni_probe;
#[cfg(target_os = "android")]
pub use android_usbfs::{
    AndroidUsbDirectConfig, AndroidUsbDirectStream, AndroidUsbFs, IsoCompletion,
};

pub const QOBUZ_PCM_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

#[derive(Debug, Error, PartialEq)]
pub enum UsbDirectError {
    #[error("unsupported sample rate: {0} Hz")]
    UnsupportedRate(u32),
    #[error("unsupported channel count: {0}")]
    UnsupportedChannels(u16),
    #[error("unsupported PCM format: {bit_resolution}-bit in {subslot_bytes}-byte subslots")]
    UnsupportedPcm {
        bit_resolution: u8,
        subslot_bytes: u8,
    },
    #[error("USB service rate must be non-zero")]
    InvalidServiceRate,
    #[error("packet needs {required} bytes but endpoint allows {maximum}")]
    PacketTooLarge { required: usize, maximum: usize },
    #[error("feedback payload must contain 3 or 4 bytes, got {0}")]
    InvalidFeedbackLength(usize),
    #[error(
        "feedback rate {actual:.3} frames/tick is outside the safe range around {expected:.3}"
    )]
    ImplausibleFeedback { actual: f64, expected: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub subslot_bytes: u8,
    pub bit_resolution: u8,
}

impl PcmFormat {
    pub fn validate(self) -> Result<Self, UsbDirectError> {
        if !QOBUZ_PCM_RATES.contains(&self.sample_rate) {
            return Err(UsbDirectError::UnsupportedRate(self.sample_rate));
        }
        if self.channels != 2 {
            return Err(UsbDirectError::UnsupportedChannels(self.channels));
        }
        if !matches!((self.subslot_bytes, self.bit_resolution), (2, 16) | (3, 24)) {
            return Err(UsbDirectError::UnsupportedPcm {
                bit_resolution: self.bit_resolution,
                subslot_bytes: self.subslot_bytes,
            });
        }
        Ok(self)
    }

    pub const fn bytes_per_frame(self) -> usize {
        self.channels as usize * self.subslot_bytes as usize
    }
}

/// Fractional frame scheduler for one USB service interval.
///
/// Nominal scheduling uses an exact integer rational. After the first valid
/// feedback sample it changes to Q32 pacing for sub-frame clock corrections.
#[derive(Debug, Clone)]
pub struct PacketPlanner {
    format: PcmFormat,
    service_rate: u32,
    max_packet_size: usize,
    feedback_frames_per_tick_q32: Option<u64>,
    /// Nominal mode: units are `service_rate`; feedback mode: Q32 frames.
    accumulator: u64,
}

impl PacketPlanner {
    pub fn new(
        format: PcmFormat,
        service_rate: u32,
        max_packet_size: usize,
    ) -> Result<Self, UsbDirectError> {
        let format = format.validate()?;
        if service_rate == 0 {
            return Err(UsbDirectError::InvalidServiceRate);
        }
        let planner = Self {
            format,
            service_rate,
            max_packet_size,
            feedback_frames_per_tick_q32: None,
            accumulator: 0,
        };
        planner.check_peak_packet()?;
        Ok(planner)
    }

    pub const fn format(&self) -> PcmFormat {
        self.format
    }

    pub const fn service_rate(&self) -> u32 {
        self.service_rate
    }

    pub fn next_packet_frames(&mut self) -> usize {
        if let Some(frames_per_tick_q32) = self.feedback_frames_per_tick_q32 {
            self.accumulator = self.accumulator.wrapping_add(frames_per_tick_q32);
            let frames = (self.accumulator >> 32) as usize;
            self.accumulator &= u32::MAX as u64;
            frames
        } else {
            self.accumulator += u64::from(self.format.sample_rate);
            let frames = self.accumulator / u64::from(self.service_rate);
            self.accumulator %= u64::from(self.service_rate);
            frames as usize
        }
    }

    pub fn next_packet_bytes(&mut self) -> Result<usize, UsbDirectError> {
        let required = self.next_packet_frames() * self.format.bytes_per_frame();
        if required > self.max_packet_size {
            return Err(UsbDirectError::PacketTooLarge {
                required,
                maximum: self.max_packet_size,
            });
        }
        Ok(required)
    }

    /// Apply a raw UAC feedback sample after plausibility validation.
    ///
    /// Three-byte payloads use 10.14 fixed point; four-byte payloads use
    /// 16.16. Both represent frames per USB service interval.
    pub fn apply_feedback(&mut self, payload: &[u8]) -> Result<f64, UsbDirectError> {
        let frames_per_tick = decode_feedback(payload)?;
        let expected = self.format.sample_rate as f64 / self.service_rate as f64;
        // A real audio clock differs by ppm, not percent. Two percent is wide
        // enough for startup transients but rejects wrong units/endpoints.
        if !(expected * 0.98..=expected * 1.02).contains(&frames_per_tick) {
            return Err(UsbDirectError::ImplausibleFeedback {
                actual: frames_per_tick,
                expected,
            });
        }
        if self.feedback_frames_per_tick_q32.is_none() {
            self.accumulator =
                ((u128::from(self.accumulator) << 32) / u128::from(self.service_rate)) as u64;
        }
        self.feedback_frames_per_tick_q32 =
            Some((frames_per_tick * (1_u64 << 32) as f64).round() as u64);
        Ok(frames_per_tick)
    }

    fn check_peak_packet(&self) -> Result<(), UsbDirectError> {
        let peak_frames = self.format.sample_rate.div_ceil(self.service_rate) as usize + 1;
        let required = peak_frames * self.format.bytes_per_frame();
        if required > self.max_packet_size {
            return Err(UsbDirectError::PacketTooLarge {
                required,
                maximum: self.max_packet_size,
            });
        }
        Ok(())
    }
}

pub fn decode_feedback(payload: &[u8]) -> Result<f64, UsbDirectError> {
    match payload {
        [a, b, c] => {
            let raw = u32::from(*a) | (u32::from(*b) << 8) | (u32::from(*c) << 16);
            Ok(raw as f64 / 16_384.0)
        }
        [a, b, c, d] => {
            let raw = u32::from_le_bytes([*a, *b, *c, *d]);
            Ok(raw as f64 / 65_536.0)
        }
        other => Err(UsbDirectError::InvalidFeedbackLength(other.len())),
    }
}

/// Pack QBZ's interleaved f32 pipeline into the DAC's little-endian integer
/// subslots. This matches the existing ALSA/WASAPI direct-sink scaling.
pub fn pack_f32_le(samples: &[f32], format: PcmFormat, output: &mut Vec<u8>) {
    output.clear();
    output.reserve(samples.len() * format.subslot_bytes as usize);
    let effective_bits = u32::from(format.bit_resolution.min(format.subslot_bytes * 8));
    let peak = ((1_i64 << (effective_bits - 1)) - 1) as f64;
    for &sample in samples {
        let value = if sample.is_finite() {
            sample.clamp(-1.0, 1.0) as f64
        } else {
            0.0
        };
        let packed = (value * peak).round() as i64;
        let bytes = packed.to_le_bytes();
        output.extend_from_slice(&bytes[..format.subslot_bytes as usize]);
    }
}

/// Pack Symphonia's full-scale, left-aligned signed i32 samples without a
/// floating-point round trip. S16 is stored in bits 31..16 and S24 in
/// bits 31..8, so dropping the unused low bits reproduces the decoded PCM
/// word exactly.
pub fn pack_i32_le(samples: &[i32], format: PcmFormat, output: &mut Vec<u8>) {
    output.clear();
    output.reserve(samples.len() * format.subslot_bytes as usize);
    let shift = 32 - u32::from(format.bit_resolution);
    for &sample in samples {
        let value = sample >> shift;
        let bytes = value.to_le_bytes();
        output.extend_from_slice(&bytes[..format.subslot_bytes as usize]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(rate: u32, bits: u8) -> PcmFormat {
        PcmFormat {
            sample_rate: rate,
            channels: 2,
            subslot_bytes: bits / 8,
            bit_resolution: bits,
        }
    }

    #[test]
    fn nominal_high_speed_schedule_is_exact_for_every_qobuz_rate() {
        for rate in QOBUZ_PCM_RATES {
            let mut planner = PacketPlanner::new(format(rate, 24), 8_000, 720).unwrap();
            let frames: usize = (0..8_000).map(|_| planner.next_packet_frames()).sum();
            assert_eq!(frames, rate as usize, "{rate} Hz");
        }
    }

    #[test]
    fn packed_192k_packets_fit_panasonic_endpoint() {
        let mut planner = PacketPlanner::new(format(192_000, 24), 8_000, 720).unwrap();
        for _ in 0..8_000 {
            assert!(planner.next_packet_bytes().unwrap() <= 720);
        }
    }

    #[test]
    fn fractional_44100_schedule_uses_five_and_six_frame_packets() {
        let mut planner = PacketPlanner::new(format(44_100, 24), 8_000, 720).unwrap();
        let frames: Vec<_> = (0..80).map(|_| planner.next_packet_frames()).collect();
        assert!(frames.iter().all(|&count| count == 5 || count == 6));
        assert_eq!(frames.iter().sum::<usize>(), 441);
    }

    #[test]
    fn decodes_both_uac_feedback_encodings() {
        let ten_point_fourteen = (6.0_f64 * 16_384.0) as u32;
        let bytes = ten_point_fourteen.to_le_bytes();
        assert_eq!(decode_feedback(&bytes[..3]).unwrap(), 6.0);

        let sixteen_point_sixteen = (24.0_f64 * 65_536.0) as u32;
        assert_eq!(
            decode_feedback(&sixteen_point_sixteen.to_le_bytes()).unwrap(),
            24.0
        );
    }

    #[test]
    fn feedback_changes_packet_pacing_but_rejects_wrong_units() {
        let mut planner = PacketPlanner::new(format(192_000, 24), 8_000, 720).unwrap();
        let adjusted = (24.001_f64 * 65_536.0).round() as u32;
        assert!(planner.apply_feedback(&adjusted.to_le_bytes()).is_ok());
        assert!(planner.apply_feedback(&[0, 0, 1, 0]).is_err());
    }

    #[test]
    fn packs_s16_and_s24_little_endian() {
        let mut output = Vec::new();
        pack_f32_le(&[0.0, 1.0, -1.0], format(44_100, 16), &mut output);
        assert_eq!(output, [0, 0, 0xff, 0x7f, 1, 0x80]);

        pack_f32_le(&[0.0, 1.0, -1.0], format(192_000, 24), &mut output);
        assert_eq!(output, [0, 0, 0, 0xff, 0xff, 0x7f, 1, 0, 0x80]);
    }

    #[test]
    fn packs_left_aligned_integer_pcm_bit_exactly() {
        let mut output = Vec::new();
        pack_i32_le(&[0x1234_0000, -0x1234_0000], format(44_100, 16), &mut output);
        assert_eq!(output, [0x34, 0x12, 0xcc, 0xed]);

        pack_i32_le(&[0x12345600, -0x12345600], format(192_000, 24), &mut output);
        assert_eq!(output, [0x56, 0x34, 0x12, 0xaa, 0xcb, 0xed]);
    }

    #[test]
    fn rejects_formats_that_would_silently_resample_or_repack() {
        assert_eq!(
            format(384_000, 24).validate(),
            Err(UsbDirectError::UnsupportedRate(384_000))
        );
        assert!(format(44_100, 32).validate().is_err());
    }
}
