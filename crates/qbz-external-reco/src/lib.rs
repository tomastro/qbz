//! Frontend-agnostic external-recommendations engine for Discover (ADR-006).
//!
//! Blends Last.fm + ListenBrainz into the Discover "Recommendations" tab,
//! validating every candidate against the Qobuz catalog before display (the app
//! can only play Qobuz content). Lineup (owner-directed 2026-06-27):
//!   - Recommended Artists (Last.fm similar of your recent top, not heard).
//!   - Recommended Albums (Last.fm artist top-albums, not scrobbled).
//!   - Fresh Releases (ListenBrainz, from artists you follow).
//!   - Weekly Exploration / Weekly Jams (ListenBrainz curated playlists).
//!   - Deep-cut albums from artists you know.
//!   - Cold-start fallback: Qobuz editorial top albums + artists.
//!
//! The per-row builders are public so the frontend can paint each row the moment
//! it resolves (progressive load). The "heard" filters compare against the LOCAL
//! reco-store history + a light Last.fm/LB top-set (not a full history sweep).

pub mod cache;
pub mod matching;
pub mod types;

mod carousels;
mod validate;

use std::sync::Mutex;

use qbz_integrations::{LastFmClient, ListenBrainzClient, MusicBrainzClient};
use qbz_models::{Album, Artist, Track};

pub use cache::RecoCache;
pub use carousels::{compose_artist_rails, ArtistRailComposition, ARTIST_DISPLAY_CAP};
// Shared catalog text rules. The Library release-replacement picker needs the
// same bracket/edition stripping and similarity semantics as recommendation
// validation; exporting the two pure helpers prevents a third drifting copy.
pub use matching::{normalize as normalize_catalog_name, similarity as catalog_similarity};
// `validate` stays a private module — only this one helper is public, because
// the unavailable-track replacement flow (qbz-qt) needs the ISRC -> streamable
// -track lookup and nothing else in it.
pub use types::{
    AlbumReco, ArtistReco, ExtHistory, ExternalCarousels, LocalHistory, RecoSource, TrackReco,
};
pub use validate::find_by_isrc;

/// The Qobuz catalog operations the engine needs. Implemented by the frontend
/// over its own `QbzCore`. Every method swallows errors to an empty result.
#[async_trait::async_trait]
pub trait RecoCatalog: Send + Sync {
    async fn search_tracks(&self, query: &str, limit: usize) -> Vec<Track>;
    async fn search_artists(&self, query: &str, limit: usize) -> Vec<Artist>;
    /// Free-text Qobuz album search (for validating a recommended album).
    async fn search_albums(&self, query: &str, limit: usize) -> Vec<Album>;
    async fn artist_top_tracks(&self, artist_id: u64, limit: usize) -> Vec<Track>;
    /// An artist's albums (the deep-cut candidate source).
    async fn artist_albums(&self, artist_id: u64, limit: usize) -> Vec<Album>;
    /// Editorial featured albums by kind ("most-streamed" | "new-releases" | …).
    async fn featured_albums(&self, kind: &str, limit: usize) -> Vec<Album>;
    async fn get_artist(&self, artist_id: u64) -> Option<Artist>;
}

pub struct LastFmHandle<'a> {
    pub username: String,
    pub client: &'a LastFmClient,
}

pub struct ListenBrainzHandle<'a> {
    pub username: String,
    pub client: &'a ListenBrainzClient,
}

pub struct RecoInputs<'a> {
    pub lastfm: Option<LastFmHandle<'a>>,
    pub listenbrainz: Option<ListenBrainzHandle<'a>>,
    pub musicbrainz: &'a MusicBrainzClient,
    pub catalog: &'a dyn RecoCatalog,
    pub cache: Option<&'a Mutex<RecoCache>>,
    pub local: LocalHistory,
    /// Daily rotation offset (e.g. days since the Unix epoch).
    pub rotation_seed: u64,
}

impl RecoInputs<'_> {
    /// Whether any external source is connected.
    pub fn has_external(&self) -> bool {
        self.lastfm.is_some() || self.listenbrainz.is_some()
    }
}

/// True when no external source is connected -> editorial fallback regime.
pub fn is_cold_start(inputs: &RecoInputs<'_>) -> bool {
    !inputs.has_external()
}

/// Gather the external "heard" history ONCE (shared across all row builders).
pub async fn gather_history(inputs: &RecoInputs<'_>) -> ExtHistory {
    carousels::gather_history(inputs).await
}

// ── Per-row builders (progressive: the frontend paints each as it resolves) ──

pub async fn build_rec_artists_common(
    inputs: &RecoInputs<'_>,
    history: &ExtHistory,
) -> Vec<ArtistReco> {
    carousels::build_rec_artists_common(inputs, history).await
}
pub async fn build_rec_artists_recent(
    inputs: &RecoInputs<'_>,
    history: &ExtHistory,
) -> Vec<ArtistReco> {
    carousels::build_rec_artists_recent(inputs, history).await
}
pub async fn build_rec_albums(inputs: &RecoInputs<'_>, history: &ExtHistory) -> Vec<AlbumReco> {
    carousels::build_rec_albums(inputs, history).await
}
pub async fn build_fresh_releases(inputs: &RecoInputs<'_>) -> Vec<AlbumReco> {
    carousels::build_fresh_releases(inputs).await
}
pub async fn build_weekly_exploration(inputs: &RecoInputs<'_>) -> Vec<TrackReco> {
    carousels::build_weekly(inputs, "weekly-exploration").await
}
pub async fn build_weekly_jams(inputs: &RecoInputs<'_>) -> Vec<TrackReco> {
    carousels::build_weekly(inputs, "weekly-jams").await
}
pub async fn build_deep_cut_albums(inputs: &RecoInputs<'_>) -> Vec<AlbumReco> {
    carousels::build_deep_cut_albums(inputs).await
}
/// Album page: albums similar to a seed album, derived from its primary
/// artist's Last.fm similar artists (one top album each). `exclude_pairs` are
/// the (artist, title) already shown by the Qobuz suggestions row.
pub async fn build_similar_albums_seeded(
    inputs: &RecoInputs<'_>,
    seed_artist: &str,
    exclude_pairs: &[(String, String)],
) -> Vec<AlbumReco> {
    carousels::build_similar_albums_seeded(inputs, seed_artist, exclude_pairs).await
}
/// Cold-start editorial (top albums + artists).
pub async fn build_editorial(inputs: &RecoInputs<'_>) -> (Vec<AlbumReco>, Vec<ArtistReco>) {
    carousels::build_editorial(inputs).await
}

/// Convenience: build the whole set at once (non-progressive callers / tests).
pub async fn build_external_carousels(inputs: RecoInputs<'_>) -> ExternalCarousels {
    if is_cold_start(&inputs) {
        let (top_albums, top_artists) = build_editorial(&inputs).await;
        return ExternalCarousels {
            editorial_fallback: true,
            top_albums,
            top_artists,
            ..Default::default()
        };
    }
    let history = gather_history(&inputs).await;
    let (
        rec_artists_common,
        rec_artists_recent,
        rec_albums,
        fresh_releases,
        weekly_exploration,
        weekly_jams,
        deep_cut_albums,
    ) = tokio::join!(
        build_rec_artists_common(&inputs, &history),
        build_rec_artists_recent(&inputs, &history),
        build_rec_albums(&inputs, &history),
        build_fresh_releases(&inputs),
        build_weekly_exploration(&inputs),
        build_weekly_jams(&inputs),
        build_deep_cut_albums(&inputs),
    );
    ExternalCarousels {
        editorial_fallback: false,
        rec_artists_common,
        rec_artists_recent,
        rec_albums,
        fresh_releases,
        weekly_exploration,
        weekly_jams,
        deep_cut_albums,
        top_albums: Vec::new(),
        top_artists: Vec::new(),
    }
}
