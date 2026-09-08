//! Buffered media source for streaming playback.
//!
//! Provides two main components:
//! 1. `BufferedMediaSource` - Wraps an async HTTP response to provide a synchronous
//!    `Read + Seek` interface required by symphonia decoders.
//! 2. `IncrementalStreamingSource` - A rodio Source that decodes audio packets
//!    incrementally as they become available, allowing playback to start before
//!    the entire file is downloaded.
//!
//! # Design
//!
//! The source uses a growing buffer that accumulates data from the HTTP response.
//! - Reads block if requesting data not yet buffered
//! - Seek forward blocks until data is available
//! - Seek backward works within buffered data
//! - Seek beyond current buffer position blocks until data arrives
//!
//! # Thread Safety
//!
//! The buffer state is shared between:
//! - The reader (audio thread, synchronous)
//! - The writer (download task, async)
//!
//! Communication uses `Mutex` + `Condvar` for blocking synchronization.

use std::collections::VecDeque;
use std::io::{Cursor, Error as IoError, ErrorKind, Read, Result as IoResult, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rodio::Source;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::default::{get_codecs, get_probe};

use super::{PlaybackBufferReporter, PlaybackBufferState};

/// Configuration for the streaming buffer
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Minimum bytes to buffer before allowing reads (for format detection)
    pub initial_buffer_bytes: usize,
    /// Maximum buffer size before backpressure (not enforced, just for info)
    pub max_buffer_bytes: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            // 512KB default - enough for format headers and ~2-5 seconds of audio
            // This allows playback to start quickly while still having enough
            // buffer to handle network jitter
            initial_buffer_bytes: 512 * 1024,
            // 100MB max buffer
            max_buffer_bytes: 100 * 1024 * 1024,
        }
    }
}

impl StreamingConfig {
    /// Create config from buffer seconds and approximate bitrate
    ///
    /// For Hi-Res FLAC at 192kHz/24bit stereo, bitrate is roughly 9.2 Mbps
    /// We estimate ~1MB per second as a conservative approximation
    pub fn from_seconds(seconds: u8) -> Self {
        // Minimum 256KB to ensure format detection works
        let bytes = ((seconds as usize) * 1024 * 1024).max(256 * 1024);
        Self {
            initial_buffer_bytes: bytes,
            max_buffer_bytes: 100 * 1024 * 1024,
        }
    }

    /// Create a minimal config for fastest startup
    /// Uses smallest buffer that still allows format detection (~256KB)
    pub fn fast_start() -> Self {
        Self {
            initial_buffer_bytes: 256 * 1024,
            max_buffer_bytes: 100 * 1024 * 1024,
        }
    }

    /// Create config dynamically based on measured download speed
    ///
    /// - Very fast (>10 MB/s): 256KB (instant start)
    /// - Fast (5-10 MB/s): 384KB
    /// - Normal (2-5 MB/s): 512KB
    /// - Slow (1-2 MB/s): 1MB (more buffer to prevent stutter)
    /// - Very slow (<1 MB/s): 2MB
    ///
    /// Result is clamped to the process-wide cap configured via
    /// [`set_max_initial_buffer_bytes`] (typically derived from the host's
    /// memory profile — see qbz-core's system_capabilities). On
    /// memory-constrained hosts the slow-connection branches would
    /// otherwise inflate to 2 MB, which is exactly the wrong direction
    /// when "slow connection" is itself a symptom of swap thrash
    /// (issue #331, Pi 3B).
    pub fn from_speed_mbps(speed_mbps: f64) -> Self {
        let cap = MAX_INITIAL_BUFFER_BYTES.load(std::sync::atomic::Ordering::Relaxed);
        let cfg = Self::from_speed_mbps_with_cap(speed_mbps, cap);

        if cfg.initial_buffer_bytes < raw_initial_buffer_for_speed(speed_mbps) {
            log::info!(
                "Dynamic buffer: {:.1} MB/s detected → {}KB (capped from {}KB by host memory profile)",
                speed_mbps,
                cfg.initial_buffer_bytes / 1024,
                raw_initial_buffer_for_speed(speed_mbps) / 1024
            );
        } else {
            log::info!(
                "Dynamic buffer: {:.1} MB/s detected → {}KB initial buffer",
                speed_mbps,
                cfg.initial_buffer_bytes / 1024
            );
        }

        cfg
    }

    /// Pure variant of [`from_speed_mbps`] — derives the speed-based
    /// initial buffer and clamps to `cap` without touching global state
    /// or logging. Exposed for unit tests; production callers should use
    /// `from_speed_mbps`, which reads the process-wide cap.
    pub fn from_speed_mbps_with_cap(speed_mbps: f64, cap: usize) -> Self {
        let raw_initial_buffer = raw_initial_buffer_for_speed(speed_mbps);
        Self {
            initial_buffer_bytes: raw_initial_buffer.min(cap),
            max_buffer_bytes: 100 * 1024 * 1024,
        }
    }
}

/// Speed-driven initial buffer size, before any cap is applied.
/// Pure function — used by both `from_speed_mbps` and
/// `from_speed_mbps_with_cap` so they share the same ladder.
fn raw_initial_buffer_for_speed(speed_mbps: f64) -> usize {
    if speed_mbps >= 10.0 {
        256 * 1024 // 256KB - instant start for very fast connections
    } else if speed_mbps >= 5.0 {
        384 * 1024 // 384KB
    } else if speed_mbps >= 2.0 {
        512 * 1024 // 512KB - default
    } else if speed_mbps >= 1.0 {
        1024 * 1024 // 1MB - more buffer for slower connections
    } else {
        2 * 1024 * 1024 // 2MB - maximum buffer for very slow connections
    }
}

/// Process-wide cap for dynamically-derived initial buffer sizes.
/// Defaults to `usize::MAX` (no cap) so behavior is unchanged unless the
/// host explicitly configures it via [`set_max_initial_buffer_bytes`] —
/// typically once at process start, derived from the detected memory
/// profile.
static MAX_INITIAL_BUFFER_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);

/// Set the process-wide cap for `StreamingConfig::from_speed_mbps`.
/// Subsequent calls to that constructor clamp their result to this cap.
pub fn set_max_initial_buffer_bytes(bytes: usize) {
    MAX_INITIAL_BUFFER_BYTES.store(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// Read the current cap. Mainly useful for tests.
pub fn max_initial_buffer_bytes() -> usize {
    MAX_INITIAL_BUFFER_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Absolute ceiling for the up-front buffer reservation: 1 GiB. Guards
/// against absurd Content-Length values reserving more RAM than any real
/// track needs.
const MAX_PREALLOC_BYTES: u64 = 1024 * 1024 * 1024;

/// Pure decision for the up-front reservation in [`BufferedMediaSource::new`]:
/// `Some(capacity)` when `total_size` is known and non-zero (capped at
/// [`MAX_PREALLOC_BYTES`]), `None` when the caller should fall back to the
/// config's initial-buffer capacity.
fn prealloc_capacity(total_size: Option<u64>) -> Option<usize> {
    let total = total_size?;
    if total == 0 {
        return None;
    }
    Some(total.min(MAX_PREALLOC_BYTES) as usize)
}

/// Internal state shared between reader and writer
struct BufferState {
    /// Accumulated data from HTTP response
    data: Vec<u8>,
    /// True when HTTP download is complete
    download_complete: bool,
    /// Error from download, if any
    download_error: Option<String>,
    /// Total expected size (from Content-Length), if known
    total_size: Option<u64>,
}

/// A media source that buffers from an async HTTP stream.
///
/// Provides `Read + Seek` interface for decoders while data is still downloading.
/// The source is created with a `BufferWriter` that receives chunks from the
/// download task.
pub struct BufferedMediaSource {
    state: Arc<(Mutex<BufferState>, Condvar)>,
    config: StreamingConfig,
    /// Each reader has its own read position
    read_pos: std::sync::atomic::AtomicU64,
}

impl BufferedMediaSource {
    /// Create a new buffered source.
    ///
    /// Returns the source and a writer for pushing downloaded chunks.
    /// The writer should be used from the async download task.
    pub fn new(config: StreamingConfig, total_size: Option<u64>) -> (Self, BufferWriter) {
        // When the total track size is known, reserve it up front: the
        // buffer ends up holding the whole track either way, so this skips
        // the ~8 doubling reallocs (and their memcpy) a 60-400 MB download
        // would otherwise pay. Falls back to the initial-buffer capacity
        // when the reservation fails (e.g. a 424 MB track on 32-bit, where
        // the contiguous range may not exist) or the size is unknown.
        let data = match prealloc_capacity(total_size) {
            Some(cap) => {
                let mut v = Vec::new();
                if v.try_reserve_exact(cap).is_err() {
                    log::warn!(
                        "Stream buffer: could not reserve {} bytes up front; growing dynamically",
                        cap
                    );
                    v = Vec::with_capacity(config.initial_buffer_bytes);
                }
                v
            }
            None => Vec::with_capacity(config.initial_buffer_bytes),
        };

        let state = Arc::new((
            Mutex::new(BufferState {
                data,
                download_complete: false,
                download_error: None,
                total_size,
            }),
            Condvar::new(),
        ));

        let source = Self {
            state: Arc::clone(&state),
            config: config.clone(),
            read_pos: std::sync::atomic::AtomicU64::new(0),
        };

        let writer = BufferWriter { state };

        (source, writer)
    }

    /// Create a new reader that shares the same buffer but has its own read position.
    /// This is used to pass to symphonia which needs ownership of the reader.
    pub fn create_reader(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            config: self.config.clone(),
            read_pos: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Wait until initial buffer is filled or download completes.
    ///
    /// This should be called before passing the source to the decoder,
    /// to ensure enough data is available for format detection.
    ///
    /// Returns error if download fails before initial buffer is filled.
    pub fn wait_for_initial_buffer(&self) -> IoResult<()> {
        let (lock, cvar) = &*self.state;
        let mut state = lock
            .lock()
            .map_err(|_| IoError::new(ErrorKind::Other, "Failed to acquire buffer lock"))?;

        while state.data.len() < self.config.initial_buffer_bytes
            && !state.download_complete
            && state.download_error.is_none()
        {
            state = cvar
                .wait(state)
                .map_err(|_| IoError::new(ErrorKind::Other, "Condition variable wait failed"))?;
        }

        if let Some(ref err) = state.download_error {
            return Err(IoError::new(ErrorKind::Other, err.clone()));
        }

        Ok(())
    }

    /// Check if download is complete (full file in buffer)
    pub fn is_complete(&self) -> bool {
        let (lock, _) = &*self.state;
        if let Ok(state) = lock.lock() {
            state.download_complete && state.download_error.is_none()
        } else {
            false
        }
    }

    /// Get current buffer size in bytes
    pub fn buffer_size(&self) -> usize {
        let (lock, _) = &*self.state;
        if let Ok(state) = lock.lock() {
            state.data.len()
        } else {
            0
        }
    }

    /// Get the complete data if download finished successfully.
    ///
    /// Used to store in cache after streaming playback completes.
    /// Returns None if download is not complete or failed.
    ///
    /// IMPORTANT: This clones the buffer rather than moving it. Earlier
    /// attempts to use `mem::take` here regressed playback — the
    /// `Source` impl is still actively reading from `state.data` when
    /// the promotion path calls this, and zeroing the buffer out from
    /// under the reader caused immediate EOF (tracks ending 10s into a
    /// 104s file). Cloning is the safe choice; the audible hiccup at
    /// promotion that the move attempted to fix needs a different
    /// approach (shared `Arc<Vec<u8>>` ownership, or off-thread copy).
    pub fn take_complete_data(&self) -> Option<Vec<u8>> {
        let (lock, _) = &*self.state;
        if let Ok(state) = lock.lock() {
            if state.download_complete && state.download_error.is_none() {
                Some(state.data.clone())
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Get a copy of the currently buffered data (for metadata extraction).
    ///
    /// Returns whatever data has been downloaded so far, even if incomplete.
    /// Useful for extracting file-level metadata (e.g., ReplayGain tags)
    /// which are typically in the first few KB of the file.
    pub fn get_buffered_data(&self) -> Option<Vec<u8>> {
        let (lock, _) = &*self.state;
        if let Ok(state) = lock.lock() {
            if state.data.is_empty() {
                None
            } else {
                Some(state.data.clone())
            }
        } else {
            None
        }
    }

    /// Get download progress as a fraction (0.0 to 1.0)
    ///
    /// Returns None if total size is unknown
    pub fn progress(&self) -> Option<f32> {
        let (lock, _) = &*self.state;
        if let Ok(state) = lock.lock() {
            state.total_size.map(|total| {
                if total == 0 {
                    1.0
                } else {
                    state.data.len() as f32 / total as f32
                }
            })
        } else {
            None
        }
    }

    /// Check if minimum buffer for playback is available
    ///
    /// Returns true when initial_buffer_bytes have been buffered
    /// or the download is complete.
    pub fn has_min_buffer(&self) -> bool {
        let (lock, _) = &*self.state;
        if let Ok(state) = lock.lock() {
            state.data.len() >= self.config.initial_buffer_bytes || state.download_complete
        } else {
            false
        }
    }

    /// Error reported by the feeder, if any. Lets waiters (the initial
    /// buffer fill loop) bail out immediately instead of sitting through
    /// the full buffer timeout when the feeder has already died.
    pub fn download_error(&self) -> Option<String> {
        let (lock, _) = &*self.state;
        lock.lock().ok().and_then(|s| s.download_error.clone())
    }
}

impl Read for BufferedMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        use std::sync::atomic::Ordering;

        let (lock, cvar) = &*self.state;
        let mut state = lock
            .lock()
            .map_err(|_| IoError::new(ErrorKind::Other, "Failed to acquire buffer lock"))?;

        let read_pos = self.read_pos.load(Ordering::SeqCst) as usize;

        // Wait for data if we're ahead of buffer
        while read_pos >= state.data.len()
            && !state.download_complete
            && state.download_error.is_none()
        {
            state = cvar
                .wait(state)
                .map_err(|_| IoError::new(ErrorKind::Other, "Condition variable wait failed"))?;
        }

        // Check for errors
        if let Some(ref err) = state.download_error {
            return Err(IoError::new(ErrorKind::Other, err.clone()));
        }

        // EOF if at end and download complete
        if read_pos >= state.data.len() && state.download_complete {
            return Ok(0);
        }

        // Read available data
        let available = state.data.len() - read_pos;
        let to_read = buf.len().min(available);
        buf[..to_read].copy_from_slice(&state.data[read_pos..read_pos + to_read]);
        self.read_pos
            .store((read_pos + to_read) as u64, Ordering::SeqCst);

        Ok(to_read)
    }
}

impl Seek for BufferedMediaSource {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        use std::sync::atomic::Ordering;

        let (lock, cvar) = &*self.state;
        let mut state = lock
            .lock()
            .map_err(|_| IoError::new(ErrorKind::Other, "Failed to acquire buffer lock"))?;

        let current_pos = self.read_pos.load(Ordering::SeqCst) as i64;

        let new_pos = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::Current(offset) => current_pos + offset,
            SeekFrom::End(offset) => {
                // For End seeks, we need to know total size or have complete download
                if let Some(total) = state.total_size {
                    total as i64 + offset
                } else if state.download_complete {
                    state.data.len() as i64 + offset
                } else {
                    // Can't seek from end without knowing size
                    return Err(IoError::new(
                        ErrorKind::Unsupported,
                        "Cannot seek from end while streaming without known size",
                    ));
                }
            }
        };

        if new_pos < 0 {
            return Err(IoError::new(
                ErrorKind::InvalidInput,
                "Seek position before start of stream",
            ));
        }

        let new_pos_usize = new_pos as usize;

        // If seeking forward beyond buffer, wait for data
        while new_pos_usize > state.data.len()
            && !state.download_complete
            && state.download_error.is_none()
        {
            state = cvar
                .wait(state)
                .map_err(|_| IoError::new(ErrorKind::Other, "Condition variable wait failed"))?;
        }

        if let Some(ref err) = state.download_error {
            return Err(IoError::new(ErrorKind::Other, err.clone()));
        }

        // After download complete, check bounds
        if state.download_complete && new_pos_usize > state.data.len() {
            return Err(IoError::new(
                ErrorKind::InvalidInput,
                "Seek position beyond end of stream",
            ));
        }

        self.read_pos.store(new_pos as u64, Ordering::SeqCst);
        Ok(new_pos as u64)
    }
}

// Required for symphonia MediaSource trait
impl MediaSource for BufferedMediaSource {
    fn is_seekable(&self) -> bool {
        // We support seeking within buffered data
        true
    }

    fn byte_len(&self) -> Option<u64> {
        let (lock, _) = &*self.state;
        if let Ok(state) = lock.lock() {
            state.total_size
        } else {
            None
        }
    }
}

/// Writer half for pushing downloaded chunks from the async download task.
///
/// This is the sender side that receives data from the HTTP response
/// and makes it available to the `BufferedMediaSource` reader.
#[derive(Clone)]
pub struct BufferWriter {
    state: Arc<(Mutex<BufferState>, Condvar)>,
}

impl BufferWriter {
    /// Push a chunk of downloaded data
    ///
    /// This wakes up any readers waiting for data.
    pub fn push_chunk(&self, chunk: &[u8]) -> Result<(), String> {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().map_err(|_| "Failed to acquire buffer lock")?;

        state.data.extend_from_slice(chunk);
        cvar.notify_all();

        Ok(())
    }

    /// Mark download as complete
    ///
    /// After this is called, readers will receive EOF after reading all buffered data.
    pub fn complete(&self) -> Result<(), String> {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().map_err(|_| "Failed to acquire buffer lock")?;

        state.download_complete = true;
        cvar.notify_all();

        Ok(())
    }

    /// Mark download as failed
    ///
    /// After this is called, readers will receive the error on next read.
    /// The first recorded error wins: it is the root cause, and the feeder
    /// fail-guards fire a generic "aborted" error on drop after a specific
    /// failure has already been recorded, which must not overwrite it.
    pub fn error(&self, err: String) -> Result<(), String> {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().map_err(|_| "Failed to acquire buffer lock")?;

        if state.download_error.is_none() {
            state.download_error = Some(err);
        }
        cvar.notify_all();

        Ok(())
    }

    /// Get current buffer size in bytes
    pub fn buffer_size(&self) -> usize {
        let (lock, _) = &*self.state;
        if let Ok(state) = lock.lock() {
            state.data.len()
        } else {
            0
        }
    }

    /// Copy the buffered bytes into `out` in bounded chunks, re-taking the
    /// state lock per chunk instead of holding it for the whole transfer,
    /// so live readers are starved for at most one chunk's write instead of
    /// a full-track copy. Used by the low-memory oversized-track path to
    /// persist the finished track to the L2 disk cache straight from the
    /// playback buffer — no second full in-RAM copy. Returns bytes written.
    pub fn write_buffered_to<W: std::io::Write>(&self, out: &mut W) -> IoResult<usize> {
        const CHUNK: usize = 1024 * 1024;
        let (lock, _) = &*self.state;
        let mut offset = 0usize;
        loop {
            let state = lock
                .lock()
                .map_err(|_| IoError::new(ErrorKind::Other, "Failed to acquire buffer lock"))?;
            if offset >= state.data.len() {
                break;
            }
            let end = (offset + CHUNK).min(state.data.len());
            out.write_all(&state.data[offset..end])?;
            offset = end;
        }
        Ok(offset)
    }
}

// =============================================================================
// IncrementalStreamingSource - A rodio Source that decodes on-demand
// =============================================================================

/// A rodio Source that decodes audio packets incrementally from a BufferedMediaSource.
///
/// This allows playback to start immediately after the initial buffer is filled,
/// while the rest of the file continues downloading in the background.
///
/// The source maintains an internal queue of decoded samples and decodes more
/// packets on-demand as samples are consumed.
pub struct IncrementalStreamingSource {
    /// Sample rate of the audio
    sample_rate: u32,
    /// Number of channels
    channels: u16,
    /// Queue of decoded samples ready to play
    sample_queue: VecDeque<f32>,
    /// The format reader (demuxer)
    format: Box<dyn FormatReader>,
    /// The audio decoder
    decoder: Box<dyn Decoder>,
    /// Track ID we're decoding
    track_id: u32,
    /// Whether we've reached end of stream
    finished: bool,
    /// Number of packets decoded (for stats)
    packets_decoded: u64,
    /// True while inside a WouldBlock stall episode (playback caught up
    /// with the download). Set on the first WouldBlock after at least one
    /// decoded packet, cleared on the next successful decode — so each
    /// episode records exactly one underrun with the network throttle.
    stalled: bool,
    /// Reference to the buffered source (for cache retrieval after playback)
    buffered_source: Arc<BufferedMediaSource>,
    /// Optional generation-safe side channel into the player's buffer state.
    buffer_reporter: Option<PlaybackBufferReporter>,
}

impl IncrementalStreamingSource {
    /// Create a new incremental streaming source.
    ///
    /// This initializes the symphonia decoder and prepares for incremental decoding.
    /// The BufferedMediaSource should already have its initial buffer filled.
    ///
    /// Returns the source along with detected sample_rate and channels.
    pub fn new(buffered_source: Arc<BufferedMediaSource>) -> Result<Self, String> {
        Self::new_inner(buffered_source, None)
    }

    pub(super) fn new_for_play(
        buffered_source: Arc<BufferedMediaSource>,
        buffer_reporter: PlaybackBufferReporter,
    ) -> Result<Self, String> {
        Self::new_inner(buffered_source, Some(buffer_reporter))
    }

    fn new_inner(
        buffered_source: Arc<BufferedMediaSource>,
        buffer_reporter: Option<PlaybackBufferReporter>,
    ) -> Result<Self, String> {
        // Create a reader from the buffered source
        let reader = buffered_source.create_reader();
        let media_source = Box::new(reader) as Box<dyn MediaSource>;
        let mss = MediaSourceStream::new(media_source, Default::default());

        let mut hint = Hint::new();
        hint.with_extension("flac"); // Most Qobuz Hi-Res is FLAC

        let format_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let metadata_opts: MetadataOptions = Default::default();

        let probed = get_probe()
            .format(&hint, mss, &format_opts, &metadata_opts)
            .map_err(|err| format!("Symphonia probe failed for streaming: {}", err))?;

        let track = probed
            .format
            .default_track()
            .ok_or_else(|| "Symphonia: no supported audio tracks in stream".to_string())?;

        let track_id = track.id;
        let codec_params = track.codec_params.clone();

        // Extract sample rate and channels from codec params
        let sample_rate = codec_params
            .sample_rate
            .ok_or_else(|| "No sample rate in codec params".to_string())?;
        let channels = codec_params.channels.map(|c| c.count() as u16).unwrap_or(2);

        let decoder = get_codecs()
            .make(&codec_params, &DecoderOptions::default())
            .map_err(|err| format!("Symphonia decoder init failed for streaming: {}", err))?;

        log::info!(
            "IncrementalStreamingSource initialized: {}Hz, {} channels",
            sample_rate,
            channels
        );

        Ok(Self {
            sample_rate,
            channels,
            sample_queue: VecDeque::with_capacity(sample_rate as usize * channels as usize), // ~1s buffer
            format: probed.format,
            decoder,
            track_id,
            finished: false,
            packets_decoded: 0,
            stalled: false,
            buffered_source,
            buffer_reporter,
        })
    }

    /// Get the sample rate
    pub fn get_sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get the number of channels
    pub fn get_channels(&self) -> u16 {
        self.channels
    }

    /// Get reference to the buffered source for cache retrieval
    pub fn buffered_source(&self) -> &Arc<BufferedMediaSource> {
        &self.buffered_source
    }

    /// Seek the decoder to the given time using Symphonia's native seek.
    ///
    /// For FLAC this uses the seek table to jump directly to the nearest
    /// seek point, then decodes forward to the exact sample — far cheaper
    /// than skip_duration's decode-every-sample-from-zero path. For MP3
    /// with Xing/VBRI headers it uses the TOC; without headers, Symphonia
    /// falls back to a binary search, still much cheaper than linear decode.
    ///
    /// The underlying BufferedMediaSource::seek is the I/O target. If the
    /// requested byte offset isn't buffered yet it will block on the
    /// condition variable — callers must only invoke this for times within
    /// the downloaded watermark.
    pub fn seek_to(&mut self, time: Duration) -> Result<(), String> {
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: time.into(),
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| format!("Symphonia seek failed: {}", e))?;
        self.decoder.reset();
        self.sample_queue.clear();
        self.packets_decoded = 0;
        self.finished = false;
        Ok(())
    }

    /// Decode more packets to fill the sample queue.
    ///
    /// This is called when the sample queue is running low.
    /// It will decode packets until the queue has at least `min_samples` or EOF is reached.
    fn decode_more(&mut self, min_samples: usize) {
        if self.finished {
            return;
        }

        while self.sample_queue.len() < min_samples {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    // Not enough data buffered yet - wait briefly and retry
                    // This happens when playback catches up with download
                    if !self.stalled && self.packets_decoded > 0 {
                        // Mid-playback stall, not initial buffering (≥1 packet
                        // already decoded): put the prefetch throttle in panic
                        // mode so the live stream gets the pipe to itself (#591).
                        self.stalled = true;
                        qbz_audio::network_throttle::state().record_underrun();
                        if let Some(reporter) = &self.buffer_reporter {
                            reporter.report(PlaybackBufferState::Underrun);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(SymphoniaError::IoError(_)) => {
                    // EOF or other IO error
                    log::info!(
                        "IncrementalStreamingSource: EOF reached after {} packets",
                        self.packets_decoded
                    );
                    self.finished = true;
                    if self.buffered_source.download_error().is_some() {
                        if let Some(reporter) = &self.buffer_reporter {
                            reporter.report(PlaybackBufferState::Error);
                        }
                    }
                    return;
                }
                Err(err) => {
                    log::error!("Symphonia read error in stream: {}", err);
                    self.finished = true;
                    if let Some(reporter) = &self.buffer_reporter {
                        reporter.report(PlaybackBufferState::Error);
                    }
                    return;
                }
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(audio_buf) => {
                    // Vorbis/Opus setup, comment, and padding packets can decode to zero frames.
                    // Passing one to copy_interleaved_ref panics on the CPAL output thread,
                    // tearing down audio output and closing its command channel for the session.
                    if audio_buf.frames() == 0 {
                        continue;
                    }
                    let spec = *audio_buf.spec();
                    let mut sample_buf = SampleBuffer::<f32>::new(audio_buf.frames() as u64, spec);
                    sample_buf.copy_interleaved_ref(audio_buf);

                    // Add samples to queue
                    self.sample_queue
                        .extend(sample_buf.samples().iter().copied());
                    let ready_edge = should_report_ready(self.packets_decoded, self.stalled);
                    self.packets_decoded += 1;
                    // Successful decode ends any stall episode; the next
                    // WouldBlock streak records a fresh underrun.
                    self.stalled = false;
                    if ready_edge {
                        if let Some(reporter) = &self.buffer_reporter {
                            reporter.report(PlaybackBufferState::Ready);
                        }
                    }
                }
                Err(SymphoniaError::DecodeError(e)) => {
                    log::warn!("Decode error (skipping packet): {}", e);
                    continue;
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(err) => {
                    log::error!("Symphonia decode error: {}", err);
                    self.finished = true;
                    if let Some(reporter) = &self.buffer_reporter {
                        reporter.report(PlaybackBufferState::Error);
                    }
                    return;
                }
            }
        }
    }
}

const fn should_report_ready(packets_decoded: u64, stalled: bool) -> bool {
    packets_decoded == 0 || stalled
}

impl Source for IncrementalStreamingSource {
    fn current_span_len(&self) -> Option<usize> {
        // We don't know frame boundaries in the queue
        None
    }

    fn channels(&self) -> std::num::NonZero<u16> {
        std::num::NonZero::new(self.channels).unwrap()
    }

    fn sample_rate(&self) -> std::num::NonZero<u32> {
        std::num::NonZero::new(self.sample_rate).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        // We don't know total duration until download completes
        // Could estimate from content-length if available
        None
    }
}

impl Iterator for IncrementalStreamingSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        // If queue is running low, decode more
        // Keep at least 0.5 seconds of audio buffered
        let min_buffer = (self.sample_rate as usize * self.channels as usize) / 2;
        if self.sample_queue.len() < min_buffer {
            self.decode_more(min_buffer);
        }

        self.sample_queue.pop_front()
    }
}

/// Shared, immutable audio bytes: lets several readers (the player, the cast
/// media server, the cast visualizer's shadow decoder) sit on ONE copy of a
/// 100 MB track instead of cloning it per consumer.
pub struct SharedBytes(pub Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// Cursor-backed MediaSource for in-memory audio data.
struct InMemoryMediaSource {
    inner: Cursor<SharedBytes>,
    len: u64,
}

impl InMemoryMediaSource {
    fn shared(data: Arc<Vec<u8>>) -> Self {
        let len = data.len() as u64;
        Self {
            inner: Cursor::new(SharedBytes(data)),
            len,
        }
    }
}

impl Read for InMemoryMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.inner.read(buf)
    }
}

impl Seek for InMemoryMediaSource {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        self.inner.seek(pos)
    }
}

impl MediaSource for InMemoryMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

/// Symphonia-backed decoder for fully-in-memory audio bytes, with native
/// seek support.
///
/// Exists because rodio's `skip_duration` decodes every sample from the
/// start of the track when seeking, which on FLAC Hi-Res costs several
/// seconds of CPU for long jumps and stalls the audio thread. This
/// source uses `FormatReader::seek(Accurate, SeekTo::Time)` — FLAC seek
/// table, MP3 Xing/VBRI TOC — to jump straight to the target sample, so
/// the post-seek decode window is ~O(seek point density) instead of
/// O(position).
///
/// Non-Symphonia formats (notably rodio's native MP4/AAC path) aren't
/// supported here; callers must fall back to `decode_with_fallback` +
/// `skip_duration` when `new` returns `Err`.
pub struct InMemorySource {
    sample_rate: u32,
    channels: u16,
    sample_queue: VecDeque<f32>,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    finished: bool,
}

impl InMemorySource {
    pub fn new(data: Vec<u8>) -> Result<Self, String> {
        Self::from_shared(Arc::new(data))
    }

    /// Decode bytes that another consumer keeps alive too (no copy).
    pub fn from_shared(data: Arc<Vec<u8>>) -> Result<Self, String> {
        let source = Box::new(InMemoryMediaSource::shared(data)) as Box<dyn MediaSource>;
        let mss = MediaSourceStream::new(source, Default::default());

        let hint = Hint::new();

        let format_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let metadata_opts: MetadataOptions = Default::default();

        let probed = get_probe()
            .format(&hint, mss, &format_opts, &metadata_opts)
            .map_err(|err| format!("Symphonia probe failed for in-memory source: {}", err))?;

        let track = probed
            .format
            .default_track()
            .ok_or_else(|| "Symphonia: no supported audio tracks".to_string())?;

        let track_id = track.id;
        let codec_params = track.codec_params.clone();

        let sample_rate = codec_params
            .sample_rate
            .ok_or_else(|| "No sample rate in codec params".to_string())?;
        let channels = codec_params.channels.map(|c| c.count() as u16).unwrap_or(2);

        let decoder = get_codecs()
            .make(&codec_params, &DecoderOptions::default())
            .map_err(|err| format!("Symphonia decoder init failed: {}", err))?;

        Ok(Self {
            sample_rate,
            channels,
            sample_queue: VecDeque::with_capacity(sample_rate as usize * channels as usize),
            format: probed.format,
            decoder,
            track_id,
            finished: false,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn seek_to(&mut self, time: Duration) -> Result<(), String> {
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: time.into(),
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| format!("Symphonia in-memory seek failed: {}", e))?;
        self.decoder.reset();
        self.sample_queue.clear();
        self.finished = false;
        Ok(())
    }

    fn decode_more(&mut self, min_samples: usize) {
        if self.finished {
            return;
        }

        while self.sample_queue.len() < min_samples {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(_)) => {
                    self.finished = true;
                    return;
                }
                Err(err) => {
                    log::error!("Symphonia read error in in-memory source: {}", err);
                    self.finished = true;
                    return;
                }
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(audio_buf) => {
                    // Vorbis/Opus setup, comment, and padding packets can decode to zero frames.
                    // Passing one to copy_interleaved_ref panics on the CPAL output thread,
                    // tearing down audio output and closing its command channel for the session.
                    if audio_buf.frames() == 0 {
                        continue;
                    }
                    let spec = *audio_buf.spec();
                    let mut sample_buf = SampleBuffer::<f32>::new(audio_buf.frames() as u64, spec);
                    sample_buf.copy_interleaved_ref(audio_buf);
                    self.sample_queue
                        .extend(sample_buf.samples().iter().copied());
                }
                Err(SymphoniaError::DecodeError(e)) => {
                    log::warn!("Decode error (skipping packet): {}", e);
                    continue;
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(err) => {
                    log::error!("Symphonia decode error: {}", err);
                    self.finished = true;
                    return;
                }
            }
        }
    }
}

impl Source for InMemorySource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> std::num::NonZero<u16> {
        std::num::NonZero::new(self.channels).unwrap()
    }

    fn sample_rate(&self) -> std::num::NonZero<u32> {
        std::num::NonZero::new(self.sample_rate).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Iterator for InMemorySource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let min_buffer = (self.sample_rate as usize * self.channels as usize) / 2;
        if self.sample_queue.len() < min_buffer {
            self.decode_more(min_buffer);
        }
        self.sample_queue.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn ready_reports_only_on_first_decode_and_stall_recovery() {
        assert!(should_report_ready(0, false));
        assert!(!should_report_ready(1, false));
        assert!(!should_report_ready(400, false));
        assert!(should_report_ready(1, true));
        assert!(should_report_ready(400, true));
    }

    #[test]
    fn first_error_wins_over_later_generic_abort() {
        let (source, writer) = BufferedMediaSource::new(StreamingConfig::from_seconds(1), None);
        writer.error("root cause".to_string()).unwrap();
        writer
            .error("CMAF stream aborted before completion".to_string())
            .unwrap();
        assert_eq!(source.download_error().as_deref(), Some("root cause"));
    }

    #[test]
    fn feeder_error_is_visible_to_waiters_before_min_buffer() {
        let (source, writer) = BufferedMediaSource::new(StreamingConfig::from_seconds(1), None);
        assert!(!source.has_min_buffer());
        assert!(source.download_error().is_none());
        writer.error("feeder died".to_string()).unwrap();
        // The initial-buffer wait loop polls this instead of sleeping out
        // the full buffer timeout.
        assert_eq!(source.download_error().as_deref(), Some("feeder died"));
        assert!(!source.has_min_buffer());
    }

    #[test]
    fn raw_initial_buffer_for_speed_follows_documented_ladder() {
        // Each band of the documented speed ladder produces its own size.
        assert_eq!(raw_initial_buffer_for_speed(20.0), 256 * 1024);
        assert_eq!(raw_initial_buffer_for_speed(10.0), 256 * 1024);
        assert_eq!(raw_initial_buffer_for_speed(7.0), 384 * 1024);
        assert_eq!(raw_initial_buffer_for_speed(5.0), 384 * 1024);
        assert_eq!(raw_initial_buffer_for_speed(3.0), 512 * 1024);
        assert_eq!(raw_initial_buffer_for_speed(2.0), 512 * 1024);
        assert_eq!(raw_initial_buffer_for_speed(1.5), 1024 * 1024);
        assert_eq!(raw_initial_buffer_for_speed(1.0), 1024 * 1024);
        assert_eq!(raw_initial_buffer_for_speed(0.5), 2 * 1024 * 1024);
        assert_eq!(raw_initial_buffer_for_speed(0.0), 2 * 1024 * 1024);
    }

    #[test]
    fn from_speed_mbps_with_cap_passes_through_when_under_cap() {
        // Cap above the raw value: result equals the raw ladder.
        let cfg = StreamingConfig::from_speed_mbps_with_cap(0.0, 4 * 1024 * 1024);
        assert_eq!(cfg.initial_buffer_bytes, 2 * 1024 * 1024);
    }

    #[test]
    fn from_speed_mbps_with_cap_clamps_slow_connection_to_low_memory_cap() {
        // The case from issue #331: Pi 3B, slow connection because of swap
        // thrash, would otherwise inflate the buffer to 2 MB. With the
        // LowMemory profile's 256KB cap applied, we stay at 256KB.
        let cfg = StreamingConfig::from_speed_mbps_with_cap(0.0, 256 * 1024);
        assert_eq!(cfg.initial_buffer_bytes, 256 * 1024);

        let cfg = StreamingConfig::from_speed_mbps_with_cap(1.5, 256 * 1024);
        assert_eq!(cfg.initial_buffer_bytes, 256 * 1024);
    }

    #[test]
    fn from_speed_mbps_with_cap_no_op_for_normal_profile() {
        // Normal profile cap is 2 MB — equal to the slowest raw band, so
        // any raw value passes through unchanged.
        let cap = 2 * 1024 * 1024;
        for speed in [0.0, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0] {
            let cfg = StreamingConfig::from_speed_mbps_with_cap(speed, cap);
            assert_eq!(
                cfg.initial_buffer_bytes,
                raw_initial_buffer_for_speed(speed),
                "cap should not bind for speed={}",
                speed
            );
        }
    }

    #[test]
    fn from_speed_mbps_with_cap_max_buffer_unchanged() {
        // Whatever the cap, the secondary max_buffer_bytes stays at its
        // module default; we are only clamping the initial fill target.
        let cfg = StreamingConfig::from_speed_mbps_with_cap(0.5, 64 * 1024);
        assert_eq!(cfg.max_buffer_bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn prealloc_capacity_decision() {
        // Unknown or zero size: no reservation, caller uses the initial
        // buffer capacity instead.
        assert_eq!(prealloc_capacity(None), None);
        assert_eq!(prealloc_capacity(Some(0)), None);
        // Known size reserves exactly that (no doubling slack).
        assert_eq!(prealloc_capacity(Some(60 * 1024 * 1024)), Some(60 * 1024 * 1024));
        assert_eq!(prealloc_capacity(Some(1)), Some(1));
        // Absurd sizes are capped at 1 GiB.
        assert_eq!(
            prealloc_capacity(Some(8 * 1024 * 1024 * 1024)),
            Some(MAX_PREALLOC_BYTES as usize)
        );
    }

    #[test]
    fn new_source_preallocates_known_total_size() {
        let config = StreamingConfig {
            initial_buffer_bytes: 16,
            max_buffer_bytes: 100,
        };
        let (_source, writer) = BufferedMediaSource::new(config, Some(4096));
        // The writer's buffer should have reserved the full track up front.
        // (No public capacity accessor — push the full size and ensure the
        // data lands intact, which is the observable contract.)
        let payload = vec![0xABu8; 4096];
        writer.push_chunk(&payload).unwrap();
        writer.complete().unwrap();
        assert_eq!(writer.buffer_size(), 4096);
    }

    #[test]
    fn write_buffered_to_copies_full_contents() {
        let config = StreamingConfig {
            initial_buffer_bytes: 4,
            max_buffer_bytes: 100,
        };
        let (_source, writer) = BufferedMediaSource::new(config, None);
        writer.push_chunk(b"Hello, ").unwrap();
        writer.push_chunk(b"world!").unwrap();
        writer.complete().unwrap();

        let mut out: Vec<u8> = Vec::new();
        let written = writer.write_buffered_to(&mut out).unwrap();
        assert_eq!(written, 13);
        assert_eq!(&out, b"Hello, world!");
    }

    #[test]
    fn write_buffered_to_handles_empty_buffer() {
        let config = StreamingConfig {
            initial_buffer_bytes: 4,
            max_buffer_bytes: 100,
        };
        let (_source, writer) = BufferedMediaSource::new(config, None);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(writer.write_buffered_to(&mut out).unwrap(), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn test_basic_read_write() {
        let config = StreamingConfig {
            initial_buffer_bytes: 10,
            max_buffer_bytes: 100,
        };
        let (mut source, writer) = BufferedMediaSource::new(config, Some(20));

        // Write some data
        writer.push_chunk(b"Hello").unwrap();
        writer.push_chunk(b"World").unwrap();

        // Read it back
        let mut buf = [0u8; 5];
        assert_eq!(source.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf, b"Hello");

        assert_eq!(source.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf, b"World");
    }

    #[test]
    fn test_seek_within_buffer() {
        let config = StreamingConfig {
            initial_buffer_bytes: 5,
            max_buffer_bytes: 100,
        };
        let (mut source, writer) = BufferedMediaSource::new(config, Some(10));

        writer.push_chunk(b"0123456789").unwrap();
        writer.complete().unwrap();

        // Read first 5 bytes
        let mut buf = [0u8; 5];
        source.read(&mut buf).unwrap();
        assert_eq!(&buf, b"01234");

        // Seek back to start
        source.seek(SeekFrom::Start(0)).unwrap();
        source.read(&mut buf).unwrap();
        assert_eq!(&buf, b"01234");

        // Seek to middle
        source.seek(SeekFrom::Start(3)).unwrap();
        source.read(&mut buf).unwrap();
        assert_eq!(&buf, b"34567");
    }

    #[test]
    fn test_complete_data_retrieval() {
        let config = StreamingConfig {
            initial_buffer_bytes: 5,
            max_buffer_bytes: 100,
        };
        let (source, writer) = BufferedMediaSource::new(config, Some(10));

        writer.push_chunk(b"Hello").unwrap();
        assert!(source.take_complete_data().is_none()); // Not complete yet

        writer.push_chunk(b"World").unwrap();
        writer.complete().unwrap();

        let data = source.take_complete_data().unwrap();
        assert_eq!(&data, b"HelloWorld");
    }

    #[test]
    fn test_blocking_read() {
        let config = StreamingConfig {
            initial_buffer_bytes: 5,
            max_buffer_bytes: 100,
        };
        let (mut source, writer) = BufferedMediaSource::new(config, None);

        // Spawn thread to write after delay
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            writer.push_chunk(b"Delayed").unwrap();
            writer.complete().unwrap();
        });

        // This should block until data arrives
        let mut buf = [0u8; 7];
        let n = source.read(&mut buf).unwrap();
        assert_eq!(n, 7);
        assert_eq!(&buf, b"Delayed");
    }

    #[test]
    fn error_unblocks_reader_with_io_error() {
        use std::io::ErrorKind;
        let config = StreamingConfig {
            initial_buffer_bytes: 5,
            max_buffer_bytes: 100,
        };
        let (mut source, writer) = BufferedMediaSource::new(config, None);
        writer.error("cdn failed".into()).unwrap();
        let mut buf = [0u8; 8];
        let err = source.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Other);
        let msg = err.to_string();
        assert!(msg.contains("cdn failed"), "{msg}");
    }
}
