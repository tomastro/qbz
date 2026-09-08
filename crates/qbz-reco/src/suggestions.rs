//! Playlist suggestions engine
//!
//! Uses artist vectors to suggest new tracks for a playlist.
//! Algorithm:
//! 1. Extract unique artists from playlist tracks
//! 2. Compute combined playlist vector (sum + normalize)
//! 3. Find nearest artists not already in playlist
//! 4. Search Qobuz for top tracks by those artists
//! 5. Return suggested tracks with optional reasons
//!
//! The `Arc<tokio::Mutex/RwLock>` ownership from the original Tauri engine is
//! kept because the store/cache contain `!Sync` rusqlite connections. Step 3
//! ranks candidates by summed relationship weight via
//! `store.get_all_related_artists`, NOT cosine similarity (epic decision D3).
//! Step 2 remains only as the empty-vector gate, while Qobuz identity resolution
//! is ID/cache-first and validated against context collected from playlist seeds.

use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use serde::{Deserialize, Serialize};

use qbz_models::Track;
use qbz_qobuz::QobuzClient;

use crate::artist_guardrail::{
    normalize_name, resolve_candidate, resolve_seed_context, validate_candidate, ArtistLookup,
    ProductionArtistLookup, SeedContext,
};
use crate::builder::ArtistVectorBuilder;
use crate::sparse_vector::SparseVector;
use crate::store::ArtistVectorStore;

/// Configuration for suggestion generation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestionConfig {
    /// Maximum number of artists to consider for suggestions
    pub max_artists: usize,
    /// Number of tracks to fetch per artist
    pub tracks_per_artist: usize,
    /// Maximum total tracks in the suggestion pool
    pub max_pool_size: usize,
    /// Maximum age (days) for vector freshness
    pub vector_max_age_days: i64,
    /// Minimum similarity score to include artist
    pub min_similarity: f32,
    /// Skip building vectors - only use existing cached vectors (faster but may have fewer results)
    pub skip_vector_build: bool,
}

impl Default for SuggestionConfig {
    fn default() -> Self {
        Self {
            max_artists: 30,      // Increased from 20 for more variety
            tracks_per_artist: 6, // Increased from 5
            max_pool_size: 150,   // Increased from 100
            vector_max_age_days: 7,
            min_similarity: 0.1,
            skip_vector_build: false,
        }
    }
}

/// A suggested track with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedTrack {
    /// Qobuz track ID
    pub track_id: u64,
    /// Track title
    pub title: String,
    /// Artist name
    pub artist_name: String,
    /// Artist Qobuz ID (for navigation)
    pub artist_id: Option<u64>,
    /// Artist MBID (if known)
    pub artist_mbid: Option<String>,
    /// Album title
    pub album_title: String,
    /// Album ID for cover art
    pub album_id: String,
    /// Direct URL to album cover image
    pub album_image_url: Option<String>,
    /// Duration in seconds
    pub duration: u32,
    /// Similarity score (higher = more similar to playlist)
    pub similarity_score: f32,
    /// Reason for suggestion (for dev mode)
    pub reason: Option<String>,
}

/// Result of suggestion generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionResult {
    /// Pool of suggested tracks
    pub tracks: Vec<SuggestedTrack>,
    /// Artists that contributed to suggestions
    pub source_artists: Vec<String>,
    /// Number of playlist artists analyzed
    pub playlist_artists_count: usize,
    /// Number of similar artists found
    pub similar_artists_count: usize,
}

/// Playlist suggestions engine
pub struct SuggestionsEngine {
    /// Vector store for similarity lookups
    store: Arc<Mutex<Option<ArtistVectorStore>>>,
    /// Vector builder for lazy construction
    builder: Arc<ArtistVectorBuilder>,
    /// Qobuz client for track search
    qobuz_client: Arc<RwLock<Option<QobuzClient>>>,
    /// Identity-first artist lookup, replaceable with an in-memory fake in tests.
    artist_lookup: Arc<dyn ArtistLookup>,
    /// Configuration
    config: SuggestionConfig,
}

impl SuggestionsEngine {
    /// Create a new suggestions engine
    pub fn new(
        store: Arc<Mutex<Option<ArtistVectorStore>>>,
        builder: Arc<ArtistVectorBuilder>,
        qobuz_client: Arc<RwLock<Option<QobuzClient>>>,
        config: SuggestionConfig,
    ) -> Self {
        let artist_lookup = Arc::new(ProductionArtistLookup::new(
            qobuz_client.clone(),
            builder.musicbrainz_cache(),
            builder.musicbrainz_client(),
        ));
        Self {
            store,
            builder,
            qobuz_client,
            artist_lookup,
            config,
        }
    }

    /// Replace production artist lookups, primarily for deterministic tests.
    pub fn with_artist_lookup(mut self, artist_lookup: Arc<dyn ArtistLookup>) -> Self {
        self.artist_lookup = artist_lookup;
        self
    }

    /// Generate suggestions for a playlist
    ///
    /// # Arguments
    /// * `playlist_artists` - Artist info (MBID, name) from the playlist
    /// * `exclude_track_ids` - Track IDs to exclude (already in playlist)
    /// * `include_reasons` - Whether to include reason strings (dev mode)
    pub async fn generate_suggestions(
        &self,
        playlist_artists: &[(String, String)], // (mbid, name)
        exclude_track_ids: &HashSet<u64>,
        include_reasons: bool,
    ) -> Result<SuggestionResult, String> {
        use std::time::Instant;

        if playlist_artists.is_empty() {
            log::debug!("[SuggestionsEngine] Empty playlist, returning empty");
            return Ok(SuggestionResult {
                tracks: Vec::new(),
                source_artists: Vec::new(),
                playlist_artists_count: 0,
                similar_artists_count: 0,
            });
        }

        // Extract MBIDs for vector operations
        let playlist_artist_mbids: Vec<String> = playlist_artists
            .iter()
            .map(|(mbid, _)| mbid.clone())
            .collect();

        // 1. Ensure vectors exist for playlist artists (skip if configured)
        let step1_start = Instant::now();
        if self.config.skip_vector_build {
            log::debug!("[SuggestionsEngine] Step 1: SKIPPED (skip_vector_build=true), using only cached vectors");
        } else {
            log::debug!(
                "[SuggestionsEngine] Step 1: Ensuring vectors for {} artists",
                playlist_artists.len()
            );
            for (i, (mbid, name)) in playlist_artists.iter().enumerate() {
                let artist_start = Instant::now();
                let _ = self
                    .builder
                    .ensure_vector(mbid, Some(name), None, self.config.vector_max_age_days)
                    .await;
                log::debug!(
                    "[SuggestionsEngine] ensure_vector {}/{} took {:?}",
                    i + 1,
                    playlist_artists.len(),
                    artist_start.elapsed()
                );
            }
            log::debug!(
                "[SuggestionsEngine] Step 1 completed in {:?}",
                step1_start.elapsed()
            );
        }

        // 2. Compute combined playlist vector
        log::debug!("[SuggestionsEngine] Step 2: Computing playlist vector");
        let step2_start = Instant::now();
        let playlist_vector = self.compute_playlist_vector(&playlist_artist_mbids).await?;
        log::debug!(
            "[SuggestionsEngine] Step 2 completed in {:?}, vector empty={}",
            step2_start.elapsed(),
            playlist_vector.is_empty()
        );

        if playlist_vector.is_empty() {
            log::warn!("[SuggestionsEngine] Playlist vector is empty, returning empty result");
            return Ok(SuggestionResult {
                tracks: Vec::new(),
                source_artists: Vec::new(),
                playlist_artists_count: playlist_artist_mbids.len(),
                similar_artists_count: 0,
            });
        }

        // Resolve playlist identity once. Every related candidate is validated
        // against these seed genre/tag/neighbourhood facts, never a global list.
        let seed_context =
            resolve_seed_context(self.artist_lookup.as_ref(), playlist_artists).await;

        // 3. Find related artists (using direct relationships, not vector similarity)
        log::debug!("[SuggestionsEngine] Step 3: Finding related artists");
        let step3_start = Instant::now();
        let exclude_vec: Vec<String> = playlist_artist_mbids.to_vec();
        let similar_artists = {
            let guard__ = self.store.lock().await;
            let store = guard__
                .as_ref()
                .ok_or("No active session - please log in")?;
            // Use direct relationship lookup instead of vector similarity
            // This finds members, collaborators, etc. from the MusicBrainz data
            store.get_all_related_artists(
                &playlist_artist_mbids,
                &exclude_vec,
                self.config.max_artists,
            )?
        };
        log::debug!(
            "[SuggestionsEngine] Step 3 completed in {:?}, found {} related artists",
            step3_start.elapsed(),
            similar_artists.len()
        );

        let similar_artists_count = similar_artists.len();
        let mut source_artists = Vec::new();
        let mut all_tracks = Vec::new();

        // 4a. First, search for tracks by playlist artists themselves (highest relevance)
        log::info!(
            "[SuggestionsEngine] Step 4a: Searching tracks for {} playlist artists",
            playlist_artists.len()
        );
        let step4a_start = Instant::now();

        for (mbid, name) in playlist_artists {
            source_artists.push(name.clone());

            // Search Qobuz for tracks by this playlist artist (similarity = 1.0)
            // Fetch many more tracks since many might already be in playlist
            // For a playlist with 23 tracks, we need to search beyond those to find new ones
            let playlist_artist_limit = (self.config.tracks_per_artist * 5).max(30); // At least 30 tracks
            log::info!(
                "[SuggestionsEngine] Step 4a: Searching for '{}' (MBID: {}) with limit {}",
                name,
                mbid,
                playlist_artist_limit
            );
            let tracks = self
                .search_artist_tracks_with_limit(
                    mbid,
                    Some(name),
                    1.0,
                    playlist_artist_limit,
                    &seed_context,
                )
                .await;
            log::info!(
                "[SuggestionsEngine] Step 4a: Found {} tracks for '{}'",
                tracks.len(),
                name
            );

            let mut added = 0;
            let mut skipped = 0;
            for mut track in tracks {
                // Skip if already in playlist
                if exclude_track_ids.contains(&track.track_id) {
                    skipped += 1;
                    continue;
                }

                if include_reasons {
                    track.reason = Some(format!("More from {}", name));
                }

                all_tracks.push(track);
                added += 1;
            }
            log::info!("[SuggestionsEngine] Step 4a: Added {} tracks for '{}' ({} skipped as already in playlist)", added, name, skipped);
        }
        log::info!(
            "[SuggestionsEngine] Step 4a completed in {:?}, got {} tracks from playlist artists",
            step4a_start.elapsed(),
            all_tracks.len()
        );

        // 4b. Then search for tracks by related/similar artists
        log::debug!(
            "[SuggestionsEngine] Step 4b: Searching tracks for {} related artists",
            similar_artists.len()
        );
        let step4b_start = Instant::now();

        for (i, artist) in similar_artists.iter().enumerate() {
            if artist.similarity < self.config.min_similarity {
                continue;
            }

            if let Some(name) = &artist.name {
                if !source_artists.contains(name) {
                    source_artists.push(name.clone());
                }
            }

            // Search Qobuz for tracks by this related artist
            let tracks = self
                .search_artist_tracks(
                    &artist.mbid,
                    artist.name.as_deref(),
                    artist.similarity,
                    &seed_context,
                )
                .await;

            for mut track in tracks {
                // Skip if already in playlist
                if exclude_track_ids.contains(&track.track_id) {
                    continue;
                }

                // Add reason if requested
                if include_reasons {
                    track.reason = Some(self.generate_reason(
                        &artist.mbid,
                        artist.name.as_deref(),
                        artist.similarity,
                        &playlist_artist_mbids,
                    ));
                }

                all_tracks.push(track);
            }

            // Stop if we have enough tracks
            if all_tracks.len() >= self.config.max_pool_size * 2 {
                log::debug!(
                    "[SuggestionsEngine] Reached extended pool size {} after {} related artists",
                    all_tracks.len(),
                    i + 1
                );
                break;
            }
        }
        log::debug!(
            "[SuggestionsEngine] Step 4b completed in {:?}, got {} total tracks",
            step4b_start.elapsed(),
            all_tracks.len()
        );

        // 4c. If pool is still small, use Qobuz's "similar artists" API as fallback
        // This gives us artists that definitely exist in Qobuz
        const MIN_TRACKS_BEFORE_QOBUZ_SIMILAR: usize = 20;
        if all_tracks.len() < MIN_TRACKS_BEFORE_QOBUZ_SIMILAR {
            log::info!(
                "[SuggestionsEngine] Step 4c: Pool too small ({}), fetching Qobuz similar artists",
                all_tracks.len()
            );
            let step4c_start = Instant::now();

            let mut qobuz_similar_ids: HashSet<u64> = HashSet::new();
            for &similar_id in seed_context.neighbourhood_ids() {
                if !qobuz_similar_ids.insert(similar_id) {
                    continue;
                }
                let Some(similar_artist) = self.artist_lookup.artist_by_id(similar_id).await else {
                    continue;
                };
                if !validate_candidate(
                    self.artist_lookup.as_ref(),
                    &similar_artist,
                    None,
                    &seed_context,
                )
                .await
                {
                    continue;
                }

                if !source_artists.contains(&similar_artist.name) {
                    source_artists.push(similar_artist.name.clone());
                }

                let tracks = self
                    .search_artist_tracks_by_qobuz_id(similar_artist.id, &similar_artist.name, 0.8)
                    .await;

                for mut track in tracks {
                    if exclude_track_ids.contains(&track.track_id) {
                        continue;
                    }
                    if include_reasons {
                        track.reason = Some("Similar to your playlist (Qobuz)".to_string());
                    }
                    all_tracks.push(track);
                }

                if all_tracks.len() >= self.config.max_pool_size {
                    break;
                }
            }

            log::debug!(
                "[SuggestionsEngine] Step 4c completed in {:?}, now have {} tracks from {} Qobuz similar artists",
                step4c_start.elapsed(),
                all_tracks.len(),
                qobuz_similar_ids.len()
            );
        }

        all_tracks = rank_dedup_and_truncate(all_tracks, self.config.max_pool_size);

        Ok(SuggestionResult {
            tracks: all_tracks,
            source_artists,
            playlist_artists_count: playlist_artist_mbids.len(),
            similar_artists_count,
        })
    }

    /// Compute combined vector for playlist artists
    async fn compute_playlist_vector(
        &self,
        artist_mbids: &[String],
    ) -> Result<SparseVector, String> {
        let mut combined = SparseVector::new();
        let guard__ = self.store.lock().await;
        let store = guard__
            .as_ref()
            .ok_or("No active session - please log in")?;

        for mbid in artist_mbids {
            if let Some(vector) = store.get_vector(mbid) {
                combined = combined.add(&vector);
            }
        }

        // Normalize to unit length
        Ok(combined.normalize())
    }

    /// Search Qobuz for tracks by an artist (uses default tracks_per_artist limit)
    async fn search_artist_tracks(
        &self,
        artist_mbid: &str,
        artist_name: Option<&str>,
        similarity: f32,
        seed_context: &SeedContext,
    ) -> Vec<SuggestedTrack> {
        self.search_artist_tracks_with_limit(
            artist_mbid,
            artist_name,
            similarity,
            self.config.tracks_per_artist,
            seed_context,
        )
        .await
    }

    /// Search Qobuz for tracks by Qobuz artist ID (more reliable when we already validated the artist)
    async fn search_artist_tracks_by_qobuz_id(
        &self,
        qobuz_artist_id: u64,
        artist_name: &str,
        similarity: f32,
    ) -> Vec<SuggestedTrack> {
        let limit = self.config.tracks_per_artist;
        let guard = self.qobuz_client.read().await;
        let Some(client) = guard.as_ref() else {
            log::warn!("[SuggestionsEngine] No active Qobuz session; skipping");
            return Vec::new();
        };

        // Search by artist name but verify tracks belong to this specific Qobuz artist ID
        match client
            .search_tracks(artist_name, (limit * 3) as u32, 0, None)
            .await
        {
            Ok(results) => {
                let mut tracks = Vec::new();

                for item in results.items {
                    // Only accept tracks from this exact artist (by ID)
                    let performer_id = item.performer.as_ref().map(|p| p.id);
                    if performer_id != Some(qobuz_artist_id) {
                        continue;
                    }

                    tracks.push(self.track_to_suggested_with_qobuz_id(
                        &item,
                        qobuz_artist_id,
                        similarity,
                    ));
                    if tracks.len() >= limit {
                        break;
                    }
                }

                tracks
            }
            Err(e) => {
                log::warn!(
                    "Failed to search tracks for {} (Qobuz ID {}): {}",
                    artist_name,
                    qobuz_artist_id,
                    e
                );
                Vec::new()
            }
        }
    }

    /// Convert a Track to a SuggestedTrack (using Qobuz artist ID instead of MBID)
    fn track_to_suggested_with_qobuz_id(
        &self,
        track: &Track,
        _qobuz_artist_id: u64,
        similarity: f32,
    ) -> SuggestedTrack {
        let (album_title, album_id, album_image_url) = match &track.album {
            Some(album) => {
                let image_url = album
                    .image
                    .thumbnail
                    .as_ref()
                    .or(album.image.small.as_ref())
                    .or(album.image.large.as_ref())
                    .cloned();
                (album.title.clone(), album.id.clone(), image_url)
            }
            None => (String::new(), String::new(), None),
        };

        let (track_artist, artist_id) = match &track.performer {
            Some(p) => (p.name.clone(), Some(p.id)),
            None => (String::new(), None),
        };

        SuggestedTrack {
            track_id: track.id,
            title: track.title.clone(),
            artist_name: track_artist,
            artist_id,
            artist_mbid: None, // No MBID for Qobuz-sourced similar artists
            album_title,
            album_id,
            album_image_url,
            duration: track.duration,
            similarity_score: similarity,
            reason: None,
        }
    }

    /// Search Qobuz for tracks by an artist with custom limit
    ///
    /// Resolves the artist by ID/cache before any text fallback, then requires
    /// seed-relative genre or tag evidence before searching for tracks.
    async fn search_artist_tracks_with_limit(
        &self,
        artist_mbid: &str,
        artist_name: Option<&str>,
        similarity: f32,
        limit: usize,
        seed_context: &SeedContext,
    ) -> Vec<SuggestedTrack> {
        let search_query = match artist_name {
            Some(name) => name.to_string(),
            None => {
                // Try to get name from store
                let guard__ = self.store.lock().await;
                if let Some(store) = guard__.as_ref() {
                    store
                        .get_artist_name(artist_mbid)
                        .unwrap_or_else(|| artist_mbid.to_string())
                } else {
                    artist_mbid.to_string()
                }
            }
        };

        // Resolve identity before taking the client lock: the production lookup
        // uses the same lock for its direct-ID/cache-first checks.
        let validated_artist =
            if let Some(seed_artist) = seed_context.resolved_seed(artist_mbid, &search_query) {
                let candidate_mbid = (!artist_mbid.starts_with("qobuz:")).then_some(artist_mbid);
                validate_candidate(
                    self.artist_lookup.as_ref(),
                    seed_artist,
                    candidate_mbid,
                    seed_context,
                )
                .await
                .then(|| seed_artist.clone())
            } else {
                resolve_candidate(
                    self.artist_lookup.as_ref(),
                    artist_mbid,
                    &search_query,
                    seed_context,
                )
                .await
            };
        let Some(validated_artist) = validated_artist else {
            log::info!(
                "[SuggestionsEngine] Skipping '{}' - identity could not be validated against playlist seeds",
                search_query
            );
            return Vec::new();
        };

        let qobuz_artist_id = validated_artist.id;
        let qobuz_artist_name = validated_artist.name;
        log::info!(
            "[SuggestionsEngine] Validated '{}' -> Qobuz artist '{}' (ID: {})",
            search_query,
            qobuz_artist_name,
            qobuz_artist_id
        );

        let guard = self.qobuz_client.read().await;
        let Some(client) = guard.as_ref() else {
            log::warn!("[SuggestionsEngine] No active Qobuz session; skipping");
            return Vec::new();
        };

        // Step 2: Search for tracks by artist name
        // Fetch many more since search results include tracks where the artist appears,
        // not just tracks BY the artist. We filter down to exact matches.
        let search_limit = ((limit * 5) as u32).max(100).min(500); // Between 100-500
        match client
            .search_tracks(&search_query, search_limit, 0, None)
            .await
        {
            Ok(results) => {
                let mut tracks = Vec::new();

                for item in results.items {
                    // Verify the track's performer matches the validated Qobuz artist
                    // Use both ID matching (best) and name matching (fallback)
                    let performer_id = item.performer.as_ref().map(|p| p.id);
                    let performer_name = item
                        .performer
                        .as_ref()
                        .map(|p| p.name.clone())
                        .unwrap_or_default();

                    // Prefer ID match (exact), fall back to name comparison
                    let is_match = performer_id == Some(qobuz_artist_id)
                        || names_similar(&performer_name, &qobuz_artist_name);

                    if is_match {
                        tracks.push(self.track_to_suggested(&item, artist_mbid, similarity));
                        if tracks.len() >= limit {
                            break;
                        }
                    }
                }

                tracks
            }
            Err(e) => {
                log::warn!("Failed to search tracks for {}: {}", search_query, e);
                Vec::new()
            }
        }
    }

    /// Convert a Track to a SuggestedTrack
    fn track_to_suggested(
        &self,
        track: &Track,
        artist_mbid: &str,
        similarity: f32,
    ) -> SuggestedTrack {
        // Extract album info including image URL
        let (album_title, album_id, album_image_url) = match &track.album {
            Some(album) => {
                let image_url = album
                    .image
                    .thumbnail
                    .as_ref()
                    .or(album.image.small.as_ref())
                    .or(album.image.large.as_ref())
                    .cloned();
                (album.title.clone(), album.id.clone(), image_url)
            }
            None => (String::new(), String::new(), None),
        };

        // Extract artist name and ID from track performer
        let (track_artist, artist_id) = match &track.performer {
            Some(p) => (p.name.clone(), Some(p.id)),
            None => (String::new(), None),
        };

        SuggestedTrack {
            track_id: track.id,
            title: track.title.clone(),
            artist_name: track_artist,
            artist_id,
            artist_mbid: Some(artist_mbid.to_string()),
            album_title,
            album_id,
            album_image_url,
            duration: track.duration,
            similarity_score: similarity,
            reason: None,
        }
    }

    /// Generate a human-readable reason for suggestion
    fn generate_reason(
        &self,
        _artist_mbid: &str,
        artist_name: Option<&str>,
        similarity: f32,
        _playlist_artists: &[String],
    ) -> String {
        let name = artist_name.unwrap_or("Unknown");
        let score_pct = (similarity * 100.0).round() as u32;

        format!("Similar to your playlist ({score_pct}% match) - {name}")
    }
}

fn rank_dedup_and_truncate(
    mut tracks: Vec<SuggestedTrack>,
    max_pool_size: usize,
) -> Vec<SuggestedTrack> {
    // Stable sorting preserves insertion order for equal scores, so playlist
    // artists (inserted first) keep priority without discarding the ranking.
    tracks.sort_by(|left, right| right.similarity_score.total_cmp(&left.similarity_score));

    // Sorting first makes the first title/artist occurrence the highest-score
    // version, so retain now implements the deduplication promise honestly.
    let mut seen = HashSet::new();
    tracks.retain(|track| {
        seen.insert((track.title.to_lowercase(), track.artist_name.to_lowercase()))
    });
    tracks.truncate(max_pool_size);
    tracks
}

/// Check if two artist names are similar enough to be considered a match
///
/// STRICT matching to prevent false positives like:
/// - "Martín Méndez" matching "Tomas Martin Lopez" (share "Martin")
/// - "Martín Méndez" matching "Martin Mendez" (different person, same name)
///
/// For person names (2-3 words), we require ALL words to match.
/// This handles "George Harrison" vs "Harrison, George" but rejects partial matches.
fn names_similar(name1: &str, name2: &str) -> bool {
    let norm1 = normalize_name(name1);
    let norm2 = normalize_name(name2);

    // Exact match after normalization
    if norm1 == norm2 {
        return true;
    }

    // Split into words
    let words1: HashSet<&str> = norm1.split_whitespace().collect();
    let words2: HashSet<&str> = norm2.split_whitespace().collect();

    if words1.is_empty() || words2.is_empty() {
        return false;
    }

    // Count matching words
    let matches = words1.intersection(&words2).count();
    let max_words = words1.len().max(words2.len());
    let min_words = words1.len().min(words2.len());

    // VERY STRICT for person names:
    // - For 2-word names: require EXACT same words (handles "George Harrison" vs "Harrison, George")
    // - For 3-word names: allow at most 1 extra word
    // - This rejects "Martin Lopez" vs "Tomas Martin Lopez" (different people)
    if min_words == 2 {
        // For 2-word names, require EXACTLY the same words (just different order allowed)
        // "Martin Lopez" vs "Tomas Martin Lopez" -> max_words=3, min_words=2 -> REJECT
        // "George Harrison" vs "Harrison, George" -> max_words=2, min_words=2 -> ACCEPT
        matches == min_words && max_words == min_words
    } else if min_words == 3 {
        // For 3-word names, allow at most 1 extra word
        matches >= min_words && (max_words - min_words) <= 1
    } else {
        // For longer names (bands, etc.), allow some flexibility
        matches as f32 / max_words as f32 >= 0.75
    }
}

/// Extract unique artist MBIDs from playlist tracks
///
/// This is a helper function that should be called from the Tauri command
/// to get artist MBIDs from track data.
pub fn extract_artist_mbids(
    tracks: &[(u64, Option<String>)], // (track_id, artist_mbid)
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut mbids = Vec::new();

    for (_, mbid) in tracks {
        if let Some(id) = mbid {
            if !id.is_empty() && seen.insert(id.clone()) {
                mbids.push(id.clone());
            }
        }
    }

    mbids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_artist_mbids() {
        let tracks = vec![
            (1, Some("mbid-1".to_string())),
            (2, Some("mbid-2".to_string())),
            (3, Some("mbid-1".to_string())), // Duplicate
            (4, None),                       // No MBID
            (5, Some("".to_string())),       // Empty MBID
            (6, Some("mbid-3".to_string())),
        ];

        let mbids = extract_artist_mbids(&tracks);

        assert_eq!(mbids.len(), 3);
        assert!(mbids.contains(&"mbid-1".to_string()));
        assert!(mbids.contains(&"mbid-2".to_string()));
        assert!(mbids.contains(&"mbid-3".to_string()));
    }

    #[test]
    fn test_suggestion_config_default() {
        let config = SuggestionConfig::default();

        assert_eq!(config.max_artists, 30);
        assert_eq!(config.tracks_per_artist, 6);
        assert_eq!(config.max_pool_size, 150);
        assert_eq!(config.vector_max_age_days, 7);
        assert!(config.min_similarity > 0.0);
    }

    fn suggested(track_id: u64, title: &str, artist: &str, score: f32) -> SuggestedTrack {
        SuggestedTrack {
            track_id,
            title: title.to_string(),
            artist_name: artist.to_string(),
            artist_id: None,
            artist_mbid: None,
            album_title: String::new(),
            album_id: String::new(),
            album_image_url: None,
            duration: 0,
            similarity_score: score,
            reason: None,
        }
    }

    #[test]
    fn ranking_happens_before_pool_truncation() {
        let tracks = vec![
            suggested(1, "Lower", "Artist", 0.3),
            suggested(2, "Highest", "Artist", 0.9),
            suggested(3, "Middle", "Artist", 0.6),
        ];

        let ranked = rank_dedup_and_truncate(tracks, 2);

        assert_eq!(
            ranked
                .iter()
                .map(|track| track.track_id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn dedup_keeps_the_highest_score_version() {
        let tracks = vec![
            suggested(1, "Duplicate", "Same Artist", 0.3),
            suggested(2, "DUPLICATE", "SAME ARTIST", 0.9),
        ];

        let deduped = rank_dedup_and_truncate(tracks, 10);

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].track_id, 2);
        assert_eq!(deduped[0].similarity_score, 0.9);
    }

    #[test]
    fn equal_scores_keep_insertion_order() {
        let tracks = vec![
            suggested(1, "Playlist Artist", "Seed", 0.8),
            suggested(2, "Related Artist", "Related", 0.8),
        ];

        let ranked = rank_dedup_and_truncate(tracks, 10);

        assert_eq!(
            ranked
                .iter()
                .map(|track| track.track_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
}
