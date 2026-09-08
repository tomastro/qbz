//! The search bridge that decouples Qobuz search from any frontend.
//!
//! The resolver needs to search Qobuz, but it must not depend on Tauri or any
//! particular frontend's core. Each frontend implements this trait over its own
//! `QbzCore` / `CoreBridge` and passes a `&dyn QobuzSearchBridge` to the resolver.
//!
//! The return types are the REAL `qbz_models` search-result pages:
//! - `SearchResultsPage<Track>` (items have a numeric `id: u64`)
//! - `SearchResultsPage<Album>` (items have a string `id: String` and `title: String`)

use qbz_models::{Album, SearchResultsPage, Track};

/// Searches Qobuz for tracks and albums. Implemented by the frontend.
#[async_trait::async_trait]
pub trait QobuzSearchBridge: Send + Sync {
    /// Search Qobuz tracks. `limit`/`offset` mirror the Qobuz search params.
    async fn search_tracks(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<SearchResultsPage<Track>, String>;

    /// Search Qobuz albums. `limit`/`offset` mirror the Qobuz search params.
    async fn search_albums(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<SearchResultsPage<Album>, String>;
}
