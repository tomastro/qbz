//! Data models for local library

use serde::{Deserialize, Serialize};

/// Supported audio formats
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AudioFormat {
    Flac,
    Alac,
    Wav,
    Aiff,
    Ape,
    Mp3,
    Dsd,
    Unknown,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self::Unknown
    }
}

impl std::fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioFormat::Flac => write!(f, "FLAC"),
            AudioFormat::Alac => write!(f, "ALAC"),
            AudioFormat::Wav => write!(f, "WAV"),
            AudioFormat::Aiff => write!(f, "AIFF"),
            AudioFormat::Ape => write!(f, "APE"),
            AudioFormat::Mp3 => write!(f, "MP3"),
            AudioFormat::Dsd => write!(f, "DSD"),
            AudioFormat::Unknown => write!(f, "Unknown"),
        }
    }
}

/// A track from the local library
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalTrack {
    pub id: i64,
    pub file_path: String,

    // Metadata
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: Option<String>,
    pub album_group_key: String,
    pub album_group_title: String,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    /// Complete ordered genre set. `genre` remains the compatibility primary
    /// value for old databases and single-value tag writers.
    #[serde(default)]
    pub genres: Vec<String>,
    pub catalog_number: Option<String>,

    // Audio properties
    pub duration_secs: u64,
    pub format: AudioFormat,
    pub bit_depth: Option<u32>,
    pub sample_rate: f64, // Changed from u32 to f64 to support fractional rates (44.1kHz = 44100Hz)
    pub channels: u8,
    pub file_size_bytes: u64,

    // CUE support
    pub cue_file_path: Option<String>,
    pub cue_start_secs: Option<f64>,
    pub cue_end_secs: Option<f64>,

    // Artwork
    pub artwork_path: Option<String>,
    /// Album/collection artwork kept apart from a track or disc cover.
    /// Consumers resolve artwork from the most specific layer outward.
    #[serde(default)]
    pub collection_artwork_path: Option<String>,

    // Indexing
    pub last_modified: i64,
    pub indexed_at: i64,

    // Download tracking
    pub source: Option<String>,
    pub qobuz_track_id: Option<i64>,

    /// True when the file lives on a network-backed filesystem (NFS,
    /// CIFS/SMB, SSHFS, etc.). Detected at index time by inspecting
    /// /proc/mounts. Consumed by the UI to mark the track as
    /// unreachable under forced offline mode — cable unplugged means
    /// the mount is gone even if the path string still says /home/…,
    /// which is common under Flatpak / Snap sandboxes.
    #[serde(default)]
    pub is_network_mount: bool,

    // Cross-source identity, straight from the file's tags (Picard writes
    // them; lofty reads them). `None` when the file is untagged. The
    // recording id + ISRC are the JOIN keys the listen log stamps so a play
    // of a local file and a play of the same recording on Qobuz can meet.
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub musicbrainz_recording_id: Option<String>,
    #[serde(default)]
    pub musicbrainz_track_id: Option<String>,
    #[serde(default)]
    pub musicbrainz_release_id: Option<String>,
    #[serde(default)]
    pub musicbrainz_release_group_id: Option<String>,
    #[serde(default)]
    pub musicbrainz_artist_id: Option<String>,
}

impl Default for LocalTrack {
    fn default() -> Self {
        Self {
            id: 0,
            file_path: String::new(),
            title: String::new(),
            artist: "Unknown Artist".to_string(),
            album: "Unknown Album".to_string(),
            album_artist: None,
            album_group_key: String::new(),
            album_group_title: String::new(),
            track_number: None,
            disc_number: None,
            year: None,
            genre: None,
            genres: Vec::new(),
            catalog_number: None,
            duration_secs: 0,
            format: AudioFormat::Unknown,
            bit_depth: None,
            sample_rate: 44100.0, // Now f64
            channels: 2,
            file_size_bytes: 0,
            cue_file_path: None,
            cue_start_secs: None,
            cue_end_secs: None,
            artwork_path: None,
            collection_artwork_path: None,
            last_modified: 0,
            indexed_at: 0,
            source: None,
            qobuz_track_id: None,
            isrc: None,
            musicbrainz_recording_id: None,
            musicbrainz_track_id: None,
            musicbrainz_release_id: None,
            musicbrainz_release_group_id: None,
            musicbrainz_artist_id: None,
            is_network_mount: false,
        }
    }
}

/// A local track within a playlist, including its position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistLocalTrack {
    #[serde(flatten)]
    pub track: LocalTrack,
    /// Position in the combined playlist (Qobuz + local tracks)
    pub playlist_position: i32,
}

/// One page of metadata-grouped local albums plus the total count of
/// albums matching the same filter (for scrollbar pre-allocation on the
/// frontend). Returned by `Database::get_albums_metadata_page`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumsMetadataPage {
    pub albums: Vec<LocalAlbum>,
    pub total: u64,
}

/// An album aggregated from local tracks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAlbum {
    pub id: String,
    pub title: String,
    pub artist: String,
    /// All contributing artists (comma-separated) - used for matching in artist view
    /// This includes all unique album_artist/artist values from the album's tracks
    #[serde(default)]
    pub all_artists: String,
    pub year: Option<u32>,
    pub catalog_number: Option<String>,
    /// Union of every track/version genre represented by this album.
    #[serde(default)]
    pub genres: Vec<String>,
    pub artwork_path: Option<String>,
    /// Authoritative source that owns `artwork_path` when an album row is a
    /// frontend-side logical group. It may differ from `source`: the
    /// best-audio copy can be coverless while another physical copy supplies
    /// the group's representative artwork.
    #[serde(default)]
    pub artwork_source: Option<String>,
    pub track_count: u32,
    pub total_duration_secs: u64,
    pub format: AudioFormat,
    pub bit_depth: Option<u32>,
    pub sample_rate: f64, // Changed from u32 to f64 for decimal precision
    pub directory_path: String,
    /// Comma-separated list of distinct folder keys that contributed
    /// tracks to this album. Populated only by the metadata-grouped
    /// Albums query (`get_albums_metadata_grouped`); `None` for folder-
    /// grouped rows. The frontend uses this to render a tooltip when N
    /// folders > 1.
    #[serde(default)]
    pub source_folders: Option<String>,
    /// Source of the album: "user" for local files, "qobuz_download" for offline cached
    #[serde(default = "default_source")]
    pub source: String,
    /// Distinct authoritative sources contributing physical copies to this
    /// logical row. A legacy row has one entry; the Qt aggregation may fold
    /// several strongly-associated copies into one card without losing their
    /// provenance.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Content evidence used to associate copies conservatively across
    /// sources. This is derived from authoritative rows on every read and is
    /// never persisted as product identity.
    #[serde(default)]
    pub identity_tracks: Vec<AlbumTrackEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlbumTrackEvidence {
    pub title: String,
    pub duration_secs: u64,
}

fn default_source() -> String {
    "user".to_string()
}

/// An artist aggregated from local tracks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalArtist {
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
}

/// Scan progress for UI updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
    pub status: ScanStatus,
    pub total_files: u32,
    pub processed_files: u32,
    pub current_file: Option<String>,
    pub errors: Vec<ScanError>,
}

impl Default for ScanProgress {
    fn default() -> Self {
        Self {
            status: ScanStatus::Idle,
            total_files: 0,
            processed_files: 0,
            current_file: None,
            errors: Vec::new(),
        }
    }
}

/// Scan status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScanStatus {
    Idle,
    Scanning,
    Complete,
    Cancelled,
    Error,
}

/// A scan error for a specific file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanError {
    pub file_path: String,
    pub error: String,
}

/// Audio properties extracted from a file
#[derive(Debug, Clone, Default)]
pub struct AudioProperties {
    pub duration_secs: u64,
    pub bit_depth: Option<u32>,
    pub sample_rate: f64, // Changed from u32 to f64 for decimal precision
    pub channels: u8,
}

/// Album settings for local library albums
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumSettings {
    pub album_group_key: String,
    pub hidden: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl AlbumSettings {
    pub fn new(album_group_key: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        Self {
            album_group_key,
            hidden: false,
            created_at: now,
            updated_at: now,
        }
    }
}

/// A child entry within a folder of the local-library filesystem
/// hierarchy. Used by the Folders-tab tree view to render one level at
/// a time without preloading the entire tree.
///
/// The `kind` tag is serialised as `snake_case` so the frontend can
/// discriminate via `entry.kind === 'folder' | 'track'`.
///
/// `path` is the absolute filesystem path of the entry. `segment` is
/// the last path component for display. For folder rows,
/// `track_count_under` is the recursive count of `local_tracks`
/// matching `file_path LIKE path || '/%'`. `artwork` is an optional
/// thumbnail path lifted from any track in the subtree (best-effort,
/// not guaranteed to be the album cover).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FolderTreeEntry {
    Folder {
        path: String,
        segment: String,
        track_count_under: u32,
        artwork: Option<String>,
    },
    Track {
        path: String,
        segment: String,
    },
}

/// Information about an artist's image
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistImageInfo {
    pub artist_name: String,
    pub image_url: Option<String>,
    pub source: Option<String>,
    pub custom_image_path: Option<String>,
    pub canonical_name: Option<String>,
}
