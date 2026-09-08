//! Data types for the external-recommendations engine.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Which source produced a recommendation row item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoSource {
    /// In-house artist-vector engine (`qbz-reco`) — deferred, placeholder.
    Internal,
    LastFm,
    ListenBrainz,
    /// Qobuz editorial (cold-start fallback).
    Editorial,
}

// ── Raw candidates (pre Qobuz validation) ──────────────────────────────────

#[derive(Debug, Clone)]
pub struct ArtistCandidate {
    pub name: String,
    pub source: RecoSource,
    pub score: f32,
    /// "Similar to X, Y, Z" line, built from the seeds that surfaced this artist.
    pub subtitle: String,
}

#[derive(Debug, Clone)]
pub struct AlbumCandidate {
    pub artist: String,
    pub title: String,
    pub upc: Option<String>,
    pub source: RecoSource,
    pub score: f32,
    /// "Similar to …" / "You've scrobbled {artist} before" line.
    pub subtitle: String,
}

#[derive(Debug, Clone)]
pub struct TrackCandidate {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub isrc: Option<String>,
    pub recording_mbid: Option<String>,
    pub source: RecoSource,
    pub score: f32,
}

// ── Resolved rows (validated to Qobuz) ─────────────────────────────────────

/// A resolved artist row (validated to a Qobuz artist).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistReco {
    pub qobuz_artist_id: u64,
    pub name: String,
    pub image_url: String,
    /// "Similar to X, Y, Z".
    #[serde(default)]
    pub subtitle: String,
    pub source: RecoSource,
}

/// A resolved album row (validated to a Qobuz album).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumReco {
    pub qobuz_album_id: String,
    pub title: String,
    pub artist: String,
    pub artist_id: String,
    pub year: String,
    pub quality_tier: String,
    pub quality_label: String,
    pub artwork_url: String,
    #[serde(default)]
    pub subtitle: String,
    /// Genre name from the resolving Qobuz album — feeds the card hover
    /// overlay. Defaulted for backward compatibility with cached JSON
    /// written before the field existed.
    #[serde(default)]
    pub genre: String,
    #[serde(default = "default_source")]
    pub source: RecoSource,
}

fn default_source() -> RecoSource {
    RecoSource::Editorial
}

/// A resolved track row (validated to / sourced from a Qobuz track).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackReco {
    pub qobuz_track_id: u64,
    pub title: String,
    pub artist: String,
    pub artwork_url: String,
    pub source: RecoSource,
}

/// The full external-recommendations result for the Discover section.
///
/// Empty vecs self-hide their row in the view, so partial population is always
/// safe — the controller paints each row independently as it resolves.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalCarousels {
    /// No connected external source -> editorial fallback regime.
    pub editorial_fallback: bool,
    /// Recommended artists from your COMMON taste (overall top -> similar, not heard).
    pub rec_artists_common: Vec<ArtistReco>,
    /// Recommended artists from your RECENT taste (1-month top -> similar, not heard).
    pub rec_artists_recent: Vec<ArtistReco>,
    /// Recommended albums (Last.fm artist top-albums, not scrobbled).
    pub rec_albums: Vec<AlbumReco>,
    /// Fresh releases (ListenBrainz, from artists you follow).
    pub fresh_releases: Vec<AlbumReco>,
    /// Weekly Exploration (ListenBrainz discovery playlist) tracks.
    pub weekly_exploration: Vec<TrackReco>,
    /// Weekly Jams (ListenBrainz familiar playlist) tracks.
    pub weekly_jams: Vec<TrackReco>,
    /// Deep-cut albums from artists you know.
    pub deep_cut_albums: Vec<AlbumReco>,
    /// Cold-start fallback: Qobuz editorial top albums.
    pub top_albums: Vec<AlbumReco>,
    /// Cold-start fallback: Qobuz editorial top artists.
    pub top_artists: Vec<ArtistReco>,
}

/// Local listening signal from the per-user `reco_events` store (Qobuz ids).
#[derive(Debug, Clone, Default)]
pub struct LocalHistory {
    /// Artists the user already knows (played > threshold or favorited).
    pub known_artist_ids: HashSet<u64>,
    /// Tracks played in-app.
    pub played_track_ids: HashSet<u64>,
    /// Albums played in-app (the local "already heard albums" set).
    pub played_album_ids: HashSet<String>,
}

/// External listening signal (normalized) for the "not heard / not scrobbled"
/// filters. Gathered ONCE per build and shared across the per-row builders.
#[derive(Debug, Clone, Default)]
pub struct ExtHistory {
    /// Normalized artist names the user has listened to (Last.fm + LB).
    pub artist_names: HashSet<String>,
    /// Normalized "artist|title" track keys (scrobbled set).
    pub track_keys: HashSet<String>,
    /// Normalized "artist|album" keys (scrobbled-album set).
    pub album_keys: HashSet<String>,
}
