//! Lock-free Ring Buffer for Audio Sample Capture
//!
//! This implementation is designed to be called from the audio thread without
//! any blocking operations. The audio thread is the only writer, and the
//! visualizer thread is the only reader.
//!
//! CRITICAL: This must not affect bit-perfect playback in any way.
//! - No locks or mutexes
//! - No allocations
//! - Minimal atomic operations

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A lock-free single-producer single-consumer ring buffer
///
/// # Safety
/// This is safe because:
/// - Only one thread (audio) calls `push`
/// - Only one thread (visualizer) calls `snapshot`
/// - The buffer size is fixed at creation
pub struct RingBuffer {
    /// The actual sample storage
    buffer: UnsafeCell<Box<[f32]>>,
    /// Current write position (only modified by audio thread)
    write_pos: AtomicUsize,
    /// Buffer size
    size: usize,
}

// Safety: RingBuffer is designed for single-producer single-consumer use
unsafe impl Sync for RingBuffer {}
unsafe impl Send for RingBuffer {}

impl RingBuffer {
    /// Create a new ring buffer with the specified size
    pub fn new(size: usize) -> Self {
        Self {
            buffer: UnsafeCell::new(vec![0.0; size].into_boxed_slice()),
            write_pos: AtomicUsize::new(0),
            size,
        }
    }

    /// Push a sample to the buffer (called from audio thread)
    ///
    /// This is completely lock-free and will not block.
    /// If the visualizer is reading, it might get slightly stale data,
    /// which is perfectly fine for visualization purposes.
    #[inline]
    pub fn push(&self, sample: f32) {
        let pos = self.write_pos.fetch_add(1, Ordering::Relaxed) % self.size;

        // Safety: We're the only writer, and readers can tolerate slightly stale data
        unsafe {
            let buffer = &mut *self.buffer.get();
            buffer[pos] = sample;
        }
    }

    /// Take a snapshot of the most recent samples
    ///
    /// Copies the last `dest.len()` samples into the provided slice.
    /// The samples are ordered from oldest to newest.
    pub fn snapshot(&self, dest: &mut [f32]) {
        self.snapshot_behind(dest, 0);
    }

    /// Take a snapshot ending `lag_samples` behind the producer head.
    ///
    /// Direct output backends can queue a substantial amount of PCM ahead of
    /// the hardware playhead. The visualizer still taps the exact source
    /// samples, but asks for a historical window ending at the audible
    /// position instead of the newest queued sample. A request older than the
    /// retained ring is clamped to the oldest complete window; before enough
    /// samples exist, the missing prefix remains silent.
    pub fn snapshot_behind(&self, dest: &mut [f32], lag_samples: usize) {
        dest.fill(0.0);
        if dest.is_empty() || self.size == 0 {
            return;
        }

        let write_pos = self.write_pos.load(Ordering::Relaxed);
        let window_len = dest.len().min(self.size);
        let lag_samples = lag_samples
            .min(self.size.saturating_sub(window_len))
            .min(write_pos);
        let end_pos = write_pos - lag_samples;
        let available = end_pos.min(window_len);
        let dest_start = dest.len() - available;
        let source_start = end_pos - available;

        // Safety: We're the only reader, and the writer only increments write_pos
        unsafe {
            let buffer = &*self.buffer.get();

            for i in 0..available {
                let idx = (source_start + i) % self.size;
                dest[dest_start + i] = buffer[idx];
            }
        }
    }

    /// Get the current write position (for debugging)
    #[allow(dead_code)]
    pub fn write_position(&self) -> usize {
        self.write_pos.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_basic() {
        let buffer = RingBuffer::new(8);

        // Push some samples
        for i in 0..5 {
            buffer.push(i as f32);
        }

        // Take a snapshot
        let mut snapshot = [0.0f32; 4];
        buffer.snapshot(&mut snapshot);

        // Should get the last 4 samples: 1, 2, 3, 4
        assert_eq!(snapshot, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_ring_buffer_wrap_around() {
        let buffer = RingBuffer::new(4);

        // Push more samples than buffer size
        for i in 0..10 {
            buffer.push(i as f32);
        }

        // Take a snapshot
        let mut snapshot = [0.0f32; 4];
        buffer.snapshot(&mut snapshot);

        // Should get the last 4 samples: 6, 7, 8, 9
        assert_eq!(snapshot, [6.0, 7.0, 8.0, 9.0]);
    }

    #[test]
    fn snapshot_behind_reads_a_historical_window() {
        let buffer = RingBuffer::new(16);
        for i in 0..10 {
            buffer.push(i as f32);
        }

        let mut snapshot = [0.0f32; 4];
        buffer.snapshot_behind(&mut snapshot, 3);

        assert_eq!(snapshot, [3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn snapshot_behind_zero_fills_history_not_yet_captured() {
        let buffer = RingBuffer::new(16);
        buffer.push(1.0);
        buffer.push(2.0);

        let mut snapshot = [9.0f32; 4];
        buffer.snapshot_behind(&mut snapshot, 1);

        assert_eq!(snapshot, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn snapshot_behind_never_reads_past_retained_history() {
        let buffer = RingBuffer::new(8);
        for i in 0..12 {
            buffer.push(i as f32);
        }

        let mut snapshot = [0.0f32; 4];
        buffer.snapshot_behind(&mut snapshot, usize::MAX);

        assert_eq!(snapshot, [4.0, 5.0, 6.0, 7.0]);
    }
}
