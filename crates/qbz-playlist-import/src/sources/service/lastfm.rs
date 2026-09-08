//! Last.fm — the three player stations, and a user playlist page.
//!
//! SCRAPER CLASS, and treated as such: undocumented endpoints and, for one of
//! the two paths, page HTML. Every parse is defensive — a shape change yields
//! fewer rows or an empty list, never a panic and never a hard failure that
//! blames the user. Same discipline as `providers/apple.rs`.
//!
//! No authentication and no Last.fm API key. Both paths are public reads, which
//! is why this module does NOT reuse `qbz-integrations`'s scrobble client: that
//! one exists to WRITE with the user's credentials, and importing a public
//! playlist must not need them.
//!
//! | input | what it means |
//! |---|---|
//! | `last.fm/user/<u>/playlists/<id>` | that playlist, imported directly |
//! | `last.fm/user/<u>` or a bare `<u>` | pick one of three stations |
//!
//! The stations are JSON (`/player/station/user/<u>/<slug>?ajax=1`); the
//! specific playlist is NOT — the ajax path 404s for a playlist id, so its
//! tracklist comes out of the page's `chartlist` table.

use serde_json::Value;

use super::super::LastFmStation;
use super::BROWSER_USER_AGENT;
use crate::errors::PlaylistImportError;
use crate::http::http;
use crate::models::{ImportPlaylist, ImportProvider, ImportTrack};

const BASE: &str = "https://www.last.fm";
/// Hard request cap for a station. It is a BACKSTOP, not the normal exit — see
/// `fetch_station`, which stops as soon as the pool stops yielding new tracks.
const MAX_PAGES: usize = 20;
/// Consecutive all-duplicate draws that mean the pool is exhausted. Two, not
/// one: a radio can repeat a whole batch by chance.
const MAX_DRY_DRAWS: usize = 2;
/// HOW MANY TRACKS A STATION IS ALLOWED TO PROMISE.
///
/// This is the number the user sees in "Found N tracks" before they press
/// Import, so it has to be a number we can actually deliver. A station is a
/// radio: the pool is effectively unbounded and every extra draw returns mostly
/// repeats, so "keep pulling until it runs dry" quietly turns into "pull 20
/// times and announce 469 tracks" for a playlist that lands at 198.
///
/// 200 is the honest bound for a DYNAMIC source — the station is different
/// every day, so the value of a bigger single import is low and the cost (draw
/// after draw of duplicates, Last.fm rate-limiting the burst, hundreds of
/// wasted Qobuz searches) is real. Owner ruling 2026-08-19: "no debemos dar
/// falsas expectativas de 'traere casi 500 tracks' cuando solo podemos traer la
/// mitad."
const MAX_STATION_TRACKS: usize = 200;

/// What a pasted string means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastFmTarget {
    Profile { user: String },
    Playlist { user: String, id: String },
}

/// Cheap, synchronous, safe per keystroke.
///
/// PLAYLIST IS CHECKED FIRST. A playlist URL contains a profile URL as a
/// prefix, so testing the profile shape first would classify every playlist as
/// a profile and silently show the station picker instead of importing.
pub fn detect(input: &str) -> Option<LastFmTarget> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains("last.fm") {
        let after_user = s.split("/user/").nth(1)?;
        let mut parts = after_user.split(['?', '#']).next().unwrap_or("").split('/');
        let user = parts.next().unwrap_or("").trim();
        if !is_username(user) {
            return None;
        }
        if parts.next() == Some("playlists") {
            if let Some(id) = parts.next().map(str::trim).filter(|i| !i.is_empty()) {
                return Some(LastFmTarget::Playlist {
                    user: user.to_string(),
                    id: id.to_string(),
                });
            }
        }
        return Some(LastFmTarget::Profile {
            user: user.to_string(),
        });
    }
    if is_username(s) {
        return Some(LastFmTarget::Profile {
            user: s.to_string(),
        });
    }
    None
}

fn is_username(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// One of the three stations.
///
/// # `?page=N` IS NOT PAGINATION, and treating it as such was a real bug
///
/// A station is a RADIO. Every request is a fresh draw from the same
/// recommendation pool, not the next slice of an ordered list — measured
/// against a live profile on 2026-08-19: pages 1 and 2 returned overlapping
/// tracks, and the batch sizes varied (20, 20, 36) instead of being a stable
/// page length.
///
/// So the original loop — "fetch 20 pages, stop at the first empty one" — never
/// hit its exit condition, always burned all 20 requests (Last.fm starts
/// refusing after about four in quick succession), and returned a pile of
/// duplicates. One real import produced 469 rows holding 198 distinct tracks;
/// the other 271 were reported to the user as "skipped", which is not what
/// happened to them.
///
/// The fix is to stop counting requests and start counting NEW tracks:
/// deduplicate as we draw, stop at [`MAX_STATION_TRACKS`], and give up early
/// after [`MAX_DRY_DRAWS`] consecutive draws that add nothing. [`MAX_PAGES`]
/// stays as a backstop on the request count.
///
/// The count this returns IS the count the user is shown and IS the number of
/// rows the import will try to add. Those three used to be different numbers.
pub async fn fetch_station(
    user: &str,
    station: LastFmStation,
) -> Result<ImportPlaylist, PlaylistImportError> {
    let mut tracks: Vec<ImportTrack> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut dry = 0usize;
    let mut draws = 0usize;

    for page in 1..=MAX_PAGES {
        let url = format!(
            "{BASE}/player/station/user/{user}/{}?ajax=1&page={page}",
            station.slug()
        );
        // A FAILED DRAW IS NOT A FAILED IMPORT once we have rows. Last.fm rate-
        // limits a burst of these, and propagating that would throw away
        // everything already collected — which is exactly what `?` used to do.
        let body = match get_text(&url).await {
            Ok(b) => b,
            Err(e) if !tracks.is_empty() => {
                log::warn!("[qbz-playlist-import] last.fm station draw {page} failed: {e}");
                break;
            }
            Err(e) => return Err(e),
        };
        draws += 1;
        let doc: Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            // A shape change (or a rate-limit HTML body) must not fail an
            // import that already has rows.
            Err(e) => {
                log::warn!("[qbz-playlist-import] last.fm station draw {page}: {e}");
                break;
            }
        };
        let drawn: Vec<ImportTrack> = doc["playlist"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(map_station_entry)
            .collect();
        if drawn.is_empty() {
            break;
        }
        let before = tracks.len();
        for t in drawn {
            if tracks.len() >= MAX_STATION_TRACKS {
                break;
            }
            let key = (t.artist.to_lowercase(), t.title.to_lowercase());
            if seen.insert(key) {
                tracks.push(t);
            }
        }
        if tracks.len() >= MAX_STATION_TRACKS {
            log::info!(
                "[qbz-playlist-import] last.fm station reached its {MAX_STATION_TRACKS}-track cap"
            );
            break;
        }
        if tracks.len() == before {
            dry += 1;
            if dry >= MAX_DRY_DRAWS {
                break;
            }
        } else {
            dry = 0;
        }
    }
    log::info!(
        "[qbz-playlist-import] last.fm station {}/{}: {} distinct tracks in {draws} draw(s)",
        user,
        station.slug(),
        tracks.len()
    );
    if tracks.is_empty() {
        return Err(PlaylistImportError::EmptyPlaylist);
    }
    Ok(ImportPlaylist {
        provider: ImportProvider::LastFm,
        provider_id: format!("{user}/{}", station.slug()),
        name: format!("{user} — {}", station_label(station)),
        description: None,
        tracks,
    })
}

fn station_label(station: LastFmStation) -> &'static str {
    // Not localized: it is part of a NAME the playlist is created under, and a
    // playlist should not be renamed by changing the app's language.
    match station {
        LastFmStation::Library => "Library",
        LastFmStation::Mix => "Mix",
        LastFmStation::Recommended => "Recommendations",
    }
}

fn map_station_entry(e: &Value) -> Option<ImportTrack> {
    let title = e["_name"]
        .as_str()
        .or_else(|| e["name"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }
    let artist = e["artists"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["_name"].as_str().or_else(|| x["name"].as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let path = e["url"].as_str().unwrap_or("");
    Some(ImportTrack {
        title,
        artist,
        album: e["primary_album"]["name"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        // The station feed's `duration` is SECONDS.
        duration_ms: e["duration"].as_u64().filter(|d| *d > 0).map(|d| d * 1000),
        isrc: None,
        provider_id: path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        provider_url: (!path.is_empty()).then(|| format!("{BASE}{path}")),
    })
}

/// A specific user playlist, scraped from its page.
pub async fn fetch_playlist(
    user: &str,
    playlist_id: &str,
) -> Result<ImportPlaylist, PlaylistImportError> {
    let mut tracks: Vec<ImportTrack> = Vec::new();
    let mut name = format!("{user} — Playlist");
    for page in 1..=MAX_PAGES {
        let url = format!("{BASE}/user/{user}/playlists/{playlist_id}?page={page}");
        let html = get_text(&url).await?;
        if page == 1 {
            if let Some(t) = scrape_title(&html) {
                name = t;
            }
        }
        let page_tracks = scrape_chartlist(&html);
        if page_tracks.is_empty() {
            break;
        }
        tracks.extend(page_tracks);
    }
    if tracks.is_empty() {
        return Err(PlaylistImportError::EmptyPlaylist);
    }
    Ok(ImportPlaylist {
        provider: ImportProvider::LastFm,
        provider_id: format!("{user}/{playlist_id}"),
        name,
        description: None,
        tracks,
    })
}

/// The rows of a Last.fm `chartlist` table.
///
/// It reads the `data-track-name` / `data-artist-name` ATTRIBUTES rather than
/// the visible cell text, and that is not a style choice: the visible text is
/// truncated with an ellipsis at the column width, so scraping it would import
/// "Everything In Its Right Pla…" and lose the match. The attributes carry the
/// full strings.
fn scrape_chartlist(html: &str) -> Vec<ImportTrack> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = html[cursor..].find("data-track-name=\"") {
        let start = cursor + rel;
        let Some(title) = attr_value(html, start, "data-track-name=\"") else {
            cursor = start + 17;
            continue;
        };
        // The artist attribute lives on the same element; look ahead a bounded
        // window rather than scanning the rest of the document.
        let window_end = (start + 2048).min(html.len());
        let artist = html[start..window_end]
            .find("data-artist-name=\"")
            .and_then(|r| attr_value(html, start + r, "data-artist-name=\""))
            .unwrap_or_default();
        if !title.trim().is_empty() {
            out.push(ImportTrack {
                title: decode_entities(title.trim()),
                artist: decode_entities(artist.trim()),
                album: None,
                duration_ms: None,
                isrc: None,
                provider_id: None,
                provider_url: None,
            });
        }
        cursor = start + 17;
    }
    out
}

fn attr_value(html: &str, at: usize, attr: &str) -> Option<String> {
    let rest = &html[at + attr.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn scrape_title(html: &str) -> Option<String> {
    let start = html.find("<title>")? + 7;
    let end = html[start..].find("</title>")? + start;
    let raw = decode_entities(html[start..end].trim());
    // Last.fm titles read "<Playlist> | Last.fm".
    let cleaned = raw.split(" | ").next().unwrap_or(&raw).trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The five XML entities plus `&#39;`. A full HTML entity table is not worth a
/// dependency for track titles.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

async fn get_text(url: &str) -> Result<String, PlaylistImportError> {
    let resp = http()
        .get(url)
        // The crate's shared client sends no User-Agent and Last.fm's CDN 403s
        // that. Per-request, so nothing else on the shared client changes.
        .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
        .send()
        .await
        .map_err(|e| PlaylistImportError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(PlaylistImportError::Http(format!(
            "Last.fm returned {}",
            resp.status()
        )));
    }
    resp.text()
        .await
        .map_err(|e| PlaylistImportError::Http(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playlist_url_is_never_read_as_a_profile() {
        assert_eq!(
            detect("https://www.last.fm/user/rob/playlists/12345678"),
            Some(LastFmTarget::Playlist {
                user: "rob".into(),
                id: "12345678".into()
            })
        );
        assert_eq!(
            detect("https://www.last.fm/user/rob"),
            Some(LastFmTarget::Profile { user: "rob".into() })
        );
        assert_eq!(
            detect("https://www.last.fm/user/rob/library"),
            Some(LastFmTarget::Profile { user: "rob".into() })
        );
        assert_eq!(
            detect("  rob-2  "),
            Some(LastFmTarget::Profile { user: "rob-2".into() })
        );
        assert_eq!(detect(""), None);
        assert_eq!(detect("https://example.com/user/rob"), None);
    }

    #[test]
    fn a_station_entry_maps_and_its_duration_is_seconds() {
        let e: Value = serde_json::from_str(
            r#"{"_name":"Song","artists":[{"_name":"A"},{"_name":"B"}],
                "primary_album":{"name":"Al"},"duration":214,
                "url":"/music/A/_/Song"}"#,
        )
        .unwrap();
        let t = map_station_entry(&e).unwrap();
        assert_eq!(t.title, "Song");
        assert_eq!(t.artist, "A, B");
        assert_eq!(t.album.as_deref(), Some("Al"));
        // 214 SECONDS -> ms.
        assert_eq!(t.duration_ms, Some(214_000));
        assert_eq!(t.provider_url.as_deref(), Some("https://www.last.fm/music/A/_/Song"));
    }

    #[test]
    fn a_station_entry_without_a_name_is_dropped() {
        let e: Value = serde_json::from_str(r#"{"artists":[{"_name":"A"}]}"#).unwrap();
        assert!(map_station_entry(&e).is_none());
    }

    #[test]
    fn chartlist_rows_come_from_the_attributes_not_the_truncated_text() {
        let html = r#"
        <tr class="chartlist-row">
          <td class="chartlist-name">Everything In Its Right Pla…</td>
          <button data-track-name="Everything In Its Right Place"
                  data-artist-name="Radiohead">Play</button>
        </tr>
        <tr class="chartlist-row">
          <button data-track-name="Sigur R&amp;#39;s" data-artist-name="Sigur R&amp;s">Play</button>
        </tr>"#;
        let rows = scrape_chartlist(html);
        assert_eq!(rows.len(), 2);
        // The FULL title, not the ellipsized cell text.
        assert_eq!(rows[0].title, "Everything In Its Right Place");
        assert_eq!(rows[0].artist, "Radiohead");
        // Entities are decoded.
        assert_eq!(rows[1].artist, "Sigur R&s");
    }

    /// The draw loop's bookkeeping, isolated from the network: a radio hands
    /// back overlapping batches, and what comes out must be distinct and
    /// bounded by the cap the user is promised.
    #[test]
    fn overlapping_draws_dedupe_and_stop_at_the_cap() {
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        let mut kept = 0usize;
        // 40 draws of 30 entries each, cycling through a 250-track pool —
        // the shape a station actually has.
        for draw in 0..40 {
            for i in 0..30 {
                if kept >= MAX_STATION_TRACKS {
                    break;
                }
                let idx = (draw * 7 + i) % 250;
                if seen.insert(("band".into(), format!("song{idx}"))) {
                    kept += 1;
                }
            }
        }
        assert_eq!(kept, MAX_STATION_TRACKS, "the cap is what bounds the result");
        assert_eq!(seen.len(), kept, "nothing repeats in the output");
    }

    #[test]
    fn an_html_page_with_no_chartlist_yields_no_rows_rather_than_failing() {
        assert!(scrape_chartlist("<html><body>nothing here</body></html>").is_empty());
    }

    #[test]
    fn the_page_title_is_cleaned_of_the_site_suffix() {
        assert_eq!(
            scrape_title("<html><head><title>Road Trip | Last.fm</title></head>").as_deref(),
            Some("Road Trip")
        );
    }
}
