//! Minimal 24-bit PCM WAV encoding for the converted-DSD stream.
//!
//! The converter knows its exact final frame count up front (DSF/DFF headers
//! carry the sample count), so the WAV header sizes are exact — the player's
//! streaming decoder sees an ordinary finite WAV file arriving over a
//! buffered source.

const HEADER_LEN: u64 = 44;
const BYTES_PER_SAMPLE: u64 = 3; // 24-bit

pub fn wav_total_size(total_frames: u64, channels: u16) -> u64 {
    HEADER_LEN + total_frames * channels as u64 * BYTES_PER_SAMPLE
}

/// Standard 44-byte RIFF/WAVE header for 24-bit integer PCM.
pub fn wav_header(total_frames: u64, channels: u16, sample_rate: u32) -> Vec<u8> {
    let data_len = (total_frames * channels as u64 * BYTES_PER_SAMPLE) as u32;
    let byte_rate = sample_rate * channels as u32 * BYTES_PER_SAMPLE as u32;
    let block_align = channels * BYTES_PER_SAMPLE as u16;
    let mut h = Vec::with_capacity(HEADER_LEN as usize);
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
    h.extend_from_slice(&24u16.to_le_bytes()); // bits per sample
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    debug_assert_eq!(h.len() as u64, HEADER_LEN);
    h
}

/// Encode interleaved f32 frames to little-endian 24-bit PCM, appending to
/// `out`. Values are hard-clamped; no dither (inaudible at 24-bit).
pub fn frames_to_pcm24(frames: &[f32], out: &mut Vec<u8>) {
    out.reserve(frames.len() * 3);
    for &s in frames {
        let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
        let b = v.to_le_bytes();
        out.extend_from_slice(&b[0..3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_size_math_agree() {
        let frames = 176_400u64 * 3; // 3 seconds
        let h = wav_header(frames, 2, 176_400);
        assert_eq!(h.len(), 44);
        assert_eq!(wav_total_size(frames, 2), 44 + frames * 2 * 3);
        // RIFF size field = total - 8.
        let riff = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as u64;
        assert_eq!(riff, wav_total_size(frames, 2) - 8);
    }

    #[test]
    fn pcm24_encoding_clamps_and_scales() {
        let mut out = Vec::new();
        frames_to_pcm24(&[0.0, 1.0, -1.0, 2.0], &mut out);
        assert_eq!(out.len(), 12);
        assert_eq!(&out[0..3], &[0, 0, 0]);
        let max = i32::from_le_bytes([out[3], out[4], out[5], 0]);
        assert_eq!(max, 8_388_607);
        // -1.0 → -8388607 in 24-bit two's complement (low 3 bytes).
        let neg = i32::from_le_bytes([out[6], out[7], out[8], 0xFF]);
        assert_eq!(neg, -8_388_607);
        assert_eq!(&out[9..12], &out[3..6]); // 2.0 clamps to full scale
    }
}
