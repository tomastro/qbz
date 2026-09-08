//! L2 Disk Cache - File-based playback cache
//!
//! Secondary cache for audio data evicted from memory.
//! Provides faster access than re-downloading from network.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

/// Entry metadata for tracking cache usage
#[derive(Debug, Clone)]
struct CacheEntry {
    #[allow(dead_code)]
    track_id: u64,
    size_bytes: u64,
    last_accessed: SystemTime,
}

/// Disk-based playback cache state
struct PlaybackCacheState {
    /// Track metadata keyed by track ID
    entries: HashMap<u64, CacheEntry>,
    /// Current total size in bytes
    current_size: u64,
}

/// Disk-based playback cache for evicted tracks
///
/// Stores audio data as files on disk with LRU eviction.
/// Files are named `{track_id}.audio` in the cache directory.
pub struct PlaybackCache {
    state: Mutex<PlaybackCacheState>,
    /// Cache directory path
    cache_dir: PathBuf,
    /// Maximum cache size in bytes
    max_size_bytes: u64,
}

impl PlaybackCache {
    /// Create a new playback cache with default location
    ///
    /// Default path: `~/.cache/qbz/playback/`
    pub fn new(max_size_bytes: u64) -> Result<Self, String> {
        let cache_dir = dirs::cache_dir()
            .ok_or("Could not determine cache directory")?
            .join("qbz")
            .join("playback");

        Self::with_path(cache_dir, max_size_bytes)
    }

    /// Create a new playback cache at a specific path
    pub fn with_path(cache_dir: PathBuf, max_size_bytes: u64) -> Result<Self, String> {
        // Create directory
        fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create playback cache directory: {}", e))?;

        let cache = Self {
            state: Mutex::new(PlaybackCacheState {
                entries: HashMap::new(),
                current_size: 0,
            }),
            cache_dir,
            max_size_bytes,
        };

        // Scan existing files to rebuild state
        cache.rebuild_state();

        log::info!(
            "Playback cache initialized at {:?} (max {} MB)",
            cache.cache_dir,
            max_size_bytes / (1024 * 1024)
        );

        Ok(cache)
    }

    /// Rebuild cache state from existing files on disk
    fn rebuild_state(&self) {
        let mut state = self.state.lock().unwrap();
        state.entries.clear();
        state.current_size = 0;

        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        // Parse track ID from filename (format: {track_id}.audio)
                        if let Some(filename) = entry.file_name().to_str() {
                            if let Some(id_str) = filename.strip_suffix(".audio") {
                                if let Ok(track_id) = id_str.parse::<u64>() {
                                    let size = metadata.len();
                                    let last_accessed =
                                        metadata.accessed().unwrap_or_else(|_| SystemTime::now());

                                    state.entries.insert(
                                        track_id,
                                        CacheEntry {
                                            track_id,
                                            size_bytes: size,
                                            last_accessed,
                                        },
                                    );
                                    state.current_size += size;
                                }
                            }
                        }
                    }
                }
            }
        }

        log::info!(
            "Playback cache rebuilt: {} tracks, {} MB",
            state.entries.len(),
            state.current_size / (1024 * 1024)
        );
    }

    /// Get file path for a track
    fn track_path(&self, track_id: u64) -> PathBuf {
        self.cache_dir.join(format!("{}.audio", track_id))
    }

    /// Check if a track is in the cache
    pub fn contains(&self, track_id: u64) -> bool {
        self.state.lock().unwrap().entries.contains_key(&track_id)
    }

    /// Get a track from the cache
    pub fn get(&self, track_id: u64) -> Option<Vec<u8>> {
        let path = self.track_path(track_id);

        // Check if file exists and read it
        if !path.exists() {
            // File was deleted externally, update state
            let mut state = self.state.lock().unwrap();
            if let Some(entry) = state.entries.remove(&track_id) {
                state.current_size = state.current_size.saturating_sub(entry.size_bytes);
            }
            return None;
        }

        match fs::File::open(&path) {
            Ok(mut file) => {
                let mut data = Vec::new();
                if file.read_to_end(&mut data).is_ok() {
                    // Update last accessed time
                    let mut state = self.state.lock().unwrap();
                    if let Some(entry) = state.entries.get_mut(&track_id) {
                        entry.last_accessed = SystemTime::now();
                    }

                    // Touch file to update filesystem access time
                    let _ = filetime::set_file_atime(&path, filetime::FileTime::now());

                    log::debug!(
                        "Playback cache hit for track {} ({} bytes)",
                        track_id,
                        data.len()
                    );
                    Some(data)
                } else {
                    log::warn!("Failed to read playback cache file for track {}", track_id);
                    None
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to open playback cache file for track {}: {}",
                    track_id,
                    e
                );
                None
            }
        }
    }

    /// Insert a track into the cache (called when evicting from memory cache)
    pub fn insert(&self, track_id: u64, data: &[u8]) {
        let size = data.len() as u64;

        // Don't cache if larger than max size
        if size > self.max_size_bytes {
            log::debug!(
                "Track {} too large for playback cache ({} MB > {} MB)",
                track_id,
                size / (1024 * 1024),
                self.max_size_bytes / (1024 * 1024)
            );
            return;
        }

        // Evict old entries if needed
        self.evict_if_needed(size);

        let path = self.track_path(track_id);

        // Write file
        match fs::File::create(&path) {
            Ok(mut file) => {
                if file.write_all(data).is_ok() {
                    let mut state = self.state.lock().unwrap();

                    // Remove old entry if exists
                    if let Some(old) = state.entries.remove(&track_id) {
                        state.current_size = state.current_size.saturating_sub(old.size_bytes);
                    }

                    // Add new entry
                    state.entries.insert(
                        track_id,
                        CacheEntry {
                            track_id,
                            size_bytes: size,
                            last_accessed: SystemTime::now(),
                        },
                    );
                    state.current_size += size;

                    log::info!(
                        "Saved track {} to playback cache ({} KB). Total: {} MB / {} MB",
                        track_id,
                        size / 1024,
                        state.current_size / (1024 * 1024),
                        self.max_size_bytes / (1024 * 1024)
                    );
                } else {
                    log::warn!("Failed to write playback cache file for track {}", track_id);
                    let _ = fs::remove_file(&path);
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to create playback cache file for track {}: {}",
                    track_id,
                    e
                );
            }
        }
    }

    /// Insert a track whose bytes are streamed in by `fill` rather than
    /// passed as an already-materialized slice. Used by the low-memory
    /// oversized-track path, where a second full in-RAM copy of the track
    /// just to hand `insert` a `&[u8]` is exactly what we're avoiding —
    /// `fill` typically chunked-copies straight out of the playback buffer.
    ///
    /// `size_hint` is the expected byte count: it drives the too-large
    /// rejection and pre-write eviction; the entry records the actual bytes
    /// written.
    pub fn insert_from<F>(&self, track_id: u64, size_hint: u64, fill: F)
    where
        F: FnOnce(&mut fs::File) -> std::io::Result<usize>,
    {
        // Don't cache if larger than max size
        if size_hint > self.max_size_bytes {
            log::debug!(
                "Track {} too large for playback cache ({} MB > {} MB)",
                track_id,
                size_hint / (1024 * 1024),
                self.max_size_bytes / (1024 * 1024)
            );
            return;
        }

        // Evict old entries if needed
        self.evict_if_needed(size_hint);

        let path = self.track_path(track_id);

        match fs::File::create(&path) {
            Ok(mut file) => match fill(&mut file) {
                Ok(written) => {
                    let size = written as u64;
                    let mut state = self.state.lock().unwrap();

                    // Remove old entry if exists
                    if let Some(old) = state.entries.remove(&track_id) {
                        state.current_size = state.current_size.saturating_sub(old.size_bytes);
                    }

                    state.entries.insert(
                        track_id,
                        CacheEntry {
                            track_id,
                            size_bytes: size,
                            last_accessed: SystemTime::now(),
                        },
                    );
                    state.current_size += size;

                    log::info!(
                        "Saved track {} to playback cache ({} KB). Total: {} MB / {} MB",
                        track_id,
                        size / 1024,
                        state.current_size / (1024 * 1024),
                        self.max_size_bytes / (1024 * 1024)
                    );
                }
                Err(e) => {
                    log::warn!(
                        "Failed to stream playback cache file for track {}: {}",
                        track_id,
                        e
                    );
                    let _ = fs::remove_file(&path);
                }
            },
            Err(e) => {
                log::warn!(
                    "Failed to create playback cache file for track {}: {}",
                    track_id,
                    e
                );
            }
        }
    }

    /// Evict oldest entries to make room for new data
    fn evict_if_needed(&self, needed_bytes: u64) {
        let mut state = self.state.lock().unwrap();

        while state.current_size + needed_bytes > self.max_size_bytes && !state.entries.is_empty() {
            // Find oldest entry
            let oldest_id = state
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_accessed)
                .map(|(id, _)| *id);

            if let Some(track_id) = oldest_id {
                if let Some(entry) = state.entries.remove(&track_id) {
                    state.current_size = state.current_size.saturating_sub(entry.size_bytes);

                    // Delete file
                    let path = self.cache_dir.join(format!("{}.audio", track_id));
                    if let Err(e) = fs::remove_file(&path) {
                        log::debug!("Failed to delete playback cache file: {}", e);
                    } else {
                        log::debug!(
                            "Evicted track {} from playback cache ({} KB)",
                            track_id,
                            entry.size_bytes / 1024
                        );
                    }
                }
            } else {
                break;
            }
        }
    }

    /// Clear the entire cache
    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap();

        for track_id in state.entries.keys() {
            let path = self.cache_dir.join(format!("{}.audio", track_id));
            let _ = fs::remove_file(&path);
        }

        state.entries.clear();
        state.current_size = 0;

        log::info!("Playback cache cleared");
    }

    /// Get cache statistics
    pub fn stats(&self) -> PlaybackCacheStats {
        let state = self.state.lock().unwrap();
        PlaybackCacheStats {
            cached_tracks: state.entries.len(),
            current_size_bytes: state.current_size,
            max_size_bytes: self.max_size_bytes,
        }
    }

    /// Get the cache directory path
    pub fn cache_dir(&self) -> &PathBuf {
        &self.cache_dir
    }
}

/// Playback cache statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlaybackCacheStats {
    pub cached_tracks: usize,
    pub current_size_bytes: u64,
    pub max_size_bytes: u64,
}
