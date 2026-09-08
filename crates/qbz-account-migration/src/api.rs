//! The slice of the Qobuz client the migration needs, as a trait so the
//! planner and the applier are tested against a fake and so another host
//! (qbzd) can supply its own client.

use qbz_models::{Playlist, PlaylistWithTrackIds};
use qbz_qobuz::QobuzClient;
use serde_json::Value;

/// Reads and writes on the ACTIVE session's account. Writes are additive
/// by construction: there is no delete, no unsubscribe, no update here.
#[allow(async_fn_in_trait)]
pub trait MigrationApi: Send + Sync {
    /// `favorite/getUserFavorites?type=<plural>&limit&offset` — the raw
    /// envelope `{ <plural>: { items, total } }`.
    async fn favorites_page(&self, plural: &str, limit: u32, offset: u32) -> Result<Value, String>;
    /// Every playlist the account owns or follows.
    async fn user_playlists(&self) -> Result<Vec<Playlist>, String>;
    /// A playlist's track ids in order (`extra=track_ids`).
    async fn playlist_track_ids(&self, playlist_id: u64) -> Result<PlaylistWithTrackIds, String>;
    /// `favorite/create` for one id; `singular` is `album|track|artist|label|award`.
    async fn add_favorite(&self, singular: &str, id: &str) -> Result<(), String>;
    async fn create_playlist(
        &self,
        name: &str,
        description: Option<&str>,
        is_public: bool,
    ) -> Result<Playlist, String>;
    /// Appends `track_ids` in order (the caller chunks).
    async fn add_tracks(&self, playlist_id: u64, track_ids: &[u64]) -> Result<(), String>;
    async fn subscribe(&self, playlist_id: u64) -> Result<(), String>;
}

impl MigrationApi for QobuzClient {
    async fn favorites_page(&self, plural: &str, limit: u32, offset: u32) -> Result<Value, String> {
        self.get_favorites(plural, limit, offset)
            .await
            .map_err(|e| e.to_string())
    }

    async fn user_playlists(&self) -> Result<Vec<Playlist>, String> {
        self.get_user_playlists().await.map_err(|e| e.to_string())
    }

    async fn playlist_track_ids(&self, playlist_id: u64) -> Result<PlaylistWithTrackIds, String> {
        self.get_playlist_track_ids(playlist_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn add_favorite(&self, singular: &str, id: &str) -> Result<(), String> {
        QobuzClient::add_favorite(self, singular, id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn create_playlist(
        &self,
        name: &str,
        description: Option<&str>,
        is_public: bool,
    ) -> Result<Playlist, String> {
        QobuzClient::create_playlist(self, name, description, is_public)
            .await
            .map_err(|e| e.to_string())
    }

    async fn add_tracks(&self, playlist_id: u64, track_ids: &[u64]) -> Result<(), String> {
        self.add_tracks_to_playlist(playlist_id, track_ids)
            .await
            .map_err(|e| e.to_string())
    }

    async fn subscribe(&self, playlist_id: u64) -> Result<(), String> {
        self.subscribe_playlist(playlist_id)
            .await
            .map_err(|e| e.to_string())
    }
}
