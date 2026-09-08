//! Frontend-agnostic FFT/visualizer producer.
//!
//! All DSP — Hann window, FFT, log-bars, energy bands, transient detection,
//! waveform downsample, and the spectral ribbon — lives here so any frontend
//! (Tauri, Slint, headless) can consume the same five typed streams without a
//! framework dependency. A [`VizSink`] is the only seam: the producer thread
//! computes a [`VizFrame`] and hands it to `sink.submit(...)`.
//!
//! This is strictly downstream of the lockless [`RingBuffer`](super::RingBuffer)
//! (the read-only tap on the bit-perfect stream); it touches none of the
//! protected audio device/stream path. See
//! `qbz-nix-docs/immersive-slint-handoff/recon/source/audio-transport.md`.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use spectrum_analyzer::scaling::divide_by_N_sqrt;
use spectrum_analyzer::windows::hann_window;
use spectrum_analyzer::{samples_fft_to_spectrum, FrequencyLimit};

use crate::SpectralAnalyzer;

use super::scope::{ScopeProcessor, GONIOMETER_BIT, OSCILLOSCOPE_BIT};
use super::{VisualizerTap, FFT_SIZE, NUM_BARS, TARGET_FPS};

/// Number of energy bands for the Energy Bands visualizer
const NUM_ENERGY_BANDS: usize = 5;
const NUM_SPECTRAL_BANDS: usize = 512;
const SPECTRAL_UPDATE_RATE_HZ: u32 = 58;
const SPECTRAL_SMOOTHING: f32 = 0.30;

/// Energy band frequency ranges (Hz):
/// Sub-bass (20-60), Bass (60-250), Mids (250-2k), Presence (2k-6k), Air (6k-20k)
const ENERGY_BAND_RANGES: [(f32, f32); NUM_ENERGY_BANDS] = [
    (20.0, 60.0),
    (60.0, 250.0),
    (250.0, 2000.0),
    (2000.0, 6000.0),
    (6000.0, 20000.0),
];

/// One frame of computed visualization data. Each variant corresponds to one of
/// the five streams the Tauri build historically emitted as `viz:*` events; the
/// payload is the decoded magnitudes, not the LE-f32 byte blob (the Tauri
/// adapter re-serializes those for backward compatibility).
#[derive(Clone, Debug)]
pub enum VizFrame {
    /// 16 log-spaced FFT bars (`viz:data`).
    Viz16([f32; 16]),
    /// 256 L samples followed by 256 R samples (`viz:waveform`).
    Wave256x2(Box<[f32; 512]>),
    /// `NUM_SPECTRAL_BANDS` spectral bands (`viz:spectral`).
    Spectral512(Vec<f32>),
    /// 5 energy bands (`viz:energy`).
    Energy5([f32; 5]),
    /// A single transient intensity, submitted only on detection (`viz:transient`).
    Transient1(f32),
    /// 256 packed mid/side points plus the stereo correlation coefficient.
    Goniometer {
        points: Box<[f32; 512]>,
        correlation: f32,
    },
    /// 512 pitch-locked mono samples.
    Oscilloscope(Box<[f32; 512]>),
}

/// Frontend-agnostic consumer of visualization frames. Implemented by the Tauri
/// adapter (re-emits the `viz:*` events) and the Slint adapter (latches frames
/// for the UI-thread drain).
pub trait VizSink: Send + Sync {
    fn submit(&self, frame: VizFrame);
}

/// Idle poll for the disabled AND paused states. Instead of spinning at
/// `TARGET_FPS` while the tap is off (or the player is paused and the ring
/// buffer is stale), the producer parks for this long between re-checks of the
/// `enabled`/`paused` atomics. An enable/resume path MAY `unpark()` the thread
/// (via the returned `JoinHandle`'s `.thread()`) for an instant wake — the
/// Slint frontend does; callers that don't still get picked up within this
/// bound (≤200ms, well under the ~250ms resume-latency budget).
const IDLE_POLL: Duration = Duration::from_millis(200);

/// Spawn the FFT processing thread. Idempotency is the caller's concern (the
/// `Visualizer`/shell guards against a double start). Returns the join handle;
/// callers that run for the app lifetime can drop it (or keep `.thread()` to
/// `unpark()` on enable, see [`IDLE_POLL`]).
pub fn spawn_visualizer_thread(tap: VisualizerTap, sink: Arc<dyn VizSink>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("visualizer-fft".to_string())
        .spawn(move || {
            run_fft_loop(tap, sink);
        })
        .expect("Failed to spawn visualizer thread")
}

/// Main FFT processing loop. Reads samples from the tap's ring buffer, computes
/// all five streams at `TARGET_FPS`, and submits them to the sink. The enabled
/// path (pacing, `sample_rate` reads, `SpectralAnalyzer` cadence, DSP) matches
/// the historical Tauri loop; while DISABLED or PAUSED the thread parks (see
/// [`IDLE_POLL`]) instead of spinning at `TARGET_FPS`.
fn run_fft_loop(tap: VisualizerTap, sink: Arc<dyn VizSink>) {
    // Pre-allocate all buffers to avoid allocations in the hot path
    let mut samples = vec![0.0f32; FFT_SIZE];
    let mut windowed = vec![0.0f32; FFT_SIZE];
    let mut output = vec![0.0f32; NUM_BARS];
    let mut smoothed = vec![0.0f32; NUM_BARS];

    // Waveform: 256 L + 256 R = 512 floats, written straight into the per-frame
    // Box (see the submit site — the Box itself must stay per-frame).
    const WAVEFORM_POINTS: usize = 256;

    // Energy bands state
    let mut energy_bands = [0.0f32; NUM_ENERGY_BANDS];
    let mut smoothed_energy = [0.0f32; NUM_ENERGY_BANDS];
    let mut spectral_analyzer = SpectralAnalyzer::new(
        tap.sample_rate.load(Ordering::Relaxed),
        FFT_SIZE,
        NUM_SPECTRAL_BANDS,
        SPECTRAL_UPDATE_RATE_HZ,
        SPECTRAL_SMOOTHING,
    );
    let mut scopes = ScopeProcessor::default();

    // Transient detection state
    let mut prev_rms = 0.0f32;
    let mut transient_cooldown = 0u32; // frames remaining in cooldown
    const TRANSIENT_THRESHOLD: f32 = 0.04; // RMS jump threshold (sensitive)
    const TRANSIENT_COOLDOWN_FRAMES: u32 = 3; // ~100ms at 30fps

    // Smoothing factor: 0 = no smoothing, higher = more smoothing
    const SMOOTHING: f32 = 0.65;

    let frame_duration = Duration::from_micros(1_000_000 / TARGET_FPS);

    loop {
        if !tap.enabled.load(Ordering::Relaxed) || tap.paused.load(Ordering::Relaxed) {
            // Disabled OR paused: park instead of pacing at TARGET_FPS doing
            // nothing (paused = the ring buffer receives no new samples, so
            // re-FFTing it would burn CPU on identical stale data).
            // Spurious wakeups are fine (the loop just re-checks the atomics);
            // the bounded timeout means no enable/resume path is REQUIRED to
            // unpark. Never blocks shutdown: the thread is detached and always
            // wakes.
            std::thread::park_timeout(IDLE_POLL);
            continue;
        }

        let frame_start = Instant::now();

        {
            let sample_rate = tap.sample_rate.load(Ordering::Relaxed);

            // Get samples from ring buffer
            tap.ring_buffer
                .snapshot_behind(&mut samples, tap.output_delay_samples());

            // The scope pipeline is explicitly requested by the visible
            // frontend destination. Its FIR and pitch FFT remain completely
            // idle for the existing bars/waveform/energy modes.
            let scope_mask = tap.scope_mask.load(Ordering::Relaxed);
            if scope_mask & GONIOMETER_BIT != 0 {
                let (points, correlation) = scopes.goniometer(&samples, sample_rate);
                sink.submit(VizFrame::Goniometer {
                    points,
                    correlation,
                });
            }
            if scope_mask & OSCILLOSCOPE_BIT != 0 {
                sink.submit(VizFrame::Oscilloscope(
                    scopes.oscilloscope(&samples, sample_rate),
                ));
            }

            // Compact, progressive spectrogram bands for the Spectral Ribbon,
            // gated on the analyzer's own update cadence.
            if spectral_analyzer.process_audio_frame(&samples, sample_rate) {
                let spectral = spectral_analyzer.get_latest_bands();
                sink.submit(VizFrame::Spectral512(spectral.to_vec()));
            }

            // Apply Hann window to reduce spectral leakage
            let window = hann_window(&samples);
            for (i, (sample, win)) in samples.iter().zip(window.iter()).enumerate() {
                windowed[i] = sample * win;
            }

            // Compute FFT spectrum
            match samples_fft_to_spectrum(
                &windowed,
                sample_rate,
                FrequencyLimit::Range(20.0, 20000.0),
                Some(&divide_by_N_sqrt),
            ) {
                Ok(spectrum) => {
                    // Map spectrum to logarithmic frequency bars
                    map_to_log_bars(&spectrum, &mut output);

                    // Apply smoothing for visual continuity
                    for i in 0..NUM_BARS {
                        let new = output[i];
                        // Faster attack, slower decay for punchy visuals
                        if new > smoothed[i] {
                            smoothed[i] = smoothed[i] * 0.3 + new * 0.7; // Fast attack
                        } else {
                            smoothed[i] = smoothed[i] * SMOOTHING + new * (1.0 - SMOOTHING);
                            // Slow decay
                        }
                        output[i] = smoothed[i];
                    }

                    let mut bars = [0.0f32; 16];
                    bars.copy_from_slice(&output);
                    sink.submit(VizFrame::Viz16(bars));

                    // --- Energy Bands: compute RMS per frequency band from spectrum ---
                    // ONE spectrum pass per band; the smoothed band value AND the
                    // transient raw RMS both derive from the same `compressed`
                    // aggregate (the former separate raw-RMS loop computed the
                    // identical per-band value — merged, not changed).
                    let data = spectrum.data();
                    let mut raw_sum = 0.0f32;
                    for (band_idx, &(lo, hi)) in ENERGY_BAND_RANGES.iter().enumerate() {
                        let mut sum_sq = 0.0f32;
                        let mut count = 0u32;
                        for (freq, magnitude) in data.iter() {
                            let f = freq.val();
                            if f >= lo && f < hi {
                                let mag = magnitude.val();
                                sum_sq += mag * mag;
                                count += 1;
                            }
                        }
                        let rms = if count > 0 {
                            (sum_sq / count as f32).sqrt()
                        } else {
                            0.0
                        };
                        // Compress and normalize
                        let compressed = (rms * 6.0).powf(0.5).clamp(0.0, 1.0);
                        // Smooth: fast attack, slow decay
                        if compressed > smoothed_energy[band_idx] {
                            smoothed_energy[band_idx] =
                                smoothed_energy[band_idx] * 0.2 + compressed * 0.8;
                        } else {
                            smoothed_energy[band_idx] =
                                smoothed_energy[band_idx] * 0.85 + compressed * 0.15;
                        }
                        energy_bands[band_idx] = smoothed_energy[band_idx];
                        // Transient feed: raw (pre-smoothed) value, bass/sub-bass
                        // weighted 2x for beat detection.
                        let weight = if band_idx < 2 { 2.0 } else { 1.0 };
                        raw_sum += compressed * weight;
                    }
                    sink.submit(VizFrame::Energy5(energy_bands));

                    // --- Transient Detection: detect sharp RMS jumps ---
                    let raw_rms = raw_sum / (NUM_ENERGY_BANDS as f32 + 2.0); // account for extra bass weight
                    let rms_delta = raw_rms - prev_rms;

                    if transient_cooldown > 0 {
                        transient_cooldown -= 1;
                    }

                    if rms_delta > TRANSIENT_THRESHOLD && transient_cooldown == 0 {
                        // Transient detected! Submit intensity (0.0 - 1.0)
                        let intensity = (rms_delta * 5.0).clamp(0.0, 1.0);
                        sink.submit(VizFrame::Transient1(intensity));
                        transient_cooldown = TRANSIENT_COOLDOWN_FRAMES;
                    }

                    prev_rms = raw_rms;
                }
                Err(e) => {
                    log::debug!("FFT error: {:?}", e);
                }
            }

            // Raw waveform data for the oscilloscope (stereo L/R).
            // samples[] is interleaved: L0, R0, L1, R1, ...
            // 1024 samples = 512 stereo pairs → downsample to 256 per channel
            // The Box allocation is load-bearing: ownership moves through the
            // sink into the UI's single-slot cell, so it cannot be reused here.
            let stereo_pairs = FFT_SIZE / 2; // 512
            let step = stereo_pairs / WAVEFORM_POINTS; // 512/256 = 2
            let mut wave = Box::new([0.0f32; 512]);
            for i in 0..WAVEFORM_POINTS {
                let base = i * step * 2; // index into interleaved buffer
                wave[i] = samples[base]; // L
                wave[WAVEFORM_POINTS + i] = samples[base + 1]; // R
            }
            sink.submit(VizFrame::Wave256x2(wave));
        }

        // Maintain target FPS
        let elapsed = frame_start.elapsed();
        if elapsed < frame_duration {
            std::thread::sleep(frame_duration - elapsed);
        }
    }
}

/// Map spectrum data to logarithmically-spaced frequency bars.
///
/// Human hearing is logarithmic, so we use log-spaced bars to match how we
/// perceive frequency. This gives equal visual weight to bass, mids, and treble.
fn map_to_log_bars(spectrum: &spectrum_analyzer::FrequencySpectrum, output: &mut [f32]) {
    let num_bars = output.len();

    // Frequency range (Hz)
    const MIN_FREQ: f32 = 20.0;
    const MAX_FREQ: f32 = 20000.0;

    let min_log = MIN_FREQ.ln();
    let max_log = MAX_FREQ.ln();

    // Get spectrum data
    let data = spectrum.data();

    for (i, bar) in output.iter_mut().enumerate() {
        // Calculate logarithmic frequency bounds for this bar
        let t_low = i as f32 / num_bars as f32;
        let t_high = (i + 1) as f32 / num_bars as f32;

        let freq_low = (min_log + (max_log - min_log) * t_low).exp();
        let freq_high = (min_log + (max_log - min_log) * t_high).exp();

        // Find all frequency bins that fall within this bar's range
        let mut sum = 0.0f32;
        let mut count = 0u32;

        for (freq, magnitude) in data.iter() {
            let f = freq.val();
            if f >= freq_low && f < freq_high {
                // Apply perceptual weighting (boost bass slightly)
                let weight = if f < 200.0 {
                    1.5 // Bass boost
                } else if f < 2000.0 {
                    1.0 // Mids
                } else {
                    0.8 // Reduce harsh highs
                };

                sum += magnitude.val() * weight;
                count += 1;
            }
        }

        // Average magnitude for this bar
        let avg = if count > 0 { sum / count as f32 } else { 0.0 };

        // Apply dynamic range compression and normalize.
        // This makes quiet passages more visible while preventing clipping.
        let compressed = (avg * 4.0).powf(0.6);
        *bar = compressed.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_frequency_distribution() {
        // Verify that frequency bars are logarithmically distributed
        let num_bars = NUM_BARS; // Use the actual constant
        let min_log = 20.0_f32.ln();
        let max_log = 20000.0_f32.ln();

        let mut freqs = Vec::new();
        for i in 0..num_bars {
            let t = i as f32 / num_bars as f32;
            let freq = (min_log + (max_log - min_log) * t).exp();
            freqs.push(freq);
        }

        // First bar should be around 20Hz
        assert!(freqs[0] > 19.0 && freqs[0] < 25.0);

        // Middle bar (~16 for 32 bars) should be around 630Hz (geometric mean of 20 and 20000)
        let mid = num_bars / 2;
        assert!(freqs[mid] > 500.0 && freqs[mid] < 800.0);

        // Last bar should approach 20000Hz (but won't reach it since t < 1.0)
        assert!(freqs[num_bars - 1] > 10000.0);
    }
}
