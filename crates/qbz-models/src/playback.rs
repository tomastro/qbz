//! Playback-related types for QBZ
//!
//! This module contains types related to audio playback:
//! - Queue track representation
//! - Repeat mode
//! - Queue state snapshots
//! - Playback state

use serde::{Deserialize, Serialize};

// ============ Queue Types ============

/// Track info stored in the queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueTrack {
    pub id: u64,
    pub title: String,
    /// Subtitle/edition info from Qobuz (e.g. "Player's Ball Mix") that
    /// the frontend renders parenthesized after the title (issue #360).
    #[serde(default)]
    pub version: Option<String>,
    pub artist: String,
    pub album: String,
    /// Album-level release variant ("2009 Remaster", "Hi-Res", …) — distinct
    /// from the per-track `version`. Appended to the album name for the
    /// now-playing bar + MPRIS (NOT for Last.fm scrobbling, which wants the
    /// clean album name). Populated on the album-play path; None elsewhere.
    #[serde(default)]
    pub album_version: Option<String>,
    pub duration_secs: u64,
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub hires: bool,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<f64>,
    /// Whether this is a local library track (not from streaming service)
    #[serde(default)]
    pub is_local: bool,
    /// Album ID for navigation
    pub album_id: Option<String>,
    /// Artist ID for navigation
    pub artist_id: Option<u64>,
    /// Whether the track is streamable (false = removed/unavailable)
    #[serde(default = "default_streamable")]
    pub streamable: bool,
    /// Source identifier (e.g., "qobuz", "local", "plex")
    #[serde(default)]
    pub source: Option<String>,
    /// Parental advisory / explicit content
    #[serde(default)]
    pub parental_warning: bool,
    /// Opaque identifier of the Mixtape/Collection item that produced this track,
    /// used by v2_skip_to_next_item / v2_skip_to_previous_item to detect boundaries.
    /// For non-Mixtape enqueue paths, set to the track's album_id so boundary
    /// detection still works for "play album" flows. None is a safe fallback
    /// (the skip commands fall back to album_id when this is absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_item_id_hint: Option<String>,
    /// The container this track was launched FROM — the "playing from" origin
    /// used by the now-playing song-card "layers" button. `context_kind` is one
    /// of "album" | "artist" | "playlist" | "label"; `context_id` is that
    /// container's navigation id. Stamped per-track at enqueue time so the
    /// button always carries the CURRENT track's true source and is re-derived
    /// on every track change (never a stale single global). None = no container
    /// origin (bare single-track / favorites / mix / search play) → the button
    /// falls back to the track's own album. `serde(default)` keeps the persisted
    /// session-queue back-compatible (older payloads restore as None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Cross-source identity keys, when the source knows them: the ISRC (the
    /// one key Qobuz always has) and the MusicBrainz recording id (Picard-
    /// tagged local files, media servers that expose provider ids). Purely
    /// additive — `None` everywhere that has not been taught them — and
    /// stamped into the listen log, which is the only consumer today.
    ///
    /// NOT an identity on their own: numeric `id`s collide across sources,
    /// so `(source, native item id)` stays the row identity and these are
    /// the JOIN keys between sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_mbid: Option<String>,
}

fn default_streamable() -> bool {
    true
}

/// Repeat mode options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl Default for RepeatMode {
    fn default() -> Self {
        Self::Off
    }
}

/// Queue state snapshot for frontend
#[derive(Debug, Clone, Serialize)]
pub struct QueueState {
    pub current_track: Option<QueueTrack>,
    pub current_index: Option<usize>,
    pub upcoming: Vec<QueueTrack>,
    pub history: Vec<QueueTrack>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub total_tracks: usize,
    pub stop_after_track_id: Option<u64>,
    /// Manual-block size (#442): how many entries right after `current_index`
    /// were hand-queued ("Play next" / "Play later") and play before the
    /// source resumes.
    pub manual_next_count: usize,
}

// ============ Playback State ============

/// Current playback state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackState {
    /// No track loaded
    Stopped,
    /// Track loaded and playing
    Playing,
    /// Track loaded but paused
    Paused,
    /// Loading/buffering track
    Loading,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self::Stopped
    }
}

/// Detailed playback status with position and duration
#[derive(Debug, Clone, Serialize)]
pub struct PlaybackStatus {
    pub state: PlaybackState,
    pub track_id: Option<u64>,
    pub position_secs: u64,
    pub duration_secs: u64,
    pub volume: f32,
    /// Sample rate of currently playing track (Hz)
    pub sample_rate: Option<u32>,
    /// Bit depth of currently playing track
    pub bit_depth: Option<u32>,
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        Self {
            state: PlaybackState::Stopped,
            track_id: None,
            position_secs: 0,
            duration_secs: 0,
            volume: 1.0,
            sample_rate: None,
            bit_depth: None,
        }
    }
}

// Note: Audio backend types (AudioBackendType, AudioDevice, etc.) are defined
// in qbz-audio crate to keep the audio module self-contained and immutable.
