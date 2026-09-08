//! Visualizer while casting: a SHADOW decoder.
//!
//! The FFT/scope surfaces are fed by the local engine's writer thread
//! (`VisualizerTap.ring_buffer`). While a renderer plays, that engine is
//! stopped and the ring stays empty — every FFT-driven animation froze the
//! moment a cast started (owner report 2026-08-30). QBZ still holds the very
//! bytes the renderer is playing (the progressive download buffer, the cached
//! track, the local file), so this module decodes them a second time, SILENTLY,
//! and pushes the samples into the same tap at real-time pace, re-anchored to
//! the position the renderer reports on every poll. Nothing downstream (the
//! producer, the drain, QML) knows the difference.
//!
//! Accuracy is bounded by the renderer's position report (Chromecast media
//! status; DLNA `RelTime` at 1 s resolution) plus its own buffer latency:
//! expect the bars to follow the music within a few hundred ms, not
//! beat-exact. Re-anchoring only fires past `DRIFT_RESEEK_SECS` so poll
//! jitter does not make the picture stutter.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use qbz_audio::VisualizerTap;
use qbz_player::{BufferedMediaSource, InMemorySource, IncrementalStreamingSource};

/// Where the shadow decoder reads the renderer's bytes from.
pub(crate) enum ShadowSource {
    /// A Qobuz track still downloading (or just downloaded) for the renderer.
    Buffered(Arc<BufferedMediaSource>),
    /// A cached track, shared with the media server (no copy).
    Bytes(Arc<Vec<u8>>),
    /// A local file, read once on the decoder thread.
    File(PathBuf),
}

/// Drift between the shadow position and the renderer's report that
/// triggers a re-seek. Below it the pace clock is trusted.
const DRIFT_RESEEK_SECS: f64 = 0.75;
/// Frames pushed per pace step (small enough that a re-anchor is snappy).
const STEP_FRAMES: u64 = 1024;

enum Decoder {
    Buffered(IncrementalStreamingSource),
    Memory(InMemorySource),
}

impl Decoder {
    fn open(source: &ShadowSource) -> Result<(Self, u32, u16), String> {
        match source {
            ShadowSource::Buffered(buffer) => {
                let dec = IncrementalStreamingSource::new(Arc::clone(buffer))?;
                let (sr, ch) = (dec.get_sample_rate(), dec.get_channels());
                Ok((Decoder::Buffered(dec), sr, ch))
            }
            ShadowSource::Bytes(bytes) => {
                let dec = InMemorySource::from_shared(Arc::clone(bytes))?;
                let (sr, ch) = (dec.sample_rate(), dec.channels());
                Ok((Decoder::Memory(dec), sr, ch))
            }
            ShadowSource::File(path) => {
                let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
                let dec = InMemorySource::new(bytes)?;
                let (sr, ch) = (dec.sample_rate(), dec.channels());
                Ok((Decoder::Memory(dec), sr, ch))
            }
        }
    }

    fn seek_to(&mut self, at: Duration) -> Result<(), String> {
        match self {
            Decoder::Buffered(d) => d.seek_to(at),
            Decoder::Memory(d) => d.seek_to(at),
        }
    }

    fn next(&mut self) -> Option<f32> {
        match self {
            Decoder::Buffered(d) => d.next(),
            Decoder::Memory(d) => d.next(),
        }
    }
}

struct Session {
    stop: AtomicBool,
    playing: AtomicBool,
    /// Renderer position to re-anchor to (seconds), consumed by the thread.
    seek: Mutex<Option<f64>>,
    /// Shadow position in frames (what the thread has pushed so far).
    pos_frames: AtomicU64,
    sample_rate: AtomicU64,
}

static CURRENT: OnceLock<Mutex<Option<Arc<Session>>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Arc<Session>>> {
    CURRENT.get_or_init(|| Mutex::new(None))
}

/// Start shadowing `source` into `tap`. Replaces any running shadow.
pub(crate) fn start(source: ShadowSource, tap: VisualizerTap) {
    stop();
    let session = Arc::new(Session {
        stop: AtomicBool::new(false),
        playing: AtomicBool::new(true),
        seek: Mutex::new(None),
        pos_frames: AtomicU64::new(0),
        sample_rate: AtomicU64::new(0),
    });
    if let Ok(mut s) = slot().lock() {
        *s = Some(Arc::clone(&session));
    }
    let spawned = std::thread::Builder::new()
        .name("qbz-cast-viz".into())
        .spawn(move || run(session, source, tap));
    if let Err(e) = spawned {
        log::warn!("[qbz-qt][cast-viz] could not start the shadow decoder: {e}");
    }
}

/// Feed the renderer's latest report: play state and position (seconds).
pub(crate) fn anchor(position_secs: f64, playing: bool) {
    let Some(session) = slot().lock().ok().and_then(|s| s.clone()) else {
        return;
    };
    session.playing.store(playing, Ordering::Relaxed);
    let sr = session.sample_rate.load(Ordering::Relaxed);
    if sr == 0 {
        return;
    }
    let shadow_secs = session.pos_frames.load(Ordering::Relaxed) as f64 / sr as f64;
    if (shadow_secs - position_secs).abs() > DRIFT_RESEEK_SECS {
        if let Ok(mut seek) = session.seek.lock() {
            *seek = Some(position_secs.max(0.0));
        }
    }
}

/// Stop the shadow decoder (track change, disconnect, shutdown).
pub(crate) fn stop() {
    if let Some(session) = slot().lock().ok().and_then(|mut s| s.take()) {
        session.stop.store(true, Ordering::Relaxed);
    }
}

fn run(session: Arc<Session>, source: ShadowSource, tap: VisualizerTap) {
    let (mut decoder, sample_rate, channels) = match Decoder::open(&source) {
        Ok(opened) => opened,
        Err(e) => {
            log::warn!("[qbz-qt][cast-viz] shadow decoder unavailable: {e}");
            return;
        }
    };
    session
        .sample_rate
        .store(sample_rate as u64, Ordering::Relaxed);
    tap.set_sample_rate(sample_rate);
    // No output device between the tap and the ear: no delay to compensate.
    tap.clear_output_delay();
    log::info!("[qbz-qt][cast-viz] shadow decoder running ({sample_rate} Hz, {channels} ch)");

    let channels = channels.max(1) as u64;
    let mut pos_frames: u64 = 0;
    // (instant, frames) the pace clock was last (re)started from.
    let mut clock: Option<(Instant, u64)> = None;
    let mut ended = false;

    while !session.stop.load(Ordering::Relaxed) {
        if let Some(target) = session.seek.lock().ok().and_then(|mut s| s.take()) {
            match decoder.seek_to(Duration::from_secs_f64(target)) {
                Ok(()) => {
                    pos_frames = (target * sample_rate as f64) as u64;
                    session.pos_frames.store(pos_frames, Ordering::Relaxed);
                    ended = false;
                }
                Err(e) => log::debug!("[qbz-qt][cast-viz] re-anchor to {target:.1}s failed: {e}"),
            }
            clock = None;
        }
        if ended || !session.playing.load(Ordering::Relaxed) {
            clock = None;
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let (t0, f0) = *clock.get_or_insert_with(|| (Instant::now(), pos_frames));
        let due = f0 + (t0.elapsed().as_secs_f64() * sample_rate as f64) as u64;
        if pos_frames >= due {
            std::thread::sleep(Duration::from_millis(8));
            continue;
        }
        let frames = (due - pos_frames).min(STEP_FRAMES);
        let mut pushed = 0u64;
        'chunk: for _ in 0..frames {
            for _ in 0..channels {
                match decoder.next() {
                    Some(sample) => tap.push(sample),
                    None => {
                        ended = true;
                        break 'chunk;
                    }
                }
            }
            pushed += 1;
        }
        pos_frames += pushed;
        session.pos_frames.store(pos_frames, Ordering::Relaxed);
    }
    log::debug!("[qbz-qt][cast-viz] shadow decoder stopped");
}
