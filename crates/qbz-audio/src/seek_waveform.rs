//! Progressive, track-length waveform analysis for seek bars.
//!
//! The analyzer consumes absolute-frame chunks from `AnalyzerTap`, so dropped
//! batches and seeks leave holes instead of shifting every later bin. The UI
//! snapshot is process-global because there is one audible player; consumers
//! only clone 512 bytes when its revision changes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};

pub const SEEK_WAVEFORM_BINS: usize = 512;
const SAMPLES_PER_BIN_LIMIT: u64 = 4096;
const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);

static ENABLED: AtomicBool = AtomicBool::new(false);
static REVISION: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT: OnceLock<Mutex<SeekWaveformSnapshot>> = OnceLock::new();
static TRACK_KEYS: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeekWaveformSnapshot {
    pub revision: u64,
    pub track_id: u64,
    pub bins: [u8; SEEK_WAVEFORM_BINS],
    pub analyzed_bins: u16,
    pub complete: bool,
}

impl Default for SeekWaveformSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            track_id: 0,
            bins: [0; SEEK_WAVEFORM_BINS],
            analyzed_bins: 0,
            complete: false,
        }
    }
}

pub fn set_seek_waveform_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn seek_waveform_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Register a source-stable cache key for the next source carrying `track_id`.
/// Local files use a content fingerprint; catalog/server sources may keep the
/// namespaced id supplied by their frontend.
pub fn register_seek_waveform_key(track_id: u64, key: String) {
    if key.is_empty() {
        return;
    }
    TRACK_KEYS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .insert(track_id, key);
}

/// Stable, path-independent fingerprint for local/fully-buffered media.
/// Two independent FNV-1a lanes keep the cache key compact while avoiding an
/// extra cryptography dependency in the protected playback stack.
pub fn seek_waveform_content_key(bytes: &[u8]) -> String {
    let mut a = 0xcbf29ce484222325u64;
    let mut b = 0x84222325cbf29ce4u64;
    for &byte in bytes {
        a ^= byte as u64;
        a = a.wrapping_mul(0x100000001b3);
        b ^= (byte as u64).rotate_left(1);
        b = b.wrapping_mul(0x100000001b3).rotate_left(5);
    }
    format!("content:{a:016x}{b:016x}:{}", bytes.len())
}

fn cache_key(track_id: u64) -> String {
    TRACK_KEYS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .remove(&track_id)
        .unwrap_or_else(|| format!("track:{track_id}"))
}

pub fn seek_waveform_snapshot() -> SeekWaveformSnapshot {
    SNAPSHOT
        .get_or_init(|| Mutex::new(SeekWaveformSnapshot::default()))
        .lock()
        .unwrap()
        .clone()
}

fn publish(mut snapshot: SeekWaveformSnapshot) {
    snapshot.revision = REVISION.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    *SNAPSHOT
        .get_or_init(|| Mutex::new(SeekWaveformSnapshot::default()))
        .lock()
        .unwrap() = snapshot;
}

pub(crate) struct SeekWaveformCache {
    conn: Connection,
}

impl SeekWaveformCache {
    pub(crate) fn open() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or_else(|| "Could not determine data directory".to_string())?
            .join("qbz");
        std::fs::create_dir_all(&data_dir)
            .map_err(|error| format!("Failed to create data directory: {error}"))?;
        let path = data_dir.join("seek_waveform_cache.db");
        let conn = Connection::open(&path)
            .map_err(|error| format!("Failed to open seek waveform cache: {error}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS seek_waveforms (
                 cache_key TEXT PRIMARY KEY,
                 bins BLOB NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
             );",
        )
        .map_err(|error| format!("Failed to initialize seek waveform cache: {error}"))?;
        log::info!("[SeekWaveform] Cache opened at {}", path.display());
        Ok(Self { conn })
    }

    fn get(&self, key: &str) -> Option<[u8; SEEK_WAVEFORM_BINS]> {
        let bytes: Vec<u8> = self
            .conn
            .query_row(
                "SELECT bins FROM seek_waveforms WHERE cache_key=?1",
                params![key],
                |row| row.get(0),
            )
            .ok()?;
        bytes.try_into().ok()
    }

    fn set(&self, key: &str, bins: &[u8; SEEK_WAVEFORM_BINS]) {
        if let Err(error) = self.conn.execute(
            "INSERT INTO seek_waveforms(cache_key,bins,created_at)
             VALUES(?1,?2,strftime('%s','now'))
             ON CONFLICT(cache_key) DO UPDATE SET
                 bins=excluded.bins, created_at=excluded.created_at",
            params![key, bins.as_slice()],
        ) {
            log::warn!("[SeekWaveform] Cache write failed for {key}: {error}");
        }
    }
}

pub(crate) struct SeekWaveformAccumulator {
    track_id: u64,
    key: String,
    channels: u16,
    total_frames: u64,
    sum_squares: [f64; SEEK_WAVEFORM_BINS],
    counts: [u32; SEEK_WAVEFORM_BINS],
    analyzed_bins: u16,
    complete: bool,
    cached: bool,
    last_publish: Instant,
}

impl SeekWaveformAccumulator {
    pub(crate) fn begin(
        track_id: u64,
        sample_rate: u32,
        channels: u16,
        duration_secs: u64,
        cache: Option<&SeekWaveformCache>,
    ) -> Self {
        let key = cache_key(track_id);
        if let Some(bins) = cache.and_then(|cache| cache.get(&key)) {
            publish(SeekWaveformSnapshot {
                track_id,
                bins,
                analyzed_bins: SEEK_WAVEFORM_BINS as u16,
                complete: true,
                ..SeekWaveformSnapshot::default()
            });
            return Self {
                track_id,
                key,
                channels,
                total_frames: duration_secs.saturating_mul(sample_rate as u64),
                sum_squares: [0.0; SEEK_WAVEFORM_BINS],
                counts: [0; SEEK_WAVEFORM_BINS],
                analyzed_bins: SEEK_WAVEFORM_BINS as u16,
                complete: true,
                cached: true,
                last_publish: Instant::now(),
            };
        }

        publish(SeekWaveformSnapshot {
            track_id,
            ..SeekWaveformSnapshot::default()
        });
        Self {
            track_id,
            key,
            channels: channels.max(1),
            total_frames: duration_secs.saturating_mul(sample_rate as u64),
            sum_squares: [0.0; SEEK_WAVEFORM_BINS],
            counts: [0; SEEK_WAVEFORM_BINS],
            analyzed_bins: 0,
            complete: false,
            cached: false,
            last_publish: Instant::now(),
        }
    }

    pub(crate) fn feed(
        &mut self,
        start_frame: u64,
        samples: &[f32],
        cache: Option<&SeekWaveformCache>,
    ) {
        if self.complete || self.cached || self.total_frames == 0 || samples.is_empty() {
            return;
        }
        let channels = self.channels as usize;
        let frames = samples.len() / channels;
        if frames == 0 {
            return;
        }
        let frames_per_bin = self.total_frames.div_ceil(SEEK_WAVEFORM_BINS as u64);
        let stride = frames_per_bin.div_ceil(SAMPLES_PER_BIN_LIMIT).max(1);
        let mut furthest = start_frame;

        for frame_offset in 0..frames {
            let absolute = start_frame.saturating_add(frame_offset as u64);
            if absolute >= self.total_frames || absolute % stride != 0 {
                continue;
            }
            let bin = ((absolute as u128 * SEEK_WAVEFORM_BINS as u128) / self.total_frames as u128)
                as usize;
            let base = frame_offset * channels;
            let frame_square = samples[base..base + channels]
                .iter()
                .copied()
                .map(|sample| (sample as f64) * (sample as f64))
                .sum::<f64>()
                / channels as f64;
            self.sum_squares[bin] += frame_square;
            if self.counts[bin] == 0 {
                self.analyzed_bins = self.analyzed_bins.saturating_add(1);
            }
            self.counts[bin] = self.counts[bin].saturating_add(1);
            furthest = absolute;
        }

        if self.analyzed_bins as usize == SEEK_WAVEFORM_BINS {
            self.finish(cache);
        } else if furthest.saturating_add(frames_per_bin.max(frames as u64)) >= self.total_frames {
            // A resumed track, seek, or dropped channel batch can reach EOF
            // with holes. Publish those known bins, but never mark or cache a
            // partial document as the full waveform.
            self.publish_progressive();
        } else if self.last_publish.elapsed() >= PUBLISH_INTERVAL {
            self.publish_progressive();
            self.last_publish = Instant::now();
        }
    }

    fn rms_bins(&self, normalize_complete: bool) -> [f32; SEEK_WAVEFORM_BINS] {
        let mut rms = [0.0f32; SEEK_WAVEFORM_BINS];
        let mut peak = 0.0f32;
        for (index, out) in rms.iter_mut().enumerate() {
            if self.counts[index] > 0 {
                *out = (self.sum_squares[index] / self.counts[index] as f64).sqrt() as f32;
                peak = peak.max(*out);
            }
        }
        let reference = if normalize_complete && peak > f32::EPSILON {
            peak
        } else {
            // A stable progressive reference prevents the already-painted
            // shape from breathing when a later, louder passage arrives.
            0.25
        };
        for value in &mut rms {
            *value = (*value / reference).clamp(0.0, 1.0);
        }
        rms
    }

    fn quantized(&self, normalize_complete: bool) -> [u8; SEEK_WAVEFORM_BINS] {
        self.rms_bins(normalize_complete)
            .map(|value| (value * 255.0).round() as u8)
    }

    fn publish_progressive(&self) {
        publish(SeekWaveformSnapshot {
            track_id: self.track_id,
            bins: self.quantized(false),
            analyzed_bins: self.analyzed_bins,
            complete: false,
            ..SeekWaveformSnapshot::default()
        });
    }

    fn finish(&mut self, cache: Option<&SeekWaveformCache>) {
        if self.complete {
            return;
        }
        self.complete = true;
        let bins = self.quantized(true);
        if let Some(cache) = cache {
            cache.set(&self.key, &bins);
        }
        publish(SeekWaveformSnapshot {
            track_id: self.track_id,
            bins,
            analyzed_bins: self.analyzed_bins,
            complete: true,
            ..SeekWaveformSnapshot::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_frames_leave_dropped_chunk_holes_instead_of_shifting_bins() {
        let mut acc = SeekWaveformAccumulator::begin(7, 512, 1, 1, None);
        acc.feed(0, &[0.5; 128], None);
        acc.feed(384, &[0.5; 128], None);
        assert!(acc.counts[..128].iter().all(|count| *count > 0));
        assert!(acc.counts[128..384].iter().all(|count| *count == 0));
        assert!(acc.counts[384..].iter().all(|count| *count > 0));
    }

    #[test]
    fn stereo_channels_contribute_to_frame_rms() {
        let mut acc = SeekWaveformAccumulator::begin(8, 512, 2, 1, None);
        let mut samples = Vec::with_capacity(1024);
        for _ in 0..512 {
            samples.extend_from_slice(&[0.5, 0.5]);
        }
        acc.feed(0, &samples, None);
        assert!(acc
            .sum_squares
            .iter()
            .all(|value| (*value - 0.25).abs() < 1e-9));
        assert!(acc.complete);
    }

    #[test]
    fn stereo_energy_does_not_cancel_when_channels_are_out_of_phase() {
        let mut acc = SeekWaveformAccumulator::begin(10, 512, 2, 1, None);
        let mut samples = Vec::with_capacity(1024);
        for _ in 0..512 {
            samples.extend_from_slice(&[0.5, -0.5]);
        }
        acc.feed(0, &samples, None);
        assert!(acc
            .sum_squares
            .iter()
            .all(|value| (*value - 0.25).abs() < 1e-9));
    }

    #[test]
    fn resumed_tail_is_not_cached_as_a_complete_waveform() {
        let mut acc = SeekWaveformAccumulator::begin(11, 512, 1, 1, None);
        acc.feed(256, &[0.5; 256], None);
        assert!(!acc.complete);
        assert_eq!(acc.analyzed_bins, 256);
    }

    #[test]
    fn progressive_reference_does_not_rescale_earlier_bins() {
        let mut acc = SeekWaveformAccumulator::begin(9, 512, 1, 1, None);
        acc.feed(0, &[0.125; 128], None);
        let before = acc.quantized(false)[0];
        acc.feed(128, &[0.9; 128], None);
        assert_eq!(acc.quantized(false)[0], before);
    }

    #[test]
    fn content_key_is_path_independent_and_content_sensitive() {
        assert_eq!(
            seek_waveform_content_key(b"same audio bytes"),
            seek_waveform_content_key(b"same audio bytes")
        );
        assert_ne!(
            seek_waveform_content_key(b"same audio bytes"),
            seek_waveform_content_key(b"different audio bytes")
        );
    }
}
