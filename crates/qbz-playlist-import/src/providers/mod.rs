//! Provider implementations

pub mod apple;
pub mod deezer;
pub mod spotify;
pub mod tidal;

use serde::{Deserialize, Serialize};

use crate::errors::PlaylistImportError;
use crate::models::ImportPlaylist;

/// Which streaming platform a music link belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MusicProvider {
    Spotify,
    AppleMusic,
    Tidal,
    Deezer,
}

/// The kind of resource a music URL points to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MusicResource {
    /// A native Qobuz URL — resolve directly.
    Qobuz,
    /// A single track on a third-party platform.
    Track {
        provider: MusicProvider,
        url: String,
    },
    /// An album on a third-party platform.
    Album {
        provider: MusicProvider,
        url: String,
    },
    /// A playlist — should be redirected to the Playlist Importer.
    Playlist { provider: MusicProvider },
    /// A song.link / album.link / odesli.co URL — resolve via Odesli API.
    SongLink { url: String },
}

/// Detect what kind of music resource a URL points to.
///
/// Returns `None` for URLs that don't match any supported platform.
pub fn detect_music_resource(url: &str) -> Option<MusicResource> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // 1. Qobuz — resolve_link() handles this natively
    if qbz_qobuz::resolve_link(url).is_ok() {
        return Some(MusicResource::Qobuz);
    }

    // 2. song.link / album.link / odesli.co URLs
    let lower = url.to_ascii_lowercase();
    if lower.contains("song.link/") || lower.contains("album.link/") || lower.contains("odesli.co/")
    {
        return Some(MusicResource::SongLink {
            url: url.to_string(),
        });
    }

    // 3. Per-provider detection (track/album/playlist)
    if let Some(resource) = spotify::detect_resource(url) {
        return Some(resource);
    }
    if let Some(resource) = apple::detect_resource(url) {
        return Some(resource);
    }
    if let Some(resource) = tidal::detect_resource(url) {
        return Some(resource);
    }
    if let Some(resource) = deezer::detect_resource(url) {
        return Some(resource);
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    Spotify {
        playlist_id: String,
    },
    AppleMusic {
        storefront: String,
        playlist_id: String,
    },
    Tidal {
        playlist_id: String,
    },
    Deezer {
        playlist_id: String,
    },
}

pub fn detect_provider(url: &str) -> Result<ProviderKind, PlaylistImportError> {
    if let Some(id) = spotify::parse_playlist_id(url) {
        return Ok(ProviderKind::Spotify { playlist_id: id });
    }
    if let Some((storefront, id)) = apple::parse_playlist_id(url) {
        return Ok(ProviderKind::AppleMusic {
            storefront,
            playlist_id: id,
        });
    }
    if let Some(id) = tidal::parse_playlist_id(url) {
        return Ok(ProviderKind::Tidal { playlist_id: id });
    }
    if let Some(id) = deezer::parse_playlist_id(url) {
        return Ok(ProviderKind::Deezer { playlist_id: id });
    }

    Err(PlaylistImportError::UnsupportedProvider(url.to_string()))
}

/// Fetch playlist (proxy handles credentials)
pub async fn fetch_playlist(kind: ProviderKind) -> Result<ImportPlaylist, PlaylistImportError> {
    match kind {
        ProviderKind::Spotify { playlist_id } => spotify::fetch_playlist(&playlist_id).await,
        ProviderKind::AppleMusic {
            storefront,
            playlist_id,
        } => apple::fetch_playlist(&storefront, &playlist_id).await,
        // Default Tidal storefront ("US"); callers wanting another country
        // call tidal::fetch_playlist directly (e.g. with an env read at
        // their edge — the Tauri original read TIDAL_COUNTRY_CODE here).
        ProviderKind::Tidal { playlist_id } => tidal::fetch_playlist(&playlist_id, None).await,
        ProviderKind::Deezer { playlist_id } => deezer::fetch_playlist(&playlist_id).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_provider_table() {
        // Spotify URI / URL / embed forms + query strip
        assert_eq!(
            detect_provider("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M").unwrap(),
            ProviderKind::Spotify {
                playlist_id: "37i9dQZF1DXcBWIGoYBM5M".to_string()
            }
        );
        assert_eq!(
            detect_provider("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=abc")
                .unwrap(),
            ProviderKind::Spotify {
                playlist_id: "37i9dQZF1DXcBWIGoYBM5M".to_string()
            }
        );
        assert_eq!(
            detect_provider("https://open.spotify.com/embed/playlist/37i9dQZF1DXcBWIGoYBM5M")
                .unwrap(),
            ProviderKind::Spotify {
                playlist_id: "37i9dQZF1DXcBWIGoYBM5M".to_string()
            }
        );

        // Apple storefront + pl. ids
        assert_eq!(
            detect_provider("https://music.apple.com/us/playlist/top-100-global/pl.d25f5d1181894928af76c85c967f8f31")
                .unwrap(),
            ProviderKind::AppleMusic {
                storefront: "us".to_string(),
                playlist_id: "pl.d25f5d1181894928af76c85c967f8f31".to_string()
            }
        );

        // Tidal
        assert_eq!(
            detect_provider(
                "https://tidal.com/browse/playlist/1b418bb8-90a7-4f87-901d-707993838346"
            )
            .unwrap(),
            ProviderKind::Tidal {
                playlist_id: "1b418bb8-90a7-4f87-901d-707993838346".to_string()
            }
        );

        // Deezer
        assert_eq!(
            detect_provider("https://www.deezer.com/en/playlist/1234567890").unwrap(),
            ProviderKind::Deezer {
                playlist_id: "1234567890".to_string()
            }
        );

        // Rejects
        assert!(detect_provider("https://example.com/playlist/1").is_err());
        assert!(detect_provider("https://open.spotify.com/track/abc").is_err());
        assert!(detect_provider("").is_err());
    }

    #[test]
    fn detect_music_resource_song_link_and_none() {
        assert_eq!(
            detect_music_resource("https://song.link/i/1440857781"),
            Some(MusicResource::SongLink {
                url: "https://song.link/i/1440857781".to_string()
            })
        );
        assert_eq!(detect_music_resource(""), None);
        assert_eq!(detect_music_resource("https://example.com/whatever"), None);
    }

    #[test]
    fn detect_music_resource_playlist_routes_to_importer() {
        assert_eq!(
            detect_music_resource("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            Some(MusicResource::Playlist {
                provider: MusicProvider::Spotify
            })
        );
    }
}
