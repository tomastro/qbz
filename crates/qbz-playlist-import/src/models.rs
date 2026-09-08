//! Data models for playlist import

use serde::{Deserialize, Serialize};

/// Where a playlist came FROM, as a CLASS — never a format.
///
/// The four streaming variants are the original set. The four added by the
/// 2.0.3 expansion are per-class on purpose (design §4.3): `provider_display_name`
/// returns `&'static str` and cannot hold an owned string, so the IDENTITY of a
/// source — the filename, the ListenBrainz playlist title, the Last.fm
/// username — belongs in `ImportPlaylist.name` / `.provider_id`, which are
/// free-form. "Imported from Playlist file" plus the filename in `.name` is the
/// right split; `M3u`/`Pls`/`Xspf` variants would bloat two exhaustive matches
/// for no UX gain.
///
/// JSON stays separate from File because the user is told a different thing
/// about it — it is read best-effort and the track count is the gate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImportProvider {
    Spotify,
    AppleMusic,
    Tidal,
    Deezer,
    /// XSPF / PLS / M3U / M3U8.
    File,
    /// Best-effort JSON.
    Json,
    ListenBrainz,
    LastFm,
}

impl ImportProvider {
    /// The wire/log form. `importer.rs`'s `format!("Imported from {}", …)`
    /// default description reads this, so a new variant needs nothing there.
    pub fn as_str(&self) -> &'static str {
        match self {
            ImportProvider::Spotify => "spotify",
            ImportProvider::AppleMusic => "apple_music",
            ImportProvider::Tidal => "tidal",
            ImportProvider::Deezer => "deezer",
            ImportProvider::File => "file",
            ImportProvider::Json => "json",
            ImportProvider::ListenBrainz => "listenbrainz",
            ImportProvider::LastFm => "lastfm",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub isrc: Option<String>,
    pub provider_id: Option<String>,
    pub provider_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPlaylist {
    pub provider: ImportProvider,
    pub provider_id: String,
    pub name: String,
    pub description: Option<String>,
    pub tracks: Vec<ImportTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackMatch {
    pub source: ImportTrack,
    pub qobuz_track_id: Option<u64>,
    pub qobuz_title: Option<String>,
    pub qobuz_artist: Option<String>,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSummary {
    pub provider: ImportProvider,
    pub playlist_name: String,
    pub total_tracks: u32,
    /// DISTINCT Qobuz tracks actually added — the playlist's real length.
    pub matched_tracks: u32,
    /// Source rows that found no Qobuz track at all.
    ///
    /// It used to be `total - matched`, which silently counted DUPLICATES as
    /// skipped: a source row that matched a track another row had already
    /// matched is deduplicated before the add, so it is neither in
    /// `matched_tracks` nor a failure. One real Last.fm import reported
    /// "Skipped: 271" when 255 of those had matched perfectly well and only 16
    /// had missed. Now the three numbers are disjoint and sum to
    /// `total_tracks`.
    pub skipped_tracks: u32,
    /// Source rows that matched a track ALREADY contributed by an earlier row.
    pub duplicate_tracks: u32,
    pub qobuz_playlist_ids: Vec<u64>,
    pub parts_created: u32,
    pub matches: Vec<TrackMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportProgress {
    pub phase: String,
    pub current: u32,
    pub total: u32,
    pub matched_so_far: u32,
    pub current_track: Option<String>,
}
