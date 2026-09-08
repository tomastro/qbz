//! Loudness normalization support
//!
//! Extracts ReplayGain metadata from audio files and calculates gain factors
//! for volume normalization. When normalization is disabled, this module is
//! not invoked and the audio pipeline remains bit-perfect.

use std::io::{Cursor, Read, Seek, SeekFrom};
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::FormatReader;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::{MetadataOptions, StandardTagKey, Tag, Value};
use symphonia::core::probe::Hint;
use symphonia::default::get_probe;

/// Extracted loudness data for a track
#[derive(Debug, Clone)]
pub struct ReplayGainData {
    /// Gain adjustment in dB (negative = reduce volume, positive = increase)
    pub gain_db: f32,
    /// Peak sample value (0.0-1.0+), used for clipping prevention
    pub peak: Option<f32>,
}

/// Wrapper to make Cursor<Vec<u8>> implement MediaSource
struct CursorMediaSource {
    inner: Cursor<Vec<u8>>,
}

impl CursorMediaSource {
    fn new(data: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(data),
        }
    }
}

impl Read for CursorMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for CursorMediaSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl MediaSource for CursorMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.inner.get_ref().len() as u64)
    }
}

/// Extract ReplayGain metadata from raw audio file bytes.
///
/// Searches for ReplayGain track gain and peak values in:
/// - Vorbis comments (FLAC, Ogg): `REPLAYGAIN_TRACK_GAIN`, `REPLAYGAIN_TRACK_PEAK`
/// - ID3v2 TXXX frames: `replaygain_track_gain`
/// - Standard tag keys mapped by Symphonia
///
/// Returns `None` if no ReplayGain metadata is found.
pub fn extract_replaygain(data: &[u8]) -> Option<ReplayGainData> {
    let source = Box::new(CursorMediaSource::new(data.to_vec())) as Box<dyn MediaSource>;
    let mss = MediaSourceStream::new(source, Default::default());

    let mut hint = Hint::new();
    // Help Symphonia detect isomp4/m4a
    if data.len() >= 12 && &data[4..8] == b"ftyp" {
        hint.with_extension("m4a");
    }

    let format_opts = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    let metadata_opts: MetadataOptions = Default::default();

    let mut probed = match get_probe().format(&hint, mss, &format_opts, &metadata_opts) {
        Ok(p) => p,
        Err(e) => {
            log::debug!("Loudness: probe failed for ReplayGain extraction: {}", e);
            return None;
        }
    };

    // Collect all tags from both the probe metadata and the format reader metadata
    let mut gain_db: Option<f32> = None;
    let mut peak: Option<f32> = None;

    // Check probe-level metadata (container-level tags)
    if let Some(metadata) = probed.metadata.get() {
        if let Some(rev) = metadata.current() {
            extract_from_tags(rev.tags(), &mut gain_db, &mut peak);
        }
    }

    // Check format-level metadata (in-stream tags, e.g., Vorbis comments in FLAC)
    if gain_db.is_none() {
        let fmt_metadata = probed.format.metadata();
        if let Some(rev) = fmt_metadata.current() {
            extract_from_tags(rev.tags(), &mut gain_db, &mut peak);
        }
    }

    gain_db.map(|db| {
        log::info!("Loudness: found ReplayGain: {:.2} dB, peak: {:?}", db, peak);
        ReplayGainData { gain_db: db, peak }
    })
}

/// Extract ReplayGain data from a Symphonia FormatReader (for streaming sources).
///
/// This is used when we already have a probed format reader and don't want
/// to re-probe the data.
pub fn extract_replaygain_from_reader(format: &mut dyn FormatReader) -> Option<ReplayGainData> {
    let mut gain_db: Option<f32> = None;
    let mut peak: Option<f32> = None;

    let metadata = format.metadata();
    if let Some(rev) = metadata.current() {
        extract_from_tags(rev.tags(), &mut gain_db, &mut peak);
    }

    gain_db.map(|db| {
        log::info!(
            "Loudness: found ReplayGain (streaming): {:.2} dB, peak: {:?}",
            db,
            peak
        );
        ReplayGainData { gain_db: db, peak }
    })
}

/// Search tags for ReplayGain values.
fn extract_from_tags(tags: &[Tag], gain_db: &mut Option<f32>, peak: &mut Option<f32>) {
    for tag in tags {
        // Check Symphonia's standard tag key mapping first
        if let Some(std_key) = tag.std_key {
            match std_key {
                StandardTagKey::ReplayGainTrackGain => {
                    if let Some(g) = parse_gain_value(&tag.value) {
                        *gain_db = Some(g);
                    }
                }
                StandardTagKey::ReplayGainTrackPeak => {
                    if let Some(p) = parse_peak_value(&tag.value) {
                        *peak = Some(p);
                    }
                }
                _ => {}
            }
        }

        // Also check raw tag keys (case-insensitive) for formats where
        // Symphonia might not map to StandardTagKey
        let key_lower = tag.key.to_lowercase();
        match key_lower.as_str() {
            "replaygain_track_gain" => {
                if gain_db.is_none() {
                    if let Some(g) = parse_gain_value(&tag.value) {
                        *gain_db = Some(g);
                    }
                }
            }
            "replaygain_track_peak" => {
                if peak.is_none() {
                    if let Some(p) = parse_peak_value(&tag.value) {
                        *peak = Some(p);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Parse a ReplayGain gain value like "-6.54 dB" or "-6.54"
fn parse_gain_value(value: &Value) -> Option<f32> {
    let s = value_to_string(value)?;
    // Strip " dB" suffix if present, then parse
    let trimmed = s
        .trim()
        .trim_end_matches(" dB")
        .trim_end_matches(" db")
        .trim_end_matches("dB");
    trimmed.parse::<f32>().ok()
}

/// Parse a ReplayGain peak value like "0.988553" or "1.0"
fn parse_peak_value(value: &Value) -> Option<f32> {
    let s = value_to_string(value)?;
    s.trim().parse::<f32>().ok()
}

/// Convert a Symphonia Value to a string representation
fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Float(f) => Some(f.to_string()),
        Value::SignedInt(i) => Some(i.to_string()),
        Value::UnsignedInt(u) => Some(u.to_string()),
        _ => None,
    }
}

/// Convert a gain in dB to a linear amplitude factor.
///
/// gain_db = 0.0  → factor = 1.0 (no change)
/// gain_db = -6.0 → factor ≈ 0.501 (half amplitude)
/// gain_db = +6.0 → factor ≈ 1.995 (double amplitude)
#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    10_f32.powf(db / 20.0)
}

/// Calculate the normalization gain factor for a track.
///
/// Takes ReplayGain metadata and a target LUFS level, returns the linear
/// gain factor to apply to samples. Includes clipping prevention using
/// peak data when available.
///
/// The ReplayGain standard targets -18 LUFS (83 dB SPL). If the user's
/// target differs, we adjust accordingly.
///
/// # Arguments
/// * `rg` - ReplayGain metadata extracted from the track
/// * `target_lufs` - User's target loudness (e.g., -14.0, -18.0, -23.0)
///
/// # Returns
/// Linear gain factor to multiply samples by
pub fn calculate_gain_factor(rg: &ReplayGainData, target_lufs: f32) -> f32 {
    // ReplayGain reference level is -18 LUFS (EBU R128 / ReplayGain 2.0)
    const REPLAYGAIN_REFERENCE_LUFS: f32 = -18.0;

    // Adjust gain for the user's target level
    // If target is -14 LUFS (louder than reference), we need to add +4 dB
    // If target is -23 LUFS (quieter), we need to subtract -5 dB
    let target_adjustment = target_lufs - REPLAYGAIN_REFERENCE_LUFS;
    let adjusted_gain_db = rg.gain_db + target_adjustment;

    let mut gain = db_to_linear(adjusted_gain_db);

    // Clipping prevention: if we have peak data, cap the gain so
    // the loudest sample doesn't exceed 1.0
    if let Some(peak) = rg.peak {
        if peak > 0.0 {
            let max_safe_gain = 1.0 / peak;
            if gain > max_safe_gain {
                log::debug!(
                    "Loudness: capping gain from {:.3} to {:.3} (peak: {:.4})",
                    gain,
                    max_safe_gain,
                    peak
                );
                gain = max_safe_gain;
            }
        }
    }

    // Without peak data, cap at +6 dB maximum (conservative)
    if rg.peak.is_none() {
        let max_gain = db_to_linear(6.0);
        if gain > max_gain {
            log::debug!("Loudness: capping gain to +6 dB (no peak data)");
            gain = max_gain;
        }
    }

    log::debug!(
        "Loudness: gain_db={:.2}, target={:.1} LUFS, adjusted={:.2} dB, factor={:.4}",
        rg.gain_db,
        target_lufs,
        adjusted_gain_db,
        gain
    );

    gain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_to_linear() {
        // 0 dB = factor 1.0
        assert!((db_to_linear(0.0) - 1.0).abs() < 0.001);
        // -6 dB ≈ 0.501
        assert!((db_to_linear(-6.0) - 0.501).abs() < 0.01);
        // +6 dB ≈ 1.995
        assert!((db_to_linear(6.0) - 1.995).abs() < 0.01);
        // -20 dB = 0.1
        assert!((db_to_linear(-20.0) - 0.1).abs() < 0.001);
    }

    #[test]
    fn test_calculate_gain_factor_at_reference() {
        // At -18 LUFS target (ReplayGain reference), gain_db should pass through directly
        let rg = ReplayGainData {
            gain_db: -3.0,
            peak: Some(0.9),
        };
        let factor = calculate_gain_factor(&rg, -18.0);
        // -3 dB → ~0.708
        assert!((factor - 0.708).abs() < 0.01);
    }

    #[test]
    fn test_calculate_gain_factor_with_target_adjustment() {
        // At -14 LUFS target, we add +4 dB to the RG gain
        let rg = ReplayGainData {
            gain_db: -3.0,
            peak: Some(0.5),
        };
        let factor = calculate_gain_factor(&rg, -14.0);
        // -3 + 4 = +1 dB → ~1.122
        assert!((factor - 1.122).abs() < 0.01);
    }

    #[test]
    fn test_clipping_prevention_with_peak() {
        // High positive gain but peak close to 1.0 — should be capped
        let rg = ReplayGainData {
            gain_db: 10.0,
            peak: Some(0.95),
        };
        let factor = calculate_gain_factor(&rg, -18.0);
        // max_safe_gain = 1/0.95 ≈ 1.053, which is less than db_to_linear(10) ≈ 3.162
        assert!((factor - (1.0 / 0.95)).abs() < 0.01);
    }

    #[test]
    fn test_clipping_prevention_without_peak() {
        // High gain without peak data — capped at +6 dB
        let rg = ReplayGainData {
            gain_db: 12.0,
            peak: None,
        };
        let factor = calculate_gain_factor(&rg, -18.0);
        assert!((factor - db_to_linear(6.0)).abs() < 0.01);
    }

    #[test]
    fn test_parse_gain_value_formats() {
        // Standard format: "-6.54 dB"
        assert!(
            (parse_gain_value(&Value::String("-6.54 dB".to_string())).unwrap() - (-6.54)).abs()
                < 0.001
        );
        // Without dB suffix
        assert!(
            (parse_gain_value(&Value::String("-6.54".to_string())).unwrap() - (-6.54)).abs()
                < 0.001
        );
        // Positive
        assert!(
            (parse_gain_value(&Value::String("+3.21 dB".to_string())).unwrap() - 3.21).abs()
                < 0.001
        );
        // Float value
        assert!((parse_gain_value(&Value::Float(-6.54)).unwrap() - (-6.54)).abs() < 0.001);
    }

    #[test]
    fn test_parse_peak_value() {
        assert!(
            (parse_peak_value(&Value::String("0.988553".to_string())).unwrap() - 0.988553).abs()
                < 0.0001
        );
        assert!((parse_peak_value(&Value::Float(0.95)).unwrap() - 0.95).abs() < 0.001);
    }
}
