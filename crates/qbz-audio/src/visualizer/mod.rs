//! Audio Visualizer Module
//!
//! Provides real-time FFT analysis for audio visualization without affecting bit-perfect playback.
//! Uses a lockless ring buffer to capture samples from the audio thread.
//!
//! This module contains the core types needed by the player:
//! - RingBuffer: Lockless ring buffer for sample capture
//! - TappedSource: Audio source wrapper that taps samples
//! - VisualizerTap: Shared state for visualization
//!
//! The Tauri-specific FFT thread and event emission remain in qbz-nix.

mod processor;
mod ring_buffer;
mod scope;
mod tapped_source;

pub use processor::{spawn_visualizer_thread, VizFrame, VizSink};
pub use ring_buffer::RingBuffer;
pub use scope::{GONIOMETER_BIT, OSCILLOSCOPE_BIT};
pub use tapped_source::TappedSource;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// Number of frequency bins to send to frontend
pub const NUM_BARS: usize = 16;

/// FFT size (must be power of 2). 4096 gives ~10.7 Hz bin resolution at 44.1 kHz,
/// enabling detailed spectral analysis for visualizers like Linebed.
pub const FFT_SIZE: usize = 4096;

/// Target frames per second for visualization updates
pub const TARGET_FPS: u64 = 30;

/// History retained for output-playhead alignment.
///
/// ALSA Direct currently allows up to 500 ms of queued PCM at high sample
/// rates. 1,048,576 interleaved samples cover that delay plus the FFT window
/// for stereo PCM through 768 kHz, while keeping the audio-thread write path
/// allocation-free.
const RING_HISTORY_SAMPLES: usize = 1_048_576;

/// Shared state for visualization that can be passed to the audio thread
#[derive(Clone)]
pub struct VisualizerTap {
    /// Ring buffer for sample capture
    pub ring_buffer: Arc<RingBuffer>,
    /// Whether visualization is enabled
    pub enabled: Arc<AtomicBool>,
    /// Whether playback is paused. While `enabled && paused` the FFT producer
    /// parks instead of re-FFTing the stale ring buffer at TARGET_FPS (the
    /// buffer receives no new samples while the player is paused/stopped).
    /// Consumer-side gate only — `push()` does NOT check it, so the
    /// sample-submit path is untouched. Defaults to `false` (not paused) so
    /// frontends that never wire it keep the historical behavior.
    pub paused: Arc<AtomicBool>,
    /// Current sample rate
    pub sample_rate: Arc<AtomicU32>,
    /// Requested real-time scope producers. Frontends update this off the
    /// audio thread; zero keeps the added DSP completely idle.
    pub scope_mask: Arc<AtomicU32>,
    /// Number of interleaved samples queued ahead of the audible playhead.
    /// Zero for callback-paced/shared backends; ALSA Direct updates it from
    /// `snd_pcm_delay` without changing the PCM stream.
    playback_lag_samples: Arc<AtomicUsize>,
}

impl VisualizerTap {
    /// Create a new tap
    pub fn new() -> Self {
        Self {
            ring_buffer: Arc::new(RingBuffer::new(RING_HISTORY_SAMPLES)),
            enabled: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            sample_rate: Arc::new(AtomicU32::new(44100)),
            scope_mask: Arc::new(AtomicU32::new(0)),
            playback_lag_samples: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Check if visualization is enabled (fast atomic check)
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Push a sample (only if enabled)
    #[inline]
    pub fn push(&self, sample: f32) {
        if self.is_enabled() {
            self.ring_buffer.push(sample);
        }
    }

    /// Enable or disable visualization
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Check if playback is paused (fast atomic check)
    #[inline]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Mark playback as paused/resumed. While paused (and enabled) the FFT
    /// producer parks on a bounded timeout instead of burning CPU on the
    /// stale buffer; it self-wakes within that bound after a resume, so no
    /// caller is required to unpark it. Atomic store — never blocks.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Update the sample rate
    pub fn set_sample_rate(&self, rate: u32) {
        self.sample_rate.store(rate, Ordering::Relaxed);
    }

    /// Select the scope DSP that the visible frontend surfaces consume.
    pub fn set_scope_mask(&self, mask: u32) {
        self.scope_mask.store(mask, Ordering::Relaxed);
    }

    /// Align snapshots with a direct backend's audible playhead.
    ///
    /// `queued_frames` comes from the output API, so convert it to the
    /// interleaved-sample units used by [`RingBuffer`]. Saturation keeps a
    /// malformed device report from wrapping; the ring clamps to retained
    /// history when the snapshot is taken.
    pub fn set_output_delay_frames(&self, queued_frames: u64, channels: u16) {
        let samples = queued_frames.saturating_mul(u64::from(channels));
        self.playback_lag_samples.store(
            usize::try_from(samples).unwrap_or(usize::MAX),
            Ordering::Relaxed,
        );
    }

    /// Return callback-paced/shared backends to the newest captured window.
    pub fn clear_output_delay(&self) {
        self.playback_lag_samples.store(0, Ordering::Relaxed);
    }

    pub(crate) fn output_delay_samples(&self) -> usize {
        self.playback_lag_samples.load(Ordering::Relaxed)
    }
}

impl Default for VisualizerTap {
    fn default() -> Self {
        Self::new()
    }
}
