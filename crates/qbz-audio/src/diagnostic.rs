//! Audio bit-depth diagnostic
//!
//! Lock-free capture that accumulates an OR-mask of all sample values
//! converted to i32. The trailing zeros in the mask reveal the effective
//! bit depth of the source data — no sample storage needed.
//!
//! Works for both rodio (PipeWire/ALSA via CPAL) and ALSA Direct paths
//! via a transparent Source wrapper.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::Source;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Shared diagnostic state (atomics — safe to clone across threads)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AudioDiagnostic {
    capturing: Arc<AtomicBool>,
    or_mask: Arc<AtomicU32>,
    sample_count: Arc<AtomicU64>,
    sample_rate: Arc<AtomicU32>,
    channels: Arc<AtomicU32>,
}

impl AudioDiagnostic {
    pub fn new() -> Self {
        Self {
            capturing: Arc::new(AtomicBool::new(false)),
            or_mask: Arc::new(AtomicU32::new(0)),
            sample_count: Arc::new(AtomicU64::new(0)),
            sample_rate: Arc::new(AtomicU32::new(0)),
            channels: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Begin capturing. Resets previous state.
    pub fn start_capture(&self, sample_rate: u32, channels: u32) {
        self.or_mask.store(0, Ordering::SeqCst);
        self.sample_count.store(0, Ordering::SeqCst);
        self.sample_rate.store(sample_rate, Ordering::SeqCst);
        self.channels.store(channels, Ordering::SeqCst);
        self.capturing.store(true, Ordering::SeqCst);
        log::info!(
            "[Diagnostic] Bit-depth capture started ({}Hz, {}ch)",
            sample_rate,
            channels
        );
    }

    #[inline]
    pub fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::Relaxed)
    }

    /// Push a single sample (called per-sample in the Source wrapper).
    #[inline]
    pub fn push_sample(&self, sample: f32) {
        if !self.is_capturing() {
            return;
        }
        let clamped = sample.clamp(-1.0, 1.0);
        let s32 = (clamped * 2_147_483_647.0) as i32;
        self.or_mask.fetch_or(s32.unsigned_abs(), Ordering::Relaxed);
        self.sample_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Stop capturing and return the analysis.
    pub fn stop_and_analyze(&self) -> BitDepthResult {
        self.capturing.store(false, Ordering::SeqCst);

        let or_mask = self.or_mask.load(Ordering::SeqCst);
        let sample_count = self.sample_count.load(Ordering::SeqCst);
        let sample_rate = self.sample_rate.load(Ordering::SeqCst);
        let channels = self.channels.load(Ordering::SeqCst);

        let trailing_zeros = if or_mask == 0 {
            32
        } else {
            or_mask.trailing_zeros()
        };
        let effective_bits = 32 - trailing_zeros;

        let frames = if channels > 0 {
            sample_count / channels as u64
        } else {
            sample_count
        };
        let duration_secs = if sample_rate > 0 {
            frames as f64 / sample_rate as f64
        } else {
            0.0
        };

        log::info!(
            "[Diagnostic] Capture stopped: {} samples, {:.1}s, or_mask=0x{:08X}, trailing_zeros={}, effective_bits={}",
            sample_count, duration_secs, or_mask, trailing_zeros, effective_bits
        );

        BitDepthResult {
            sample_count,
            sample_rate,
            channels,
            duration_secs,
            or_mask: format!("0x{:08X}", or_mask),
            trailing_zeros,
            effective_bits,
        }
    }
}

impl Default for AudioDiagnostic {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Result returned to the frontend
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BitDepthResult {
    pub sample_count: u64,
    pub sample_rate: u32,
    pub channels: u32,
    pub duration_secs: f64,
    pub or_mask: String,
    pub trailing_zeros: u32,
    pub effective_bits: u32,
}

// ---------------------------------------------------------------------------
// Source wrapper — transparent tap for bit-depth capture
// ---------------------------------------------------------------------------

pub struct DiagnosticSource<S>
where
    S: Source<Item = f32>,
{
    inner: S,
    diagnostic: AudioDiagnostic,
}

impl<S> DiagnosticSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(source: S, diagnostic: AudioDiagnostic) -> Self {
        Self {
            inner: source,
            diagnostic,
        }
    }
}

impl<S> Iterator for DiagnosticSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        self.diagnostic.push_sample(sample);
        Some(sample)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for DiagnosticSource<S>
where
    S: Source<Item = f32>,
{
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    #[inline]
    fn channels(&self) -> std::num::NonZero<u16> {
        self.inner.channels()
    }

    #[inline]
    fn sample_rate(&self) -> std::num::NonZero<u32> {
        self.inner.sample_rate()
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}
