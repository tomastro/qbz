use super::db::{RadioDb, RadioSeed, RadioSession};
use qbz_models::{Track, TracksContainer};
use qbz_qobuz::QobuzClient;

#[derive(Debug, Clone)]
pub struct BuildRadioOptions {
    pub rng_seed: u64,
    pub artist_spacing: u32,
    pub reseed_every: u32,

    pub seed_tracks_limit: u32,

    pub playlist_limit: usize,
    pub playlist_track_limit: usize,

    pub similar_artists_limit: u32,
    pub similar_artist_tracks_limit: u32,

    pub min_pool_size_for_second_degree: u32,
    pub second_degree_artist_limit: u32,
    pub second_degree_tracks_limit: u32,
}

impl Default for BuildRadioOptions {
    fn default() -> Self {
        Self {
            rng_seed: 0,
            artist_spacing: 5,
            reseed_every: 25,
            seed_tracks_limit: 150,

            playlist_limit: 3,
            playlist_track_limit: 200,

            similar_artists_limit: 8,
            similar_artist_tracks_limit: 30,

            min_pool_size_for_second_degree: 80,
            second_degree_artist_limit: 3,
            second_degree_tracks_limit: 30,
        }
    }
}

pub struct RadioPoolBuilder<'a> {
    pub db: &'a RadioDb,
    pub client: &'a QobuzClient,
    pub options: BuildRadioOptions,
}

impl<'a> RadioPoolBuilder<'a> {
    pub fn new(db: &'a RadioDb, client: &'a QobuzClient, options: BuildRadioOptions) -> Self {
        Self {
            db,
            client,
            options,
        }
    }

    fn is_qobuz_owned(owner_name: &str) -> bool {
        owner_name.to_ascii_lowercase().contains("qobuz")
    }

    fn track_artist_id(track: &Track, fallback: u64) -> u64 {
        if let Some(performer) = track.performer.as_ref() {
            if performer.id != 0 {
                return performer.id;
            }
        }
        fallback
    }

    /// Pool gate. `is_streamable()` rather than the raw field: this predicate
    /// runs over whatever endpoint filled the pool, and an endpoint that simply
    /// does not report availability would otherwise reject every candidate and
    /// leave radio with an empty pool — a total failure of the feature, with no
    /// error anywhere to explain it.
    fn is_music_track(track: &Track) -> bool {
        track.is_streamable() && track.duration > 0 && track.album.is_some()
    }

    fn derive_rng_seed(default_seed: u64, salt: u64) -> u64 {
        if default_seed != 0 {
            return default_seed;
        }
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0))
            ^ salt
    }

    pub async fn create_artist_radio(&self, artist_id: u64) -> Result<RadioSession, String> {
        let rng_seed = Self::derive_rng_seed(self.options.rng_seed, artist_id);
        let session = self.db.create_session(
            RadioSeed::Artist { artist_id },
            rng_seed,
            self.options.artist_spacing,
            self.options.reseed_every,
        )?;

        self.build_pool_for_seed_artist(&session.id, artist_id)
            .await?;

        Ok(session)
    }

    pub async fn create_track_radio(
        &self,
        track_id: u64,
        artist_id: u64,
    ) -> Result<RadioSession, String> {
        let rng_seed = Self::derive_rng_seed(self.options.rng_seed, track_id);
        let session = self.db.create_session(
            RadioSeed::Track {
                track_id,
                artist_id,
            },
            rng_seed,
            self.options.artist_spacing,
            self.options.reseed_every,
        )?;

        let track = self
            .client
            .get_track(track_id)
            .await
            .map_err(|e| format!("Failed to fetch seed track: {}", e))?;

        let track_artist_id = Self::track_artist_id(&track, artist_id);
        if Self::is_music_track(&track) {
            self.db
                .insert_pool_track(&session.id, track.id, track_artist_id, "seed_track", 0)?;
        }

        self.build_pool_for_seed_artist(&session.id, artist_id)
            .await?;

        Ok(session)
    }

    async fn build_pool_for_seed_artist(
        &self,
        session_id: &str,
        seed_artist_id: u64,
    ) -> Result<(), String> {
        // 1) Curated artist playlists (distance 1)
        let artist_detail = self
            .client
            .get_artist_detail(seed_artist_id, None, None)
            .await
            .map_err(|e| format!("Failed to fetch artist detail: {}", e))?;

        let mut playlists = artist_detail.playlists.unwrap_or_default();
        playlists.retain(|p| Self::is_qobuz_owned(&p.owner.name));
        playlists.sort_by(|a, b| {
            b.tracks_count
                .cmp(&a.tracks_count)
                .then_with(|| a.id.cmp(&b.id))
        });

        for playlist in playlists.into_iter().take(self.options.playlist_limit) {
            let playlist = self
                .client
                .get_playlist(playlist.id)
                .await
                .map_err(|e| format!("Failed to fetch playlist {}: {}", playlist.id, e))?;

            let tracks = playlist.tracks.map(|t| t.items).unwrap_or_default();

            for t in tracks.into_iter().take(self.options.playlist_track_limit) {
                if !Self::is_music_track(&t) {
                    continue;
                }
                let artist_id = Self::track_artist_id(&t, seed_artist_id);
                self.db
                    .insert_pool_track(session_id, t.id, artist_id, "curated_playlist", 1)?;
            }
        }

        // 2) Seed tracks (distance 0)
        let tracks: TracksContainer = self
            .client
            .get_artist_tracks(seed_artist_id, self.options.seed_tracks_limit, 0)
            .await
            .map_err(|e| format!("Failed to fetch seed artist tracks: {}", e))?;

        for t in tracks.items {
            if !Self::is_music_track(&t) {
                continue;
            }
            let artist_id = Self::track_artist_id(&t, seed_artist_id);
            self.db
                .insert_pool_track(session_id, t.id, artist_id, "seed_tracks", 0)?;
        }

        // 3) Similar artists (distance 1)
        let similar = self
            .client
            .get_similar_artists(seed_artist_id, self.options.similar_artists_limit, 0)
            .await
            .map_err(|e| format!("Failed to fetch similar artists: {}", e))?;

        let mut first_degree_artist_ids: Vec<u64> =
            similar.items.into_iter().map(|a| a.id).collect();
        first_degree_artist_ids.retain(|id| *id != 0 && *id != seed_artist_id);
        first_degree_artist_ids.sort();
        first_degree_artist_ids.dedup();

        for artist_id in first_degree_artist_ids.iter().copied() {
            let tracks = self
                .client
                .get_artist_tracks(artist_id, self.options.similar_artist_tracks_limit, 0)
                .await
                .map_err(|e| format!("Failed to fetch similar artist tracks: {}", e))?;
            for t in tracks.items {
                if !Self::is_music_track(&t) {
                    continue;
                }
                let artist_id = Self::track_artist_id(&t, artist_id);
                self.db
                    .insert_pool_track(session_id, t.id, artist_id, "similar_artist", 1)?;
            }
        }

        // 4) Second-degree expansion (distance 2, bounded)
        let pool_size = self.db.pool_size(session_id)?;
        if pool_size < self.options.min_pool_size_for_second_degree
            && self.options.second_degree_artist_limit > 0
        {
            let mut added = 0u32;

            for base_artist_id in first_degree_artist_ids.into_iter().take(2) {
                if added >= self.options.second_degree_artist_limit {
                    break;
                }

                let second = self
                    .client
                    .get_similar_artists(base_artist_id, self.options.second_degree_artist_limit, 0)
                    .await
                    .map_err(|e| format!("Failed to fetch second-degree similar artists: {}", e))?;

                for a in second.items {
                    if added >= self.options.second_degree_artist_limit {
                        break;
                    }
                    if a.id == 0 || a.id == seed_artist_id || a.id == base_artist_id {
                        continue;
                    }

                    let tracks = self
                        .client
                        .get_artist_tracks(a.id, self.options.second_degree_tracks_limit, 0)
                        .await
                        .map_err(|e| {
                            format!("Failed to fetch second-degree artist tracks: {}", e)
                        })?;
                    for t in tracks.items {
                        if !Self::is_music_track(&t) {
                            continue;
                        }
                        let artist_id = Self::track_artist_id(&t, a.id);
                        self.db.insert_pool_track(
                            session_id,
                            t.id,
                            artist_id,
                            "second_degree",
                            2,
                        )?;
                    }

                    added += 1;
                }
            }
        }

        Ok(())
    }
}
