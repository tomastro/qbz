//! Core events for frontend communication
//!
//! This module defines all events that flow from the core to frontends.
//! Frontends implement FrontendAdapter to receive these events.

use serde::Serialize;

use crate::playback::{PlaybackState, PlaybackStatus, QueueState, QueueTrack};
use crate::types::{Playlist, SearchResults, UserSession};

/// All events emitted by QBZ core to frontends
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum CoreEvent {
    // ============ Playback Events ============
    /// Track started playing
    TrackStarted {
        track: QueueTrack,
        position_secs: u64,
    },

    /// Playback state changed (play/pause/stop)
    PlaybackStateChanged { state: PlaybackState },

    /// Playback position updated (periodic, e.g., every second)
    PositionUpdated {
        position_secs: u64,
        duration_secs: u64,
    },

    /// Track finished playing naturally
    TrackEnded { track_id: u64 },

    /// Volume changed
    VolumeChanged { volume: f32 },

    /// Full playback status update
    PlaybackStatusUpdated { status: PlaybackStatus },

    // ============ Queue Events ============
    /// Queue state changed (tracks added/removed/reordered)
    QueueUpdated { state: QueueState },

    /// Shuffle mode changed
    ShuffleChanged { enabled: bool },

    /// Repeat mode changed
    RepeatModeChanged { mode: crate::playback::RepeatMode },

    // ============ Qobuz Connect Events ============
    /// Qobuz Connect session lifecycle changed (headless renderer)
    QconnectSessionChanged {
        /// "off" | "connecting" | "connected" | "retrying" | "exhausted"
        state: String,
        /// The advertised renderer name, when one is set
        device_name: Option<String>,
        /// True only while a session is established ("connected")
        session_active: bool,
    },

    // ============ Authentication Events ============
    /// User logged in successfully
    LoggedIn { session: UserSession },

    /// User logged out
    LoggedOut,

    /// Session expired or became invalid
    SessionExpired,

    // ============ Library Events ============
    /// Favorites updated
    FavoritesUpdated {
        /// Type of favorite that changed: "album", "track", or "artist"
        favorite_type: String,
    },

    /// Playlist created
    PlaylistCreated { playlist: Playlist },

    /// Playlist updated (tracks added/removed)
    PlaylistUpdated { playlist_id: u64 },

    /// Playlist deleted
    PlaylistDeleted { playlist_id: u64 },

    // ============ Loading/Progress Events ============
    /// Loading started for an operation
    LoadingStarted { operation: String },

    /// Loading completed
    LoadingCompleted { operation: String },

    /// Download progress for offline content
    DownloadProgress { track_id: u64, progress_percent: u8 },

    /// Download completed
    DownloadCompleted { track_id: u64 },

    // ============ Error Events ============
    /// An error occurred
    Error {
        code: String,
        message: String,
        /// Whether error is recoverable
        recoverable: bool,
    },

    /// Playback error (track couldn't play)
    PlaybackError { track_id: u64, message: String },

    /// Network error
    NetworkError { message: String },

    // ============ Audio System Events ============
    /// Audio device changed
    AudioDeviceChanged { device_name: String },

    /// Audio backend changed
    AudioBackendChanged {
        /// Backend name: "pipewire", "alsa", "pulse"
        backend: String,
    },

    /// Audio system diagnostic info
    AudioDiagnostic { message: String },

    // ============ Search Events ============
    /// Search results received
    SearchResultsReceived {
        query: String,
        results: SearchResults,
    },

    // ============ Navigation Hints ============
    /// Suggest navigation to album
    NavigateToAlbum { album_id: String },

    /// Suggest navigation to artist
    NavigateToArtist { artist_id: u64 },

    /// Suggest navigation to playlist
    NavigateToPlaylist { playlist_id: u64 },
}
