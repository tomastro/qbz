//! Real-time scope DSP fed by the visualizer ring-buffer snapshot.
//!
//! The processor is downstream of the passive audio tap. It never changes the
//! playback stream and runs only when a frontend requests one of the scope bits.

use spectrum_analyzer::scaling::divide_by_N_sqrt;
use spectrum_analyzer::windows::hann_window;
use spectrum_analyzer::{samples_fft_to_spectrum, FrequencyLimit};

pub const GONIOMETER_BIT: u32 = 0b01;
pub const OSCILLOSCOPE_BIT: u32 = 0b10;
pub const GONIOMETER_POINTS: usize = 256;
pub const OSCILLOSCOPE_POINTS: usize = 512;

const INV_SQRT_2: f32 = std::f32::consts::FRAC_1_SQRT_2;
const PITCH_MIN_HZ: f32 = 40.0;
const PITCH_MAX_HZ: f32 = 1_000.0;
const FIR_TAPS: usize = 257;

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn lowpass(sample_rate: u32, cutoff_hz: f32) -> Self {
        let sample_rate = sample_rate.max(1) as f32;
        let cutoff = cutoff_hz.clamp(1.0, sample_rate * 0.49);
        let omega = std::f32::consts::TAU * cutoff / sample_rate;
        let sin = omega.sin();
        let cos = omega.cos();
        let alpha = sin * INV_SQRT_2;
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos) * 0.5) / a0,
            b1: (1.0 - cos) / a0,
            b2: ((1.0 - cos) * 0.5) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, sample: f32) -> f32 {
        let out = self.b0 * sample + self.z1;
        self.z1 = self.b1 * sample - self.a1 * out + self.z2;
        self.z2 = self.b2 * sample - self.a2 * out;
        out
    }
}

pub struct ScopeProcessor {
    raw: Vec<f32>,
    conditioned: Vec<f32>,
    trigger: Vec<f32>,
    draw: Vec<f32>,
    fir: Vec<f32>,
    pitch_hz: f32,
    fir_center_hz: f32,
    fir_sample_rate: u32,
}

impl Default for ScopeProcessor {
    fn default() -> Self {
        Self {
            raw: Vec::new(),
            conditioned: Vec::new(),
            trigger: Vec::new(),
            draw: Vec::new(),
            fir: Vec::new(),
            pitch_hz: 0.0,
            fir_center_hz: 0.0,
            fir_sample_rate: 0,
        }
    }
}

impl ScopeProcessor {
    /// Mid/side points after two cascaded 8 kHz Butterworth low-pass stages,
    /// plus the correlation of the same full-rate stereo window.
    pub fn goniometer(&mut self, interleaved: &[f32], sample_rate: u32) -> (Box<[f32; 512]>, f32) {
        let frames = interleaved.len() / 2;
        let mut points = Box::new([0.0; GONIOMETER_POINTS * 2]);
        if frames == 0 {
            return (points, 0.0);
        }

        let mut ll1 = Biquad::lowpass(sample_rate, 8_000.0);
        let mut ll2 = Biquad::lowpass(sample_rate, 8_000.0);
        let mut rr1 = Biquad::lowpass(sample_rate, 8_000.0);
        let mut rr2 = Biquad::lowpass(sample_rate, 8_000.0);
        let step = (frames / GONIOMETER_POINTS).max(1);
        let first = frames.saturating_sub(step * GONIOMETER_POINTS);
        let mut next = first;
        let mut point = 0usize;
        let mut sum_lr = 0.0f64;
        let mut sum_l2 = 0.0f64;
        let mut sum_r2 = 0.0f64;

        for frame in 0..frames {
            let left = interleaved[frame * 2];
            let right = interleaved[frame * 2 + 1];
            sum_lr += (left * right) as f64;
            sum_l2 += (left * left) as f64;
            sum_r2 += (right * right) as f64;
            let left = ll2.process(ll1.process(left));
            let right = rr2.process(rr1.process(right));
            if frame >= next && point < GONIOMETER_POINTS {
                points[point * 2] = ((right - left) * INV_SQRT_2).clamp(-1.0, 1.0);
                points[point * 2 + 1] = ((right + left) * INV_SQRT_2).clamp(-1.0, 1.0);
                point += 1;
                next = next.saturating_add(step);
            }
        }

        let denom = (sum_l2 * sum_r2).sqrt();
        let correlation = if denom > f64::EPSILON {
            (sum_lr / denom).clamp(-1.0, 1.0) as f32
        } else {
            0.0
        };
        (points, correlation)
    }

    /// Pitch-locked mono oscilloscope. Pitch estimation and drawing use
    /// separate conditioned buffers; the narrow FIR is only redesigned after
    /// a material pitch change.
    pub fn oscilloscope(&mut self, interleaved: &[f32], sample_rate: u32) -> Box<[f32; 512]> {
        let frames = interleaved.len() / 2;
        let mut output = Box::new([0.0; OSCILLOSCOPE_POINTS]);
        if frames < 64 {
            return output;
        }
        self.resize(frames);
        for frame in 0..frames {
            self.raw[frame] = (interleaved[frame * 2] + interleaved[frame * 2 + 1]) * 0.5;
        }

        self.condition_for_pitch(sample_rate);
        let target_pitch = estimate_pitch(&self.conditioned, sample_rate);
        if let Some(target) = target_pitch {
            self.pitch_hz = if self.pitch_hz <= 0.0
                || (target / self.pitch_hz).max(self.pitch_hz / target) > 1.8
            {
                target
            } else {
                self.pitch_hz * 0.82 + target * 0.18
            };
        }
        let pitch = self.pitch_hz.clamp(PITCH_MIN_HZ, PITCH_MAX_HZ);
        if self.fir.is_empty()
            || self.fir_sample_rate != sample_rate
            || relative_change(pitch, self.fir_center_hz) > 0.05
        {
            self.fir = design_bandpass(pitch, sample_rate, FIR_TAPS);
            self.fir_center_hz = pitch;
            self.fir_sample_rate = sample_rate;
        }

        convolve(&self.conditioned, &self.fir, &mut self.trigger);
        lowpass_draw(&self.raw, sample_rate, &mut self.draw);

        let period = (sample_rate.max(1) as f32 / pitch).clamp(2.0, frames as f32);
        let span = (period * 2.0).clamp(96.0, (frames.saturating_sub(4)) as f32);
        let required = span.ceil() as usize + 2;
        let latest = frames.saturating_sub(required).max(1);
        let trigger_rms = rms(&self.trigger);
        let quarter = (period * 0.25).round().max(1.0) as usize;
        let threshold = (trigger_rms * 0.18).max(0.0005);
        let trigger = rising_trigger(&self.trigger, latest, quarter, threshold)
            .unwrap_or_else(|| frames.saturating_sub(required) as f32);
        let gain = (0.22 / rms(&self.draw).max(0.015)).clamp(0.5, 6.0);

        for (index, sample) in output.iter_mut().enumerate() {
            let position = trigger + span * index as f32 / (OSCILLOSCOPE_POINTS - 1) as f32;
            *sample = (interpolate(&self.draw, position) * gain).clamp(-1.0, 1.0);
        }
        output
    }

    fn resize(&mut self, frames: usize) {
        self.raw.resize(frames, 0.0);
        self.conditioned.resize(frames, 0.0);
        self.trigger.resize(frames, 0.0);
        self.draw.resize(frames, 0.0);
    }

    fn condition_for_pitch(&mut self, sample_rate: u32) {
        let rate = sample_rate.max(1) as f32;
        let low_alpha = (-std::f32::consts::TAU * 400.0 / rate).exp();
        let mut shelf_low = 0.0f32;
        let mut lp1 = Biquad::lowpass(sample_rate, 18_000.0);
        let mut lp2 = Biquad::lowpass(sample_rate, 18_000.0);
        for (input, output) in self.raw.iter().zip(self.conditioned.iter_mut()) {
            shelf_low = shelf_low * low_alpha + *input * (1.0 - low_alpha);
            let high = *input - shelf_low;
            *output = lp2.process(lp1.process(shelf_low + high * 0.707_106_77));
        }
    }
}

fn estimate_pitch(samples: &[f32], sample_rate: u32) -> Option<f32> {
    if samples.len() < 64 || rms(samples) < 0.002 {
        return None;
    }
    let windowed = hann_window(samples);
    let spectrum = samples_fft_to_spectrum(
        &windowed,
        sample_rate.max(1),
        FrequencyLimit::Range(PITCH_MIN_HZ, PITCH_MAX_HZ),
        Some(&divide_by_N_sqrt),
    )
    .ok()?;
    spectrum
        .data()
        .iter()
        .max_by(|(fa, ma), (fb, mb)| {
            let a = ma.val() / fa.val().max(PITCH_MIN_HZ).sqrt();
            let b = mb.val() / fb.val().max(PITCH_MIN_HZ).sqrt();
            a.total_cmp(&b)
        })
        .map(|(frequency, _)| frequency.val())
}

fn relative_change(a: f32, b: f32) -> f32 {
    if a <= 0.0 || b <= 0.0 {
        return f32::INFINITY;
    }
    (a - b).abs() / b
}

fn design_bandpass(center_hz: f32, sample_rate: u32, taps: usize) -> Vec<f32> {
    let rate = sample_rate.max(1) as f32;
    let half_width = (center_hz * 0.05).max(rate / taps as f32);
    let low = (center_hz - half_width).max(PITCH_MIN_HZ * 0.5) / rate;
    let high = (center_hz + half_width).min(rate * 0.48) / rate;
    let middle = (taps - 1) as f32 * 0.5;
    let mut kernel = Vec::with_capacity(taps);
    for index in 0..taps {
        let m = index as f32 - middle;
        let ideal = if m.abs() < f32::EPSILON {
            2.0 * (high - low)
        } else {
            ((std::f32::consts::TAU * high * m).sin() - (std::f32::consts::TAU * low * m).sin())
                / (std::f32::consts::PI * m)
        };
        let window = 0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / (taps - 1) as f32).cos();
        kernel.push(ideal * window);
    }
    let omega = std::f32::consts::TAU * center_hz / rate;
    let (real, imag) = kernel
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(real, imag), (i, tap)| {
            let phase = omega * i as f32;
            (real + tap * phase.cos(), imag - tap * phase.sin())
        });
    let gain = (real * real + imag * imag).sqrt().max(1.0e-6);
    for tap in &mut kernel {
        *tap /= gain;
    }
    kernel
}

fn convolve(input: &[f32], kernel: &[f32], output: &mut [f32]) {
    for (index, out) in output.iter_mut().enumerate() {
        let count = kernel.len().min(index + 1);
        let mut sum = 0.0;
        for tap in 0..count {
            sum += input[index - tap] * kernel[tap];
        }
        *out = sum;
    }
}

fn lowpass_draw(input: &[f32], sample_rate: u32, output: &mut [f32]) {
    let mut filter = Biquad::lowpass(sample_rate, 12_000.0);
    for (input, output) in input.iter().zip(output.iter_mut()) {
        *output = filter.process(*input);
    }
}

fn rising_trigger(signal: &[f32], latest: usize, quarter: usize, threshold: f32) -> Option<f32> {
    for index in (1..=latest.min(signal.len().saturating_sub(2))).rev() {
        let before = signal[index - 1];
        let after = signal[index];
        let validation = index.saturating_add(quarter).min(signal.len() - 1);
        if before <= 0.0 && after > 0.0 && signal[validation].abs() >= threshold {
            let fraction = (-before / (after - before).max(f32::EPSILON)).clamp(0.0, 1.0);
            return Some(index as f32 - 1.0 + fraction);
        }
    }
    None
}

fn interpolate(samples: &[f32], position: f32) -> f32 {
    let base = position.floor().max(0.0) as usize;
    let next = (base + 1).min(samples.len().saturating_sub(1));
    let fraction = position - base as f32;
    samples.get(base).copied().unwrap_or(0.0) * (1.0 - fraction)
        + samples.get(next).copied().unwrap_or(0.0) * fraction
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_sine(sample_rate: u32, frequency: f32, phase: f32, inverted: bool) -> Vec<f32> {
        let mut samples = Vec::with_capacity(4096);
        for frame in 0..2048 {
            let value = (std::f32::consts::TAU * frequency * frame as f32 / sample_rate as f32
                + phase)
                .sin()
                * 0.5;
            samples.push(value);
            samples.push(if inverted { -value } else { value });
        }
        samples
    }

    #[test]
    fn goniometer_maps_mono_vertical_and_reports_positive_correlation() {
        let mut scope = ScopeProcessor::default();
        let (points, correlation) =
            scope.goniometer(&stereo_sine(48_000, 440.0, 0.0, false), 48_000);
        let max_side = points
            .iter()
            .step_by(2)
            .fold(0.0f32, |peak, value| peak.max(value.abs()));
        assert!(max_side < 0.001);
        assert!(correlation > 0.999);
    }

    #[test]
    fn goniometer_maps_inverted_stereo_horizontal_and_reports_negative_correlation() {
        let mut scope = ScopeProcessor::default();
        let (points, correlation) =
            scope.goniometer(&stereo_sine(48_000, 440.0, 0.0, true), 48_000);
        let max_mid = points
            .iter()
            .skip(1)
            .step_by(2)
            .fold(0.0f32, |peak, value| peak.max(value.abs()));
        assert!(max_mid < 0.001);
        assert!(correlation < -0.999);
    }

    #[test]
    fn oscilloscope_locks_a_tone_to_a_rising_crossing() {
        let mut scope = ScopeProcessor::default();
        let output = scope.oscilloscope(&stereo_sine(48_000, 440.0, 1.7, false), 48_000);
        assert!(
            scope.pitch_hz > 400.0 && scope.pitch_hz < 500.0,
            "pitch {}",
            scope.pitch_hz
        );
        assert!(output.iter().all(|value| value.is_finite()));
        assert!(output.iter().any(|value| value.abs() > 0.1));
    }
}
