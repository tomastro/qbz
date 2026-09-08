//! L1 Memory Cache - In-memory LRU cache for audio data
//!
//! Fast access cache with configurable size limit and LRU eviction.
//! Evicted tracks can optionally spill to L2 disk cache.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::PlaybackCache;

/// Cached audio data for a track
#[derive(Clone)]
pub struct CachedTrack {
    pub track_id: u64,
    pub data: Vec<u8>,
    pub size_bytes: usize,
}

/// Internal cache state - all in one struct to avoid deadlocks
struct CacheState {
    /// Cached tracks keyed by track ID
    tracks: HashMap<u64, CachedTrack>,
    /// Order of access for LRU eviction (most recent at back)
    access_order: Vec<u64>,
    /// Current cache size in bytes
    current_size: usize,
    /// Track IDs currently being fetched
    fetching: HashSet<u64>,
    /// Track IDs whose last prefetch failed, with when it failed. Lets the
    /// prefetch scheduler back off a track that is currently un-fetchable
    /// (e.g. the account is being 403'd) instead of re-hammering it every
    /// queue tick and feeding a request storm (issue #637).
    failed: HashMap<u64, Instant>,
}

/// Audio cache manager with LRU eviction and optional disk spillover
///
/// Provides fast in-memory caching with automatic eviction when the
/// size limit is reached. Evicted tracks are written to the L2 disk
/// cache (if configured) for later retrieval.
pub struct AudioCache {
    state: Mutex<CacheState>,
    /// Maximum cache size in bytes
    max_size_bytes: usize,
    /// Optional disk-based L2 cache for evicted tracks
    playback_cache: Option<Arc<PlaybackCache>>,
}

impl Default for AudioCache {
    fn default() -> Self {
        Self::new(400 * 1024 * 1024) // 400MB for ~4-5 Hi-Res tracks
    }
}

impl AudioCache {
    /// Create a new cache with specified max size in bytes
    pub fn new(max_size_bytes: usize) -> Self {
        Self {
            state: Mutex::new(CacheState {
                tracks: HashMap::new(),
                access_order: Vec::new(),
                current_size: 0,
                fetching: HashSet::new(),
                failed: HashMap::new(),
            }),
            max_size_bytes,
            playback_cache: None,
        }
    }

    /// Create cache with disk spillover enabled
    pub fn with_playback_cache(max_size_bytes: usize, playback_cache: Arc<PlaybackCache>) -> Self {
        Self {
            state: Mutex::new(CacheState {
                tracks: HashMap::new(),
                access_order: Vec::new(),
                current_size: 0,
                fetching: HashSet::new(),
                failed: HashMap::new(),
            }),
            max_size_bytes,
            playback_cache: Some(playback_cache),
        }
    }

    /// Set the playback cache for disk spillover
    pub fn set_playback_cache(&mut self, cache: Arc<PlaybackCache>) {
        self.playback_cache = Some(cache);
    }

    /// Get the playback cache reference
    pub fn get_playback_cache(&self) -> Option<&Arc<PlaybackCache>> {
        self.playback_cache.as_ref()
    }

    /// Get a track from cache if available
    pub fn get(&self, track_id: u64) -> Option<CachedTrack> {
        let mut state = self.state.lock().unwrap();

        let track = state.tracks.get(&track_id).cloned();

        if track.is_some() {
            // Update access order (move to back = most recently used)
            state.access_order.retain(|&id| id != track_id);
            state.access_order.push(track_id);
            log::debug!("Cache hit for track {}", track_id);
        } else {
            log::debug!("Cache miss for track {}", track_id);
        }

        track
    }

    /// Check if a track is in cache without updating access order
    pub fn contains(&self, track_id: u64) -> bool {
        self.state.lock().unwrap().tracks.contains_key(&track_id)
    }

    /// Check if a track is currently being fetched
    pub fn is_fetching(&self, track_id: u64) -> bool {
        self.state.lock().unwrap().fetching.contains(&track_id)
    }

    /// Mark a track as being fetched
    pub fn mark_fetching(&self, track_id: u64) {
        self.state.lock().unwrap().fetching.insert(track_id);
    }

    /// Unmark a track as being fetched
    pub fn unmark_fetching(&self, track_id: u64) {
        self.state.lock().unwrap().fetching.remove(&track_id);
    }

    /// Record that a prefetch for this track failed (starts a back-off window).
    pub fn mark_failed(&self, track_id: u64) {
        self.state
            .lock()
            .unwrap()
            .failed
            .insert(track_id, Instant::now());
    }

    /// True if the track failed to prefetch within `cooldown` — the scheduler
    /// uses this to skip re-hammering a currently un-fetchable track (issue
    /// #637). Expired entries are cleaned up on read.
    pub fn recently_failed(&self, track_id: u64, cooldown: Duration) -> bool {
        let mut state = self.state.lock().unwrap();
        match state.failed.get(&track_id) {
            Some(when) if when.elapsed() < cooldown => true,
            Some(_) => {
                state.failed.remove(&track_id);
                false
            }
            None => false,
        }
    }

    /// Clear a track's failure marker (e.g. once it is successfully cached).
    pub fn clear_failed(&self, track_id: u64) {
        self.state.lock().unwrap().failed.remove(&track_id);
    }

    /// Insert a track into cache, evicting old entries to disk if needed
    pub fn insert(&self, track_id: u64, data: Vec<u8>) {
        let size = data.len();

        // Don't cache if track is larger than max cache size
        if size > self.max_size_bytes {
            log::warn!(
                "Track {} ({} bytes) too large for cache (max {} bytes)",
                track_id,
                size,
                self.max_size_bytes
            );
            return;
        }

        // Collect tracks to evict (to avoid holding lock while writing to disk)
        let mut tracks_to_spill: Vec<CachedTrack> = Vec::new();

        {
            let mut state = self.state.lock().unwrap();

            // Evict old entries to make room
            while state.current_size + size > self.max_size_bytes && !state.access_order.is_empty()
            {
                let oldest_id = state.access_order.remove(0);
                if let Some(track) = state.tracks.remove(&oldest_id) {
                    state.current_size = state.current_size.saturating_sub(track.size_bytes);
                    log::debug!(
                        "Evicting track {} ({} bytes) from memory cache",
                        oldest_id,
                        track.size_bytes
                    );
                    tracks_to_spill.push(track);
                }
            }
        }

        // Spill evicted tracks to disk cache (outside of lock)
        if let Some(playback_cache) = &self.playback_cache {
            for track in tracks_to_spill {
                playback_cache.insert(track.track_id, &track.data);
            }
        }

        let mut state = self.state.lock().unwrap();

        // If track already exists, update size tracking
        if let Some(existing) = state.tracks.get(&track_id) {
            state.current_size = state.current_size.saturating_sub(existing.size_bytes);
        }

        let cached = CachedTrack {
            track_id,
            data,
            size_bytes: size,
        };

        state.tracks.insert(track_id, cached);
        state.current_size += size;

        // Update access order
        state.access_order.retain(|&id| id != track_id);
        state.access_order.push(track_id);

        log::info!(
            "Cached track {} ({} bytes). Cache size: {}/{} bytes",
            track_id,
            size,
            state.current_size,
            self.max_size_bytes
        );
    }

    /// Clear all cached data (both L1 memory and L2 disk caches)
    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.tracks.clear();
        state.access_order.clear();
        state.current_size = 0;
        state.fetching.clear();
        state.failed.clear();
        log::info!("L1 memory cache cleared");

        // Also clear L2 disk cache if present
        if let Some(ref playback_cache) = self.playback_cache {
            playback_cache.clear();
            log::info!("L2 playback cache cleared");
        }
    }

    /// Drop every L1 in-memory entry WITHOUT touching the L2 disk cache or
    /// the fetching/failed bookkeeping. The memory-pressure watchdog's
    /// relief valve: RAM is the scarce resource under pressure, the disk
    /// files cost nothing to keep and save a re-download on recovery.
    pub fn evict_all_memory(&self) {
        let mut state = self.state.lock().unwrap();
        let dropped = state.tracks.len();
        state.tracks.clear();
        state.access_order.clear();
        state.current_size = 0;
        log::info!("L1 memory cache evicted ({dropped} tracks dropped, L2 disk cache kept)");
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let state = self.state.lock().unwrap();
        CacheStats {
            cached_tracks: state.tracks.len(),
            current_size_bytes: state.current_size,
            max_size_bytes: self.max_size_bytes,
            fetching_count: state.fetching.len(),
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct CacheStats {
    pub cached_tracks: usize,
    pub current_size_bytes: usize,
    pub max_size_bytes: usize,
    pub fetching_count: usize,
}
