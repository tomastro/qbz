//! ListenBrainz API models
//!
//! Types for ListenBrainz submission payloads and responses

use serde::{Deserialize, Serialize};

/// Listen type for submission
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenType {
    /// Currently playing track
    PlayingNow,
    /// Single scrobble
    Single,
}

/// A single listen submission
#[derive(Debug, Clone, Serialize)]
pub struct Listen {
    /// Unix timestamp (omit for playing_now)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listened_at: Option<i64>,
    /// Track metadata
    pub track_metadata: TrackMetadata,
}

/// Track metadata for a listen
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackMetadata {
    /// Artist name (required)
    pub artist_name: String,
    /// Track name (required)
    pub track_name: String,
    /// Release/album name (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_name: Option<String>,
    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_info: Option<AdditionalInfo>,
}

/// Additional track info for richer scrobbles
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdditionalInfo {
    /// MusicBrainz recording ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_mbid: Option<String>,
    /// MusicBrainz release ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_mbid: Option<String>,
    /// MusicBrainz artist IDs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_mbids: Option<Vec<String>>,
    /// ISRC code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    /// Track duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Track number on release
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracknumber: Option<u32>,
    /// Media player name
    pub media_player: String,
    /// Media player version
    pub media_player_version: String,
    /// Submission client name
    pub submission_client: String,
    /// Submission client version
    pub submission_client_version: String,
}

impl AdditionalInfo {
    /// Create new AdditionalInfo with QBZ identifiers
    pub fn new() -> Self {
        Self {
            recording_mbid: None,
            release_mbid: None,
            artist_mbids: None,
            isrc: None,
            duration_ms: None,
            tracknumber: None,
            media_player: "QBZ".to_string(),
            media_player_version: "1.0.0".to_string(),
            submission_client: "QBZ".to_string(),
            submission_client_version: "1.0.0".to_string(),
        }
    }

    /// Set version info (call this from your app with actual version)
    pub fn with_version(mut self, version: &str) -> Self {
        self.media_player_version = version.to_string();
        self.submission_client_version = version.to_string();
        self
    }
}

impl Default for AdditionalInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Payload for submitting listens
#[derive(Debug, Clone, Serialize)]
pub struct SubmitListensPayload {
    pub listen_type: ListenType,
    pub payload: Vec<Listen>,
}

/// User info response
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserInfo {
    pub user_name: String,
}

/// Token validation response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct TokenValidationResponse {
    pub code: i32,
    pub message: String,
    pub valid: bool,
    #[serde(default)]
    pub user_name: Option<String>,
}

/// ListenBrainz connection status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenBrainzStatus {
    pub connected: bool,
    pub user_name: Option<String>,
    pub enabled: bool,
}

/// Queued listen for offline submission
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedListen {
    pub id: i64,
    pub listened_at: i64,
    pub artist_name: String,
    pub track_name: String,
    pub release_name: Option<String>,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub artist_mbids: Option<Vec<String>>,
    pub isrc: Option<String>,
    pub duration_ms: Option<u64>,
    pub created_at: i64,
    pub attempts: i32,
    pub sent: bool,
}

/// Collaborative-filtering recommendation entry
///
/// Returned by `GET /cf/recommendation/user/{user_name}/recording`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfRecommendation {
    /// MusicBrainz recording ID of the recommended track
    pub recording_mbid: String,
    /// Recommendation score (higher = stronger match)
    pub score: f64,
    /// ISO-8601 timestamp of the user's last listen to this recording,
    /// or `None` if never listened (null/absent in the API)
    pub latest_listened_at: Option<String>,
}

/// A single listen from a user's scrobble history
///
/// Returned by `GET /user/{user_name}/listens`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LbListen {
    /// Unix timestamp when the track was listened to
    pub listened_at: i64,
    /// Artist name
    pub artist_name: String,
    /// Track name
    pub track_name: String,
    /// MusicBrainz recording ID (from `mbid_mapping`), if the listen was mapped
    pub recording_mbid: Option<String>,
}

/// Hydrated recording metadata
///
/// Returned by `GET /metadata/recording/`. Used to enrich CF recommendation
/// MBIDs with human-readable names, artist IDs, and cover art references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LbRecordingMeta {
    /// MusicBrainz recording ID (the response object key)
    pub recording_mbid: String,
    /// Recording (track) name
    pub recording_name: String,
    /// Primary artist credit name
    pub artist_name: String,
    /// MusicBrainz artist IDs that make up the credit
    pub artist_mbids: Vec<String>,
    /// Release/album name, if available
    pub release_name: Option<String>,
    /// Cover Art Archive image ID, if available
    pub caa_id: Option<i64>,
    /// Cover Art Archive release MBID, if available
    pub caa_release_mbid: Option<String>,
}

/// A "Created for you" curated playlist's metadata
///
/// Returned by `GET /user/{user_name}/playlists/createdfor`. Describes one of
/// the auto-generated playlists (Weekly Jams, Weekly Exploration, Top
/// Discoveries, etc.) without its tracks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LbPlaylistMeta {
    /// MusicBrainz playlist ID (last path segment of the JSPF `identifier`)
    pub playlist_mbid: String,
    /// Playlist title
    pub title: String,
    /// Generation algorithm patch name (e.g. `weekly-jams`, `weekly-exploration`)
    pub source_patch: Option<String>,
    /// Playlist annotation / description, if available
    pub annotation: Option<String>,
    /// ISO-8601 creation timestamp, if available
    pub created_at: Option<String>,
}

/// A single track inside a curated playlist (JSPF entry)
///
/// Returned by `GET /playlist/{playlist_mbid}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LbPlaylistTrack {
    /// MusicBrainz recording ID (last path segment of the JSPF `identifier`)
    pub recording_mbid: Option<String>,
    /// Track title
    pub title: String,
    /// Artist name (JSPF `creator`)
    pub artist_name: String,
    /// Release/album name (JSPF `album`), if available
    pub release_name: Option<String>,
    /// Cover Art Archive image ID, if available
    pub caa_id: Option<i64>,
    /// Cover Art Archive release MBID, if available
    pub caa_release_mbid: Option<String>,
}

/// A personalized fresh release entry
///
/// Returned by `GET /user/{user_name}/fresh_releases`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LbFreshRelease {
    /// Release/album name
    pub release_name: String,
    /// Artist credit name
    pub artist_credit_name: String,
    /// MusicBrainz release ID, if available
    pub release_mbid: Option<String>,
    /// MusicBrainz release-group ID, if available
    pub release_group_mbid: Option<String>,
    /// Release-group primary type (Album, Single, EP, ...), if available
    pub primary_type: Option<String>,
    /// Cover Art Archive image ID, if available
    pub caa_id: Option<i64>,
    /// Cover Art Archive release MBID, if available
    pub caa_release_mbid: Option<String>,
    /// Release date (ISO-8601), if available
    pub release_date: Option<String>,
    /// Number of listens recorded for this release, if available
    pub listen_count: Option<u64>,
}
