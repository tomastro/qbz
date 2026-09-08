//! Last.fm "similar albums" row on the album page.
//!
//! Port of `qbz/src/external_reco.rs` (`CoreRecoCatalog` +
//! `load_similar_albums_seeded`). The recommendation building itself lives in
//! `qbz-external-reco` and is frontend-agnostic (ADR-006); this module only
//! adapts `QbzCore` to its `RecoCatalog` trait and publishes the result.
//!
//! Strictly opt-in: with Last.fm not connected the row resolves to nothing and
//! NO network call is made. The resolved row is cached per album for 30 days in
//! the same `RecoCache` the Discover tab uses, so re-opening an album costs
//! zero Last.fm and zero Qobuz traffic. Only a NON-EMPTY result is cached — a
//! transient Last.fm failure must not hide the row until the app restarts.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use qbz_app::shell::AppRuntime;
use qbz_core::LoggingAdapter;
use qbz_external_reco::{
    build_similar_albums_seeded, AlbumReco, LastFmHandle, LocalHistory, RecoCache, RecoCatalog,
    RecoInputs,
};
use qbz_integrations::lastfm::LastFmClient;
use qbz_models::{Album, Artist, Track};

/// Same 30-day TTL as the Slint side.
const LASTFM_SIMILAR_TTL_SECS: i64 = 30 * 24 * 60 * 60;

static CACHE_DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();

/// The directory holding `external_reco_cache.db`. Shared with the Discover
/// Recommendations tab (`recommendations_qt`) ON PURPOSE: one resolution +
/// results cache for both surfaces, so a similar-albums lookup paid for here
/// is reused there and vice versa.
pub(crate) fn cache_dir() -> Option<std::path::PathBuf> {
    CACHE_DIR
        .get_or_init(|| dirs::cache_dir().map(|d| d.join("qbz")))
        .clone()
}

/// The daily rotation seed, so the row varies day to day but is stable within
/// one day (matching the Slint `rotation_seed`). Shared with
/// `recommendations_qt` so both surfaces rotate together.
pub(crate) fn rotation_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

// ── RecoCatalog over QbzCore (errors -> empty) ──────────────────────────────

/// THE `RecoCatalog` adapter over `QbzCore` for this frontend. Shared with the
/// Discover Recommendations tab (`recommendations_qt`) — there is exactly one
/// adapter in the port, not one per surface.
pub(crate) struct CoreRecoCatalog {
    pub(crate) runtime: Arc<AppRuntime<LoggingAdapter>>,
}

#[async_trait]
impl RecoCatalog for CoreRecoCatalog {
    async fn search_tracks(&self, query: &str, limit: usize) -> Vec<Track> {
        self.runtime
            .core()
            .search_tracks(query, limit as u32, 0, None)
            .await
            .map(|p| p.items)
            .unwrap_or_default()
    }
    async fn search_artists(&self, query: &str, limit: usize) -> Vec<Artist> {
        self.runtime
            .core()
            .search_artists(query, limit as u32, 0, None)
            .await
            .map(|p| p.items)
            .unwrap_or_default()
    }
    async fn search_albums(&self, query: &str, limit: usize) -> Vec<Album> {
        self.runtime
            .core()
            .search_albums(query, limit as u32, 0, None)
            .await
            .map(|p| p.items)
            .unwrap_or_default()
    }
    async fn artist_top_tracks(&self, artist_id: u64, limit: usize) -> Vec<Track> {
        self.runtime
            .core()
            .get_artist_tracks(artist_id, limit as u32, 0)
            .await
            .map(|c| c.items)
            .unwrap_or_default()
    }
    async fn artist_albums(&self, artist_id: u64, limit: usize) -> Vec<Album> {
        self.runtime
            .core()
            .get_artist_albums(artist_id, Some(limit as u32), Some(0))
            .await
            .map(|a| a.items)
            .unwrap_or_default()
    }
    async fn featured_albums(&self, kind: &str, limit: usize) -> Vec<Album> {
        self.runtime
            .core()
            .get_featured_albums(kind, limit as u32, 0, None)
            .await
            .map(|p| p.items)
            .unwrap_or_default()
    }
    async fn get_artist(&self, artist_id: u64) -> Option<Artist> {
        self.runtime.core().get_artist(artist_id).await.ok()
    }
}

// ── Loader ──────────────────────────────────────────────────────────────────

/// Resolve the Last.fm similar-album row for one album. Empty (and silent)
/// when Last.fm is not connected.
pub async fn similar_albums(
    runtime: &Arc<AppRuntime<LoggingAdapter>>,
    album_id: &str,
    seed_artist: &str,
    exclude_pairs: &[(String, String)],
    exclude_ids: &HashSet<String>,
) -> Vec<AlbumReco> {
    let cfg = crate::integrations_qt::scrobble_settings();
    if !cfg.lastfm_is_authed() || cfg.lastfm_username.is_empty() {
        return Vec::new();
    }

    let cache = cache_dir()
        .and_then(|dir| RecoCache::open_at(&dir).ok())
        .map(Mutex::new);
    let cache_key = format!("album_lastfm:{album_id}");

    // Cache hit: the row without any Last.fm or Qobuz traffic.
    if let Some(c) = &cache {
        if let Some(json) = c
            .lock()
            .ok()
            .and_then(|g| g.get_results(&cache_key, LASTFM_SIMILAR_TTL_SECS))
        {
            if let Ok(cached) = serde_json::from_str::<Vec<AlbumReco>>(&json) {
                return cached;
            }
        }
    }

    let lastfm_client = LastFmClient::new();
    // The core's client (respects the MusicBrainz opt-out) — see the same
    // note in `recommendations_qt::run`.
    let mb_client = runtime.core().musicbrainz_client();
    let catalog = CoreRecoCatalog {
        runtime: runtime.clone(),
    };
    let inputs = RecoInputs {
        lastfm: Some(LastFmHandle {
            username: cfg.lastfm_username.clone(),
            client: &lastfm_client,
        }),
        listenbrainz: None,
        musicbrainz: mb_client.as_ref(),
        catalog: &catalog,
        cache: cache.as_ref(),
        local: LocalHistory::default(),
        rotation_seed: rotation_seed(),
    };
    let mut recos = build_similar_albums_seeded(&inputs, seed_artist, exclude_pairs).await;
    // Drop any that resolved to an id the page's own Qobuz row already shows —
    // the pre-resolution artist|title dedup can miss those.
    recos.retain(|r| !exclude_ids.contains(&r.qobuz_album_id));

    // Only a non-empty result is cached; an empty one is likely transient.
    if !recos.is_empty() {
        if let Some(c) = &cache {
            if let (Ok(g), Ok(json)) = (c.lock(), serde_json::to_string(&recos)) {
                g.put_results(&cache_key, &json);
            }
        }
    }
    recos
}
