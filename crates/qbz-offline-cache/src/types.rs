//! Shared DTOs for the offline cache (moved verbatim from
//! `src-tauri/src/offline_cache/mod.rs`). Pure serde — no Tauri, no I/O.

use serde::{Deserialize, Serialize};

/// Cache status for a track in offline storage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfflineCacheStatus {
    Queued,
    Downloading,
    Ready,
    Failed,
}

impl OfflineCacheStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "downloading" => Self::Downloading,
            "ready" => Self::Ready,
            "failed" => Self::Failed,
            _ => Self::Failed,
        }
    }
}

/// Information about a cached track
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedTrackInfo {
    pub track_id: u64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub duration_secs: u64,
    pub file_size_bytes: u64,
    pub quality: String,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<f64>,
    pub status: OfflineCacheStatus,
    pub progress_percent: u8,
    pub error_message: Option<String>,
    pub created_at: String,
    pub last_accessed_at: String,
    /// The index's `artwork_path` column. Historically never backfilled by
    /// the downloaders (they record the cover path in library.db only), so
    /// treat it as a hint — [`Self::resolve_cover_path`] falls back to the
    /// on-disk layouts when it is unset.
    pub artwork_path: Option<String>,
    /// The audio path: the organized FLAC for v1 rows, the CMAF segments
    /// path (inside `tracks-cmaf/<id>/`) for v2 rows.
    pub file_path: String,
}

impl CachedTrackInfo {
    /// Resolve this row's on-disk cover thumbnail, if any:
    /// 1. the index's `artwork_path`, when set and still on disk;
    /// 2. the v2 CMAF bundle's `<cache_path>/tracks-cmaf/<id>/cover.jpg`;
    /// 3. the `cover.jpg` sibling of `file_path` — v1 rows save the cover
    ///    next to the organized FLAC without backfilling `artwork_path`.
    pub fn resolve_cover_path(&self, cache_path: &str) -> Option<String> {
        use std::path::Path;
        if let Some(p) = self.artwork_path.as_deref().filter(|p| !p.is_empty()) {
            if Path::new(p).is_file() {
                return Some(p.to_string());
            }
        }
        let cmaf = Path::new(cache_path)
            .join("tracks-cmaf")
            .join(self.track_id.to_string())
            .join("cover.jpg");
        if cmaf.is_file() {
            return Some(cmaf.to_string_lossy().to_string());
        }
        let fp = Path::new(&self.file_path);
        let folder = if fp.is_dir() { Some(fp) } else { fp.parent() };
        if let Some(dir) = folder {
            let sibling = dir.join("cover.jpg");
            if sibling.is_file() {
                return Some(sibling.to_string_lossy().to_string());
            }
        }
        None
    }
}

/// Minimal track info for syncing to library
#[derive(Debug, Clone)]
pub struct ReadyTrackForSync {
    pub track_id: u64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: u64,
    pub file_path: String,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<f64>,
}

/// Statistics about the offline cache
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineCacheStats {
    pub total_tracks: usize,
    pub ready_tracks: usize,
    pub downloading_tracks: usize,
    pub failed_tracks: usize,
    pub total_size_bytes: u64,
    pub limit_bytes: Option<u64>,
    pub cache_path: String,
}

/// Progress update for caching a track
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheProgress {
    pub track_id: u64,
    pub progress_percent: u8,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub status: OfflineCacheStatus,
}

/// Track metadata for initiating offline caching
#[derive(Debug, Clone)]
pub struct TrackCacheInfo {
    pub track_id: u64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub duration_secs: u64,
    pub quality: String,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<f64>,
}
