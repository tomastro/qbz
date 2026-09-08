//! Remote-metadata converters (MusicBrainz + Discogs -> unified DTOs).
//!
//! Frontend-agnostic copy of the pure converters that live in
//! `src-tauri/src/library/remote_metadata/`, so the Slint frontend can do
//! remote album lookup via `qbz_integrations` without depending on the Tauri
//! binary. The Tauri side keeps its own copy + its cache/state orchestration;
//! only these pure adapters are shared here.

mod models;
pub use models::{
    RemoteAlbumMetadata, RemoteAlbumSearchResult, RemoteArtistCredit, RemoteMetadataError,
    RemoteProvider, RemoteSearchRequest, RemoteSearchResponse, RemoteTrackMetadata,
};

fn musicbrainz_credits(
    credits: Option<&Vec<crate::musicbrainz::ArtistCredit>>,
) -> Vec<RemoteArtistCredit> {
    credits
        .into_iter()
        .flatten()
        .map(|credit| RemoteArtistCredit {
            name: credit
                .name
                .as_deref()
                .unwrap_or(&credit.artist.name)
                .to_string(),
            join_phrase: credit.joinphrase.clone().unwrap_or_default(),
            provider_id: Some(credit.artist.id.clone()),
        })
        .collect()
}

fn display_credit(credits: &[RemoteArtistCredit]) -> String {
    credits
        .iter()
        .map(|credit| format!("{}{}", credit.name, credit.join_phrase))
        .collect()
}

pub fn musicbrainz_release_to_search_result(
    release: &crate::musicbrainz::ReleaseResult,
) -> RemoteAlbumSearchResult {
    // Extract artist from artist-credit
    let artist = release
        .artist_credit
        .as_ref()
        .map(|credits| {
            credits
                .iter()
                .map(|c| {
                    format!(
                        "{}{}",
                        c.name.as_deref().unwrap_or(&c.artist.name),
                        c.joinphrase.as_deref().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    // Extract year from date (YYYY or YYYY-MM-DD)
    let year = release
        .date
        .as_ref()
        .and_then(|d| d.split('-').next().and_then(|y| y.parse::<u16>().ok()));

    // Extract label and catalog number
    let (label, catalog_number) = release
        .label_info
        .as_ref()
        .and_then(|info| info.first())
        .map(|li| {
            (
                li.label.as_ref().map(|l| l.name.clone()),
                li.catalog_number.clone(),
            )
        })
        .unwrap_or((None, None));

    // Get track count - either from direct field or sum from media
    let track_count = release.track_count.or_else(|| {
        release
            .media
            .as_ref()
            .map(|media| media.iter().filter_map(|m| m.track_count).sum())
    });

    // Get format from first medium
    let format = release
        .media
        .as_ref()
        .and_then(|m| m.first())
        .and_then(|m| m.format.clone());

    RemoteAlbumSearchResult {
        provider: RemoteProvider::MusicBrainz,
        provider_id: release.id.clone(),
        title: release.title.clone(),
        artist,
        year,
        track_count,
        country: release.country.clone(),
        label,
        catalog_number,
        confidence: release.score.map(|s| s.min(100) as u8),
        format,
    }
}

// ============ Discogs Adapter ============

/// Parse Discogs track position to (disc_number, track_number)
/// Handles formats: "1", "A1", "1-1", "CD1-1", "1.1"
pub fn parse_discogs_position(position: &str) -> (u8, u8) {
    let position = position.trim();

    // Handle empty position
    if position.is_empty() {
        return (1, 1);
    }

    // Try "X-Y" format (e.g., "1-5", "CD1-3")
    if let Some(pos) = position.find('-') {
        let disc_part = &position[..pos];
        let track_part = &position[pos + 1..];

        // Extract number from disc part (handle "CD1", "1", etc.)
        let disc = disc_part
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u8>()
            .unwrap_or(1);

        let track = track_part.parse::<u8>().unwrap_or(1);
        return (disc, track);
    }

    // Try "X.Y" format
    if let Some(pos) = position.find('.') {
        let disc_part = &position[..pos];
        let track_part = &position[pos + 1..];

        let disc = disc_part.parse::<u8>().unwrap_or(1);
        let track = track_part.parse::<u8>().unwrap_or(1);
        return (disc, track);
    }

    // Handle vinyl sides (A, B, C, D -> disc 1, 1, 2, 2)
    if position.starts_with(|c: char| c.is_ascii_alphabetic()) {
        let side = position.chars().next().unwrap().to_ascii_uppercase();
        let track_str: String = position.chars().skip(1).collect();
        let track = track_str.parse::<u8>().unwrap_or(1);

        let disc = match side {
            'A' | 'B' => 1,
            'C' | 'D' => 2,
            'E' | 'F' => 3,
            _ => 1,
        };

        return (disc, track);
    }

    // Simple number
    let track = position.parse::<u8>().unwrap_or(1);
    (1, track)
}

/// Parse Discogs duration string to milliseconds
/// Handles format: "M:SS" or "MM:SS" or "H:MM:SS"
pub fn parse_discogs_duration(duration: &str) -> Option<u32> {
    let parts: Vec<&str> = duration.split(':').collect();

    match parts.len() {
        2 => {
            // M:SS or MM:SS
            let minutes: u32 = parts[0].parse().ok()?;
            let seconds: u32 = parts[1].parse().ok()?;
            Some((minutes * 60 + seconds) * 1000)
        }
        3 => {
            // H:MM:SS
            let hours: u32 = parts[0].parse().ok()?;
            let minutes: u32 = parts[1].parse().ok()?;
            let seconds: u32 = parts[2].parse().ok()?;
            Some((hours * 3600 + minutes * 60 + seconds) * 1000)
        }
        _ => None,
    }
}

/// Convert Discogs extended search result to unified DTO
pub fn discogs_extended_to_search_result(
    result: &crate::discogs::DiscogsSearchResultExtended,
) -> RemoteAlbumSearchResult {
    // Discogs title format is usually "Artist - Album"
    let (artist, title) = if let Some(pos) = result.title.find(" - ") {
        let (a, t) = result.title.split_at(pos);
        (a.to_string(), t.trim_start_matches(" - ").to_string())
    } else {
        ("Unknown Artist".to_string(), result.title.clone())
    };

    // Parse year from string
    let year = result.year.as_ref().and_then(|y| y.parse::<u16>().ok());

    // Get first label
    let label = result.label.as_ref().and_then(|l| l.first().cloned());

    // Get format as string
    let format = result.format.as_ref().map(|f| f.join(", "));

    RemoteAlbumSearchResult {
        provider: RemoteProvider::Discogs,
        provider_id: result.id.to_string(),
        title,
        artist,
        year,
        track_count: None,
        country: result.country.clone(),
        label,
        catalog_number: result.catno.clone(),
        confidence: None,
        format,
    }
}

/// Convert MusicBrainz full release to unified metadata DTO
pub fn musicbrainz_full_to_metadata(
    release: &crate::musicbrainz::ReleaseFullResponse,
) -> RemoteAlbumMetadata {
    let artist_credits = musicbrainz_credits(release.artist_credit.as_ref());
    let artist = display_credit(&artist_credits);

    // Extract year from date
    let year = release
        .date
        .as_ref()
        .and_then(|d| d.split('-').next().and_then(|y| y.parse::<u16>().ok()));

    // Extract genres from tags (sorted by count, take top 5)
    let genres: Vec<String> = release
        .tags
        .as_ref()
        .map(|tags| {
            let mut sorted: Vec<_> = tags.iter().collect();
            sorted.sort_by(|a, b| b.count.cmp(&a.count));
            sorted.iter().take(5).map(|t| t.name.clone()).collect()
        })
        .unwrap_or_default();

    // Extract label and catalog number
    let (label, catalog_number) = release
        .label_info
        .as_ref()
        .and_then(|info| info.first())
        .map(|li| {
            (
                li.label.as_ref().map(|l| l.name.clone()),
                li.catalog_number.clone(),
            )
        })
        .unwrap_or((None, None));

    // Count discs
    let disc_count = release.media.as_ref().map(|m| m.len() as u8).unwrap_or(1);

    // Convert tracks
    let tracks: Vec<RemoteTrackMetadata> = release
        .media
        .as_ref()
        .map(|media| {
            let mut all_tracks = Vec::new();
            for medium in media {
                if let Some(tracks) = &medium.tracks {
                    for track in tracks {
                        let track_credits = track
                            .recording
                            .as_ref()
                            .map(|recording| musicbrainz_credits(recording.artist_credit.as_ref()))
                            .filter(|credits| !credits.is_empty())
                            .unwrap_or_else(|| artist_credits.clone());
                        all_tracks.push(RemoteTrackMetadata {
                            disc_number: medium.position.unwrap_or(1),
                            track_number: track.position.unwrap_or(1),
                            title: track
                                .title
                                .clone()
                                .or_else(|| track.recording.as_ref().and_then(|r| r.title.clone()))
                                .unwrap_or_default(),
                            duration_ms: track.length.map(|l| l as u32).or_else(|| {
                                track
                                    .recording
                                    .as_ref()
                                    .and_then(|r| r.length.map(|l| l as u32))
                            }),
                            artist_credit: display_credit(&track_credits),
                            artist_credits: track_credits,
                            recording_id: track
                                .recording
                                .as_ref()
                                .map(|recording| recording.id.clone()),
                            track_id: track.id.clone(),
                        });
                    }
                }
            }
            all_tracks
        })
        .unwrap_or_default();

    RemoteAlbumMetadata {
        provider: RemoteProvider::MusicBrainz,
        provider_id: release.id.clone(),
        title: release.title.clone(),
        artist,
        artist_credits,
        year,
        genres,
        label,
        catalog_number,
        country: release.country.clone(),
        barcode: release.barcode.clone(),
        tracks,
        disc_count,
        source_url: Some(format!("https://musicbrainz.org/release/{}", release.id)),
        release_group_id: release.release_group.as_ref().map(|group| group.id.clone()),
    }
}

/// Convert Discogs full release to unified metadata DTO
pub fn discogs_full_to_metadata(
    release: &crate::discogs::DiscogsReleaseMetadata,
) -> RemoteAlbumMetadata {
    let artist_credits = release
        .artists
        .as_ref()
        .map(|artists| {
            artists
                .iter()
                .map(|artist| RemoteArtistCredit {
                    name: artist.name.clone(),
                    join_phrase: artist.join.clone().unwrap_or_default(),
                    provider_id: artist.id.map(|id| id.to_string()),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let artist = display_credit(&artist_credits);

    // Combine genres and styles
    let genres: Vec<String> = {
        let mut combined = Vec::new();
        if let Some(g) = &release.genres {
            combined.extend(g.clone());
        }
        if let Some(s) = &release.styles {
            combined.extend(s.clone());
        }
        combined
    };

    // Get first label and catalog number
    let (label, catalog_number) = release
        .labels
        .as_ref()
        .and_then(|labels| labels.first())
        .map(|l| (Some(l.name.clone()), l.catno.clone()))
        .unwrap_or((None, None));

    // Convert tracklist
    let tracks: Vec<RemoteTrackMetadata> = release
        .tracklist
        .as_ref()
        .map(|tracklist| {
            tracklist
                .iter()
                .filter(|t| {
                    // Filter out headings (disc separators)
                    t.track_type.as_deref() != Some("heading")
                })
                .map(|t| {
                    let (disc_number, track_number) = parse_discogs_position(&t.position);
                    RemoteTrackMetadata {
                        disc_number,
                        track_number,
                        title: t.title.clone(),
                        duration_ms: t.duration.as_ref().and_then(|d| parse_discogs_duration(d)),
                        artist_credit: artist.clone(),
                        artist_credits: artist_credits.clone(),
                        recording_id: None,
                        track_id: None,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Count unique discs
    let disc_count = tracks.iter().map(|t| t.disc_number).max().unwrap_or(1);

    RemoteAlbumMetadata {
        provider: RemoteProvider::Discogs,
        provider_id: release.id.to_string(),
        title: release.title.clone(),
        artist,
        artist_credits,
        year: release.year.map(|y| y as u16),
        genres,
        label,
        catalog_number,
        country: release.country.clone(),
        barcode: None, // Discogs doesn't include barcode in release details
        tracks,
        disc_count,
        source_url: release.uri.clone(),
        release_group_id: None,
    }
}

// ---------------------------------------------------------------------------
// Orchestration — "ask a provider what this record is"
// ---------------------------------------------------------------------------
//
// The converters above are pure; these two are the thin async layer that puts
// a client in front of them. They live HERE rather than in the frontend for
// the reason ADR-006 gives: which provider answers, how a search query is
// built and what an empty answer means are domain decisions, and a second
// frontend must not have to re-derive them.
//
// The Tauri reference is `src-tauri/src/commands_v2/integrations.rs`
// (`v2_remote_metadata_search` / `v2_remote_metadata_get_album`). This is that
// orchestration with its Tauri `State` arguments removed — the clients are
// created per call, because both are thin wrappers over a `reqwest::Client`
// and MusicBrainz's rate limiter lives inside its own client.

/// Search one provider. Never panics, never blocks; a provider that is
/// disabled, rate-limited or simply silent answers with an EMPTY result set
/// rather than an error, because "nobody has this record" and "the server said
/// no" look identical to a user staring at a list — and the distinction that
/// does matter (rate limiting) rides its own flag.
pub async fn search(request: &RemoteSearchRequest) -> RemoteSearchResponse {
    let limit = request.limit();
    match request.provider {
        RemoteProvider::MusicBrainz => {
            let client = crate::musicbrainz::MusicBrainzClient::new();
            // The query is "artist album" by convention (the caller builds
            // it), but MusicBrainz wants the two apart. Splitting on the
            // caller's own `artist` hint is exact when it has one, and the
            // whole string is a title otherwise.
            let artist = request.artist.clone().unwrap_or_default();
            let title = strip_prefix_ci(&request.query, &artist);
            match client
                .search_releases_extended(&title, &artist, request.catalog_id.as_deref(), limit)
                .await
            {
                Ok(found) => RemoteSearchResponse {
                    provider: RemoteProvider::MusicBrainz,
                    results: found
                        .releases
                        .iter()
                        .map(musicbrainz_release_to_search_result)
                        .collect(),
                    total_count: usize::try_from(found.count).ok(),
                    rate_limited: false,
                },
                Err(e) => {
                    log::warn!("[remote-metadata] musicbrainz search failed: {e}");
                    empty(RemoteProvider::MusicBrainz, is_rate_limit(&e.to_string()))
                }
            }
        }
        RemoteProvider::Discogs => {
            let client = crate::discogs::DiscogsClient::new();
            let artist = request.artist.clone().unwrap_or_default();
            let title = strip_prefix_ci(&request.query, &artist);
            match client
                .search_releases(&artist, &title, request.catalog_id.as_deref(), limit)
                .await
            {
                Ok(found) => RemoteSearchResponse {
                    provider: RemoteProvider::Discogs,
                    total_count: Some(found.len()),
                    results: found
                        .iter()
                        .map(discogs_extended_to_search_result)
                        .collect(),
                    rate_limited: false,
                },
                Err(e) => {
                    log::warn!("[remote-metadata] discogs search failed: {e}");
                    empty(RemoteProvider::Discogs, is_rate_limit(&e))
                }
            }
        }
    }
}

/// Fetch one release in full, with its track list.
pub async fn get_album(
    provider: RemoteProvider,
    provider_id: &str,
) -> Result<RemoteAlbumMetadata, RemoteMetadataError> {
    match provider {
        RemoteProvider::MusicBrainz => {
            let client = crate::musicbrainz::MusicBrainzClient::new();
            let full = client
                .get_release_with_tracks(provider_id)
                .await
                .map_err(|e| RemoteMetadataError::ProviderUnavailable(e.to_string()))?;
            Ok(musicbrainz_full_to_metadata(&full))
        }
        RemoteProvider::Discogs => {
            // A Discogs release id is numeric. Saying so beats sending a
            // MusicBrainz UUID to Discogs and reporting whatever it answers.
            let id: u64 = provider_id
                .trim()
                .parse()
                .map_err(|_| RemoteMetadataError::InvalidProviderId(provider_id.to_string()))?;
            let client = crate::discogs::DiscogsClient::new();
            let full = client
                .get_release_metadata(id)
                .await
                .map_err(RemoteMetadataError::ProviderUnavailable)?;
            Ok(discogs_full_to_metadata(&full))
        }
    }
}

fn empty(provider: RemoteProvider, rate_limited: bool) -> RemoteSearchResponse {
    RemoteSearchResponse {
        provider,
        results: Vec::new(),
        total_count: None,
        rate_limited,
    }
}

/// A 429, however the provider spells it. Both clients hand back a formatted
/// string rather than a status, so this reads the string — narrowly, on the
/// two spellings that actually occur, rather than on the word "limit" which
/// appears in perfectly ordinary messages.
fn is_rate_limit(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("429") || m.contains("rate limit") || m.contains("too many requests")
}

/// `"Tool Fear Inoculum"` minus `"Tool"` = `"Fear Inoculum"`.
///
/// Case-insensitive and prefix-only: a band whose name also appears inside the
/// album title ("Black Sabbath — Black Sabbath") must lose exactly one copy,
/// and only the leading one.
fn strip_prefix_ci(query: &str, artist: &str) -> String {
    let q = query.trim();
    let a = artist.trim();
    if a.is_empty() || q.len() < a.len() {
        return q.to_string();
    }
    if q[..a.len()].eq_ignore_ascii_case(a) {
        return q[a.len()..].trim().to_string();
    }
    q.to_string()
}

#[cfg(test)]
mod orchestration_tests {
    use super::*;

    #[test]
    fn the_artist_is_taken_off_the_front_of_the_query_once() {
        assert_eq!(
            strip_prefix_ci("Tool Fear Inoculum", "Tool"),
            "Fear Inoculum"
        );
        assert_eq!(
            strip_prefix_ci("Black Sabbath Black Sabbath", "Black Sabbath"),
            "Black Sabbath",
            "only the LEADING copy comes off"
        );
        assert_eq!(strip_prefix_ci("tool Lateralus", "Tool"), "Lateralus");
    }

    #[test]
    fn a_query_that_does_not_start_with_the_artist_is_left_alone() {
        assert_eq!(strip_prefix_ci("Fear Inoculum", "Tool"), "Fear Inoculum");
        assert_eq!(strip_prefix_ci("Fear Inoculum", ""), "Fear Inoculum");
        // Shorter than the artist: the slice would panic, so it must not be taken.
        assert_eq!(strip_prefix_ci("Up", "Peter Gabriel"), "Up");
    }

    #[test]
    fn rate_limiting_is_recognised_without_swallowing_ordinary_messages() {
        assert!(is_rate_limit("HTTP 429 Too Many Requests"));
        assert!(is_rate_limit("rate limit exceeded"));
        assert!(!is_rate_limit("Failed to parse Discogs response"));
        assert!(
            !is_rate_limit("limit must be between 1 and 25"),
            "the bare word `limit` is not a 429"
        );
    }

    #[test]
    fn musicbrainz_converter_keeps_display_credits_components_and_ids_separate() {
        let release: crate::musicbrainz::ReleaseFullResponse = serde_json::from_value(
            serde_json::json!({
                "id": "release-id",
                "title": "Collaborations",
                "artist-credit": [
                    {"name": "Alpha", "joinphrase": " feat. ", "artist": {"id": "alpha-id", "name": "Alpha"}},
                    {"name": "Beta", "joinphrase": "", "artist": {"id": "beta-id", "name": "Beta"}}
                ],
                "release-group": {"id": "group-id", "primary-type": "Album"},
                "media": [{
                    "position": 1,
                    "tracks": [{
                        "id": "track-id",
                        "position": 1,
                        "title": "A Song",
                        "recording": {
                            "id": "recording-id",
                            "title": "A Song",
                            "artist-credit": [
                                {"name": "Gamma", "joinphrase": "", "artist": {"id": "gamma-id", "name": "Gamma"}}
                            ]
                        }
                    }]
                }]
            }),
        )
        .unwrap();

        let metadata = musicbrainz_full_to_metadata(&release);
        assert_eq!(metadata.artist, "Alpha feat. Beta");
        assert_eq!(metadata.release_group_id.as_deref(), Some("group-id"));
        assert_eq!(
            metadata.artist_credits[0].provider_id.as_deref(),
            Some("alpha-id")
        );
        assert_eq!(metadata.tracks[0].artist_credit, "Gamma");
        assert_eq!(
            metadata.tracks[0].recording_id.as_deref(),
            Some("recording-id")
        );
        assert_eq!(metadata.tracks[0].track_id.as_deref(), Some("track-id"));
        assert_eq!(
            metadata.tracks[0].artist_credits[0].provider_id.as_deref(),
            Some("gamma-id")
        );
    }
}
