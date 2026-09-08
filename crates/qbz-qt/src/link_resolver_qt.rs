//! Frontend-neutral music-link resolution adapted to the Qt navigation stack.
//!
//! `qbz-music-link` owns URL detection, cross-provider metadata lookup and the
//! smart Qobuz search. This module supplies the live `QbzCore` search bridge
//! and turns a resolved track into its album before handing the existing Qt
//! routers a navigation target.

use std::sync::Arc;

use async_trait::async_trait;
use qbz_app::shell::AppRuntime;
use qbz_core::LoggingAdapter;
use qbz_models::{Album, SearchResultsPage, Track};
use qbz_music_link::{
    resolve_music_link, MusicLinkResult, QobuzSearchBridge, ResolvedLink, SongLinkClient,
};

type Runtime = Arc<AppRuntime<LoggingAdapter>>;

/// Result shape the two Qt consumers need: the modal and launcher deep links.
pub(crate) enum Outcome {
    Resolved(ResolvedLink),
    PlaylistDetected(String),
    NotOnQobuz,
}

struct CoreSearchBridge {
    runtime: Runtime,
}

#[async_trait]
impl QobuzSearchBridge for CoreSearchBridge {
    async fn search_tracks(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<SearchResultsPage<Track>, String> {
        self.runtime
            .core()
            .search_tracks(query, limit as u32, offset as u32, None)
            .await
            .map_err(|e| e.to_string())
    }

    async fn search_albums(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<SearchResultsPage<Album>, String> {
        self.runtime
            .core()
            .search_albums(query, limit as u32, offset as u32, None)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Network-free platform hint for the modal's leading glyph.
pub(crate) fn detect_platform(url: &str) -> &'static str {
    let lower = url.trim().to_ascii_lowercase();
    if lower.contains("qobuz.com/") || lower.starts_with("qobuzapp://") {
        "qobuz"
    } else if lower.contains("spotify.com/") || lower.starts_with("spotify:") {
        "spotify"
    } else if lower.contains("music.apple.com/") {
        "apple"
    } else if lower.contains("tidal.com/") {
        "tidal"
    } else if lower.contains("deezer.com/") {
        "deezer"
    } else if lower.contains("song.link/")
        || lower.contains("album.link/")
        || lower.contains("odesli.co/")
    {
        "songlink"
    } else {
        ""
    }
}

/// Resolve a URL and normalize `OpenTrack` to the album route the UI exposes.
pub(crate) async fn resolve(runtime: Runtime, url: String) -> Result<Outcome, String> {
    let search = CoreSearchBridge {
        runtime: runtime.clone(),
    };
    let songlink = SongLinkClient::new();
    match resolve_music_link(&url, &songlink, &search)
        .await
        .map_err(|e| e.to_string())?
    {
        MusicLinkResult::Resolved {
            link: ResolvedLink::OpenTrack(track_id),
            ..
        } => {
            let track = runtime
                .core()
                .get_track(track_id)
                .await
                .map_err(|e| format!("track {track_id}: {e}"))?;
            let album_id = track
                .album
                .as_ref()
                .map(|album| album.id.clone())
                .filter(|id| !id.is_empty())
                .ok_or_else(|| format!("track {track_id} has no album"))?;
            Ok(Outcome::Resolved(ResolvedLink::OpenAlbum(album_id)))
        }
        MusicLinkResult::Resolved { link, .. } => Ok(Outcome::Resolved(link)),
        MusicLinkResult::PlaylistDetected { provider } => Ok(Outcome::PlaylistDetected(provider)),
        MusicLinkResult::NotOnQobuz { .. } => Ok(Outcome::NotOnQobuz),
    }
}

/// Send a resolved entity through the same routers used by cards and rows.
pub(crate) fn navigate(link: ResolvedLink) {
    match link {
        ResolvedLink::OpenAlbum(id) => crate::open_album(id),
        ResolvedLink::OpenArtist(id) => crate::open_artist(id.to_string()),
        ResolvedLink::OpenPlaylist(id) => crate::open_playlist(id.to_string()),
        // `resolve` normalizes this arm. Keep it exhaustive so a future caller
        // cannot silently discard a direct track result.
        ResolvedLink::OpenTrack(id) => {
            log::error!("[qbz-qt] link resolver: unnormalized track {id}");
        }
    }
}

/// Resolve a launcher/D-Bus URL without opening the modal.
pub(crate) fn resolve_deep_link(url: String) {
    let runtime = crate::app();
    crate::spawn(async move {
        match resolve(runtime, url).await {
            Ok(Outcome::Resolved(link)) => navigate(link),
            Ok(Outcome::PlaylistDetected(provider)) => {
                log::info!("[qbz-qt] deep link: {provider} playlist has no native route");
            }
            Ok(Outcome::NotOnQobuz) => {
                crate::toast_qt::error(qbz_i18n::t("This content is not available on Qobuz"));
            }
            Err(e) => {
                log::warn!("[qbz-qt] deep link resolve failed: {e}");
                crate::toast_qt::error(qbz_i18n::t("Could not resolve that link"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_detection_is_specific_and_case_insensitive() {
        assert_eq!(detect_platform("HTTPS://OPEN.QOBUZ.COM/album/a"), "qobuz");
        assert_eq!(detect_platform("spotify:track:123"), "spotify");
        assert_eq!(detect_platform("https://music.apple.com/a"), "apple");
        assert_eq!(detect_platform("https://listen.tidal.com/track/1"), "tidal");
        assert_eq!(detect_platform("https://www.deezer.com/track/1"), "deezer");
        assert_eq!(detect_platform("https://song.link/x"), "songlink");
        assert_eq!(detect_platform("https://example.com"), "");
    }
}
