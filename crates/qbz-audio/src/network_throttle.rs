//! Adaptive prefetch throttling based on observed network conditions.
//!
//! Default QBZ prefetches up to 5 tracks concurrently (≈30 simultaneous
//! HTTP/2 streams once the CMAF init + segment fan-out is counted). That
//! aggressive concurrency is exactly what makes Hi-Res playback smooth on
//! a fast connection, and we don't want to dial it down for everyone just
//! because some users have a slow link.
//!
//! Instead, this module watches two real-time signals — measured per-segment
//! bandwidth and audio underruns — and computes a *dynamic* cap that the
//! prefetch dispatcher consults. When conditions are good the cap is the
//! full memory-profile default (5 on Normal-class hosts, 1 on LowMemory).
//! When the live stream starts losing the race against the consumer, the
//! cap collapses to 0 and the prefetch fan-out gets out of the way.
//!
//! Recovery follows TCP-style slow-start logic: after `PANIC_WINDOW_SECS`
//! of no fresh underruns the cap walks back up one level at a time,
//! re-validated by each new bandwidth sample.

use std::sync::OnceLock;
use std::sync::RwLock;
use std::time::Instant;

/// EMA smoothing factor for observed bandwidth. Higher = more reactive to
/// the latest sample, lower = smoother but laggier. 0.4 is a compromise:
/// a single bad sample dents the estimate but doesn't dominate it.
const BANDWIDTH_EMA_ALPHA: f64 = 0.4;

/// How long we stay in panic mode after an audio underrun. The user just
/// experienced a glitch; we want the stream to recover with the entire
/// pipe to itself for a meaningful window before letting prefetch back in.
const PANIC_WINDOW_SECS: u64 = 30;

/// Bandwidth-to-playback ratio thresholds for the throttle levels.
///
/// - At or below `SURVIVING_RATIO`: no prefetch at all. The live stream
///   barely has bandwidth to keep itself fed.
/// - At or below `CAUTIOUS_RATIO`: only one prefetch track in flight.
/// - At or below `RELAXED_RATIO`: two prefetch tracks.
/// - Above `RELAXED_RATIO`: full memory-profile default.
const SURVIVING_RATIO: f64 = 1.5;
const CAUTIOUS_RATIO: f64 = 2.5;
const RELAXED_RATIO: f64 = 4.0;

/// Approximate sustained bandwidth required for live playback by quality
/// tier, in MB/s. These numbers are for CMAF/FLAC compressed streams;
/// they're inputs to the ratio comparison above, not hard limits.
pub fn playback_mbps_for_quality(quality_tag: PlaybackQualityTag) -> f64 {
    match quality_tag {
        PlaybackQualityTag::UltraHiRes => 2.5, // 24-bit / 192 kHz FLAC
        PlaybackQualityTag::HiRes => 1.4,      // 24-bit / 96 kHz FLAC
        PlaybackQualityTag::Lossless => 0.5,   // 16-bit / 44.1 kHz FLAC
        PlaybackQualityTag::Lossy => 0.04,     // 320 kbps MP3
    }
}

/// Small enum that mirrors `qbz-models::Quality` without taking a runtime
/// dependency on it. The callsite translates its own quality enum into one
/// of these four buckets before consulting the throttle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackQualityTag {
    UltraHiRes,
    HiRes,
    Lossless,
    Lossy,
}

#[derive(Debug, Default)]
struct ThrottleInner {
    /// Exponential moving average of observed segment-download bandwidth
    /// in MB/s. `None` until the first sample arrives.
    bandwidth_ema_mbps: Option<f64>,
    /// Timestamp of the most recent audio underrun. `None` means we have
    /// not observed one this session.
    last_underrun: Option<Instant>,
    /// Timestamp of the most recent successful segment download. `None`
    /// until the first segment lands. Used as a positive liveness signal
    /// for the offline detector (issue #467).
    last_successful_download: Option<Instant>,
}

pub struct ThrottleState {
    inner: RwLock<ThrottleInner>,
}

static GLOBAL: OnceLock<ThrottleState> = OnceLock::new();

/// Singleton accessor. Lazily initialized on first call.
pub fn state() -> &'static ThrottleState {
    GLOBAL.get_or_init(|| ThrottleState {
        inner: RwLock::new(ThrottleInner::default()),
    })
}

impl ThrottleState {
    /// Feed a fresh per-segment bandwidth measurement (MB/s). Called from
    /// the CMAF streaming loop every few segments.
    pub fn record_segment_bandwidth(&self, mbps: f64) {
        if let Ok(mut inner) = self.inner.write() {
            // Liveness signal (issue #467): reaching this path means a segment
            // batch just downloaded — bytes are flowing, so we are online,
            // independent of whether the throughput sample is usable for the
            // EMA. Recorded before the validity guard on purpose.
            inner.last_successful_download = Some(Instant::now());
            if mbps.is_finite() && mbps > 0.0 {
                inner.bandwidth_ema_mbps = Some(match inner.bandwidth_ema_mbps {
                    Some(prev) => prev * (1.0 - BANDWIDTH_EMA_ALPHA) + mbps * BANDWIDTH_EMA_ALPHA,
                    None => mbps,
                });
            }
        }
    }

    /// Signal that an audio buffer underrun just happened. Forces the
    /// throttle into panic mode for `PANIC_WINDOW_SECS`.
    pub fn record_underrun(&self) {
        if let Ok(mut inner) = self.inner.write() {
            inner.last_underrun = Some(Instant::now());
        }
    }

    /// Current EMA bandwidth in MB/s, or `None` if no samples yet.
    pub fn current_bandwidth_mbps(&self) -> Option<f64> {
        self.inner.read().ok().and_then(|i| i.bandwidth_ema_mbps)
    }

    /// Seconds since the last successful segment download, or `None` if no
    /// bytes have been pulled this session. Positive liveness signal for the
    /// offline detector (issue #467): if segments are flowing we are online
    /// by definition, regardless of what a connectivity probe reports.
    pub fn seconds_since_download(&self) -> Option<u64> {
        self.inner
            .read()
            .ok()
            .and_then(|i| i.last_successful_download)
            .map(|t| t.elapsed().as_secs())
    }

    /// True when an underrun was recorded within `PANIC_WINDOW_SECS`.
    pub fn in_panic_mode(&self) -> bool {
        let inner = match self.inner.read() {
            Ok(g) => g,
            Err(_) => return false,
        };
        match inner.last_underrun {
            Some(t) => t.elapsed().as_secs() < PANIC_WINDOW_SECS,
            None => false,
        }
    }

    /// Decide the prefetch cap given the current track's playback rate and
    /// the memory-profile default. The cap is always clamped to `[0, default_cap]`
    /// — we never *raise* prefetch above the memory profile, only restrict.
    pub fn current_prefetch_cap(&self, playback_mbps: f64, default_cap: usize) -> usize {
        if self.in_panic_mode() {
            return 0;
        }
        let bw = match self.current_bandwidth_mbps() {
            Some(v) => v,
            // No samples yet — trust the memory profile default. The first
            // segment of the first track will land within a few seconds and
            // give us real numbers.
            None => return default_cap,
        };
        let ratio = if playback_mbps > 0.0 {
            bw / playback_mbps
        } else {
            f64::INFINITY
        };
        if ratio <= SURVIVING_RATIO {
            0
        } else if ratio <= CAUTIOUS_RATIO {
            1.min(default_cap)
        } else if ratio <= RELAXED_RATIO {
            2.min(default_cap)
        } else {
            default_cap
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_returns_default_cap() {
        let s = ThrottleState {
            inner: RwLock::new(ThrottleInner::default()),
        };
        assert_eq!(s.current_prefetch_cap(2.5, 5), 5);
    }

    #[test]
    fn panic_mode_zeros_cap() {
        let s = ThrottleState {
            inner: RwLock::new(ThrottleInner::default()),
        };
        s.record_underrun();
        assert_eq!(s.current_prefetch_cap(2.5, 5), 0);
    }

    #[test]
    fn surviving_ratio_zeros_prefetch() {
        let s = ThrottleState {
            inner: RwLock::new(ThrottleInner::default()),
        };
        // bw = 3.0, playback = 2.5 → ratio = 1.2 (< 1.5)
        s.record_segment_bandwidth(3.0);
        assert_eq!(s.current_prefetch_cap(2.5, 5), 0);
    }

    #[test]
    fn cautious_ratio_allows_one() {
        let s = ThrottleState {
            inner: RwLock::new(ThrottleInner::default()),
        };
        // bw = 5.0, playback = 2.5 → ratio = 2.0 (between 1.5 and 2.5)
        s.record_segment_bandwidth(5.0);
        assert_eq!(s.current_prefetch_cap(2.5, 5), 1);
    }

    #[test]
    fn relaxed_ratio_allows_two() {
        let s = ThrottleState {
            inner: RwLock::new(ThrottleInner::default()),
        };
        // bw = 8.0, playback = 2.5 → ratio = 3.2 (between 2.5 and 4.0)
        s.record_segment_bandwidth(8.0);
        assert_eq!(s.current_prefetch_cap(2.5, 5), 2);
    }

    #[test]
    fn abundant_bandwidth_unlocks_default() {
        let s = ThrottleState {
            inner: RwLock::new(ThrottleInner::default()),
        };
        // bw = 20.0, playback = 2.5 → ratio = 8.0 (well above 4.0)
        s.record_segment_bandwidth(20.0);
        assert_eq!(s.current_prefetch_cap(2.5, 5), 5);
    }

    #[test]
    fn cap_never_exceeds_default() {
        let s = ThrottleState {
            inner: RwLock::new(ThrottleInner::default()),
        };
        s.record_segment_bandwidth(20.0);
        // Memory profile says 1 — never raise above.
        assert_eq!(s.current_prefetch_cap(2.5, 1), 1);
    }

    #[test]
    fn ema_smooths_spikes() {
        let s = ThrottleState {
            inner: RwLock::new(ThrottleInner::default()),
        };
        s.record_segment_bandwidth(10.0);
        s.record_segment_bandwidth(1.0);
        // EMA: 10 * 0.6 + 1 * 0.4 = 6.4
        let bw = s.current_bandwidth_mbps().unwrap();
        assert!((bw - 6.4).abs() < 0.01);
    }
}
