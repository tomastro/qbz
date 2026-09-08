//! Last.fm data models

use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize integer (0/1) as boolean - Last.fm API returns subscriber as number
fn deserialize_int_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value: serde_json::Value = Deserialize::deserialize(deserializer)?;
    match value {
        serde_json::Value::Bool(b) => Ok(b),
        serde_json::Value::Number(n) => Ok(n.as_i64().unwrap_or(0) != 0),
        _ => Ok(false),
    }
}

/// Last.fm session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastFmSession {
    /// Username on Last.fm
    pub name: String,
    /// Session key for API calls
    pub key: String,
    /// Whether user is a subscriber
    #[serde(deserialize_with = "deserialize_int_bool")]
    pub subscriber: bool,
}

/// Response from auth.getSession
#[derive(Debug, Deserialize)]
pub(crate) struct AuthGetSessionResponse {
    pub session: LastFmSession,
}

/// Response from auth.getToken
#[derive(Debug, Deserialize)]
pub(crate) struct AuthGetTokenResponse {
    pub token: String,
    #[serde(rename = "authUrl")]
    pub auth_url: Option<String>,
}

/// Last.fm API response wrapper (success or error)
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum LastFmResponse<T> {
    Success(T),
    Error { error: u32, message: String },
}

/// A similar artist from Last.fm's artist.getSimilar
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastFmSimilarArtist {
    /// Artist name
    pub name: String,
    /// Similarity score from 0.0 to 1.0
    pub match_score: f64,
    /// MusicBrainz ID if available
    pub mbid: Option<String>,
}

/// A user's top artist from Last.fm's user.getTopArtists
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LastFmArtist {
    /// Artist name
    pub name: String,
    /// MusicBrainz ID if available
    pub mbid: Option<String>,
    /// Number of times the user has played this artist
    pub playcount: u64,
    /// Largest available image URL, if any
    pub image: Option<String>,
}

/// An album from Last.fm's artist.getTopAlbums / user.getTopAlbums
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LastFmAlbum {
    /// Album name
    pub name: String,
    /// Artist name
    pub artist: String,
    /// Artist MusicBrainz ID if available
    pub artist_mbid: Option<String>,
    /// Album MusicBrainz ID if available
    pub mbid: Option<String>,
    /// Largest available image URL, if any
    pub image: Option<String>,
    /// Playcount (global for artist.getTopAlbums, the user's for user.getTopAlbums)
    pub playcount: u64,
}

/// A track from Last.fm user methods (top / loved / recent tracks)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LastFmTrack {
    /// Track name
    pub name: String,
    /// Artist name
    pub artist: String,
    /// Artist MusicBrainz ID if available
    pub artist_mbid: Option<String>,
    /// Track MusicBrainz ID if available
    pub mbid: Option<String>,
    /// Album name if available
    pub album: Option<String>,
    /// Largest available image URL, if any
    pub image: Option<String>,
    /// Unix timestamp of the scrobble / love, if available
    pub uts: Option<i64>,
}

/// A similar track from Last.fm's track.getSimilar
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LastFmSimilarTrack {
    /// Track name
    pub name: String,
    /// Artist name
    pub artist: String,
    /// MusicBrainz ID if available
    pub mbid: Option<String>,
    /// Raw match weight (NOT normalized to 0..1)
    pub match_score: f64,
}
