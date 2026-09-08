//! Progress events for the offline-cache download pipeline.
//!
//! The crate is frontend-agnostic, so the download pipeline reports
//! progress through a `CacheEventSink` callback instead of Tauri events.
//! The Tauri frontend wraps the sink as `app.emit("offline:caching_*", ..)`
//! (preserving the exact Svelte IPC shapes); the Slint frontend wraps it
//! as a push into its event loop / model.

/// Which on-disk format a completed/processed download produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheFormat {
    /// `cache_format = 1` — legacy plain FLAC at `file_path`.
    Flac,
    /// `cache_format = 2` — raw encrypted CMAF bundle.
    Cmaf,
}

impl CacheFormat {
    /// The string the Tauri frontend expects in the event payload.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Cmaf => "cmaf",
        }
    }
}

/// A progress/lifecycle event emitted while caching a track. Maps 1:1 to
/// the legacy Tauri `offline:caching_*` / `offline:unlock_*` events.
#[derive(Debug, Clone)]
pub enum CacheEvent {
    /// Download started (`offline:caching_started`).
    Started { track_id: u64 },
    /// Byte/segment progress (`offline:caching_progress`). `status` was
    /// always "downloading" in the Tauri payload.
    Progress {
        track_id: u64,
        progress_percent: u8,
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
    },
    /// Bytes fully fetched (`offline:caching_completed`). For the legacy
    /// path the Tauri payload omitted `format`; for CMAF it was "cmaf".
    Completed {
        track_id: u64,
        size: u64,
        format: CacheFormat,
    },
    /// Post-processing done — tags/organize/library row written
    /// (`offline:caching_processed`). `path` is the final on-disk location.
    Processed {
        track_id: u64,
        path: String,
        format: CacheFormat,
    },
    /// Download or post-processing failed (`offline:caching_failed`).
    Failed { track_id: u64, error: String },
    /// CMAF decrypt started for playback (`offline:unlock_start`).
    UnlockStart { track_id: u64 },
    /// CMAF decrypt finished (`offline:unlock_end`).
    UnlockEnd { track_id: u64, success: bool },
}

/// A thread-safe sink the download/playback pipeline calls to report
/// `CacheEvent`s. Cloneable; cheap to pass around.
pub type CacheEventSink = std::sync::Arc<dyn Fn(CacheEvent) + Send + Sync>;
