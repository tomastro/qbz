//! Dynamic gain wrapper for real-time volume normalization.
//!
//! Reads gain from a shared `Arc<AtomicU32>` (f32 stored as bits) and applies
//! it to each sample. When the gain value changes, a 50ms linear ramp smooths
//! the transition to prevent audible clicks.
//!
//! When the atomic holds 0.0 (gain not yet computed), the wrapper stays at
//! the `initial_gain` provided at construction.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::Source;

pub struct DynamicAmplify<S>
where
    S: Source<Item = f32>,
{
    inner: S,
    gain_atomic: Arc<AtomicU32>,
    /// Current applied gain (smoothly ramped)
    current_gain: f32,
    /// Target gain we're ramping toward
    target_gain: f32,
    /// Gain increment per sample during ramp
    ramp_step: f32,
    /// Samples remaining in the current ramp
    ramp_remaining: u32,
    /// Number of samples in a 50ms ramp at the current sample rate
    ramp_samples: u32,
}

impl<S> DynamicAmplify<S>
where
    S: Source<Item = f32>,
{
    pub fn new(source: S, gain_atomic: Arc<AtomicU32>, initial_gain: f32) -> Self {
        let sample_rate = source.sample_rate().get();
        let channels = source.channels().get() as u32;
        // 50ms ramp in total samples (all channels)
        let ramp_samples = (sample_rate * channels * 50) / 1000;

        Self {
            inner: source,
            gain_atomic,
            current_gain: initial_gain,
            target_gain: initial_gain,
            ramp_step: 0.0,
            ramp_remaining: 0,
            ramp_samples,
        }
    }

    /// Check for a new gain value and start a ramp if it changed.
    #[inline]
    fn poll_gain(&mut self) {
        let bits = self.gain_atomic.load(Ordering::Relaxed);
        let new_gain = f32::from_bits(bits);

        // 0.0 means "not yet computed" — stay at current gain
        if new_gain == 0.0 {
            return;
        }

        // Only start a ramp if the target actually changed
        if (new_gain - self.target_gain).abs() > f32::EPSILON {
            self.target_gain = new_gain;
            if self.ramp_samples > 0 {
                self.ramp_step = (self.target_gain - self.current_gain) / self.ramp_samples as f32;
                self.ramp_remaining = self.ramp_samples;
            } else {
                self.current_gain = self.target_gain;
                self.ramp_remaining = 0;
            }
        }
    }
}

impl<S> Iterator for DynamicAmplify<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Poll for new gain every 1024 samples to avoid atomic contention
        // (ramp_remaining check is essentially free)
        if self.ramp_remaining == 0 {
            self.poll_gain();
        }

        let sample = self.inner.next()?;

        if self.ramp_remaining > 0 {
            self.current_gain += self.ramp_step;
            self.ramp_remaining -= 1;
            if self.ramp_remaining == 0 {
                // Snap to target at end of ramp to avoid float drift
                self.current_gain = self.target_gain;
            }
        }

        Some(sample * self.current_gain)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for DynamicAmplify<S>
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
