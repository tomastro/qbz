use std::f32::consts::PI;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

const MIN_FREQ_HZ: f32 = 20.0;
const MAX_FREQ_HZ: f32 = 20_000.0;

/// Progressive spectral analyzer for the immersive Spectral Ribbon visualizer.
///
/// Design choices:
/// - FFT size defaults to 1024: better low-frequency detail than 512, while still
///   remaining lightweight for a 20-30Hz UI update cadence.
/// - No allocation in hot path: all buffers are pre-allocated.
/// - Output is compact and normalized (Vec<f32> bands), suitable for Tauri events.
pub struct SpectralAnalyzer {
    pub update_rate_hz: u32,
    pub fft_size: usize,
    pub smoothing_factor: f32,

    sample_rate_hz: u32,
    num_bands: usize,
    frame_interval: Duration,
    last_update: Instant,

    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    fft_input: Vec<Complex32>,
    magnitudes: Vec<f32>,
    band_bin_ranges: Vec<(usize, usize)>,
    bands_raw: Vec<f32>,
    bands_smoothed: Vec<f32>,
    latest_bands: Vec<f32>,
}

impl SpectralAnalyzer {
    pub fn new(
        sample_rate_hz: u32,
        fft_size: usize,
        num_bands: usize,
        update_rate_hz: u32,
        smoothing_factor: f32,
    ) -> Self {
        let clamped_fft = match fft_size {
            512 | 1024 | 2048 | 4096 | 8192 => fft_size,
            _ => 1024,
        };
        let clamped_bands = num_bands.clamp(48, 1024);
        let clamped_rate = update_rate_hz.clamp(20, 60);
        let clamped_smoothing = smoothing_factor.clamp(0.0, 0.98);

        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(clamped_fft);

        let mut analyzer = Self {
            update_rate_hz: clamped_rate,
            fft_size: clamped_fft,
            smoothing_factor: clamped_smoothing,
            sample_rate_hz,
            num_bands: clamped_bands,
            frame_interval: Duration::from_secs_f32(1.0 / clamped_rate as f32),
            last_update: Instant::now() - Duration::from_secs(1),
            window: vec![0.0; clamped_fft],
            fft,
            fft_input: vec![Complex32::default(); clamped_fft],
            magnitudes: vec![0.0; clamped_fft / 2],
            band_bin_ranges: vec![(0, 0); clamped_bands],
            bands_raw: vec![0.0; clamped_bands],
            bands_smoothed: vec![0.0; clamped_bands],
            latest_bands: vec![0.0; clamped_bands],
        };

        analyzer.rebuild_window();
        analyzer.rebuild_band_ranges(sample_rate_hz);
        analyzer
    }

    /// Process a mono frame and update spectral bands if cadence allows it.
    ///
    /// Returns true if `latest_bands` was refreshed on this call.
    pub fn process_audio_frame(&mut self, mono_samples: &[f32], sample_rate_hz: u32) -> bool {
        if mono_samples.len() < self.fft_size {
            return false;
        }

        let now = Instant::now();
        if now.duration_since(self.last_update) < self.frame_interval {
            return false;
        }
        self.last_update = now;

        if sample_rate_hz != self.sample_rate_hz {
            self.sample_rate_hz = sample_rate_hz;
            self.rebuild_band_ranges(sample_rate_hz);
        }

        for (i, sample) in mono_samples.iter().take(self.fft_size).enumerate() {
            self.fft_input[i] = Complex32::new(*sample * self.window[i], 0.0);
        }
        self.fft.process(&mut self.fft_input);

        // Magnitudes for Nyquist half, normalized by FFT size.
        let fft_norm = 1.0 / self.fft_size as f32;
        for (i, value) in self.fft_input.iter().take(self.fft_size / 2).enumerate() {
            self.magnitudes[i] = value.norm() * fft_norm;
        }

        for band_idx in 0..self.num_bands {
            let (start_bin, end_bin) = self.band_bin_ranges[band_idx];
            if end_bin <= start_bin {
                self.bands_raw[band_idx] = 0.0;
                continue;
            }

            let mut sum = 0.0f32;
            let mut count = 0u32;
            for bin in start_bin..end_bin {
                let m = self.magnitudes[bin];
                sum += m * m;
                count += 1;
            }

            // RMS energy with linear scaling — no power compression so the
            // frontend's exponential transform has full dynamic range to work with.
            let rms = if count > 0 {
                (sum / count as f32).sqrt()
            } else {
                0.0
            };
            let scaled = (rms * 25.0).clamp(0.0, 1.0);
            self.bands_raw[band_idx] = scaled;
        }

        // Exponential smoothing with fast attack / slower release.
        for i in 0..self.num_bands {
            let new_value = self.bands_raw[i];
            let prev = self.bands_smoothed[i];
            let alpha = if new_value > prev {
                1.0 - self.smoothing_factor * 0.5
            } else {
                1.0 - self.smoothing_factor
            };
            let smoothed = prev + alpha * (new_value - prev);
            self.bands_smoothed[i] = smoothed;
            self.latest_bands[i] = smoothed.clamp(0.0, 1.0);
        }

        true
    }

    pub fn get_latest_bands(&self) -> &[f32] {
        &self.latest_bands
    }

    fn rebuild_window(&mut self) {
        // Hann window coefficients: w[n] = 0.5 * (1 - cos(2πn/(N-1))).
        let denom = (self.fft_size - 1) as f32;
        for n in 0..self.fft_size {
            self.window[n] = 0.5 * (1.0 - (2.0 * PI * (n as f32) / denom).cos());
        }
    }

    fn rebuild_band_ranges(&mut self, sample_rate_hz: u32) {
        let nyquist = sample_rate_hz as f32 * 0.5;
        let max_freq = MAX_FREQ_HZ.min(nyquist.max(MIN_FREQ_HZ + 1.0));
        let min_log = MIN_FREQ_HZ.ln();
        let max_log = max_freq.ln();
        let bin_hz = sample_rate_hz as f32 / self.fft_size as f32;
        let max_bin = self.magnitudes.len().saturating_sub(1);

        for band_idx in 0..self.num_bands {
            let t0 = band_idx as f32 / self.num_bands as f32;
            let t1 = (band_idx + 1) as f32 / self.num_bands as f32;

            let low_hz = (min_log + (max_log - min_log) * t0).exp();
            let high_hz = (min_log + (max_log - min_log) * t1).exp();

            let mut start_bin = (low_hz / bin_hz).floor() as usize;
            let mut end_bin = (high_hz / bin_hz).ceil() as usize;

            start_bin = start_bin.min(max_bin);
            end_bin = end_bin.min(max_bin.saturating_add(1));
            if end_bin <= start_bin {
                end_bin = (start_bin + 1).min(max_bin.saturating_add(1));
            }

            self.band_bin_ranges[band_idx] = (start_bin, end_bin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SpectralAnalyzer;

    #[test]
    fn spectral_analyzer_returns_expected_band_count() {
        let mut analyzer = SpectralAnalyzer::new(48_000, 1024, 64, 24, 0.8);
        let frame = vec![0.0f32; 1024];
        let _ = analyzer.process_audio_frame(&frame, 48_000);
        assert_eq!(analyzer.get_latest_bands().len(), 64);
    }
}
