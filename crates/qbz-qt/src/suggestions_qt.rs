//! Immersive Suggestions panel controller (split-only, split-panel == 2) —
//! the Qt port of `crates/qbz/src/suggestions.rs` + the `SuggestionsActions`
//! handlers (`main.rs:16684-16845`), per the immersive-port contract
//! (`qbz-nix-docs/qt-frontend/2026-08-02-immersive-port/00-CONTRACT.md` §4.5,
//! block B4).
//!
//! The loader ports `suggestions::load_suggestions` 1:1 — SAME client calls
//! (`get_artist_detail` with the playlists/tracks_appears_on extras, the
//! `get_artist_tracks` sparse fallback, `get_playlist` for the book-collage
//! covers), same constants (REC 10 / SPARSE 5 / FALLBACK 30 / 2 playlist
//! cards / 3 book covers / 4 radio covers), same deterministic splitmix64
//! shuffle seeded `(artist_id << 1) ^ (track_id + 1)`. Two data products:
//!
//!   * RECOMMENDED TRACKS — `artist.tracks_appears_on`, sparse-merged with
//!     artist popular tracks, deduped by lowercase title, the current track
//!     filtered out, shuffled, take 10.
//!   * CARDS — the first 2 curated `artist.playlists` (book collage) + ONE
//!     seed "Song Radio" card (diamond collage) trailing them (Tauri order).
//!
//! The document holds REMOTE cover URLs only (the §4.4 coverflowJson
//! precedent): QML resolves them through the shared artwork pipeline
//! (`QbzShell.sidebarArtworkWindow` + `QbzLibrary.libraryArtworkReady`), so
//! the Slint artwork-job half (`suggestions_artwork_jobs` + the cover0..3
//! decoded-image slots) has no Qt twin.
//!
//! The Slint top-level `artist-id` / `seed-track-id` state fields stay
//! RUST-SIDE here: they exist for the load dedup guard
//! (`main.rs:16703-16712`), which is exactly what LOADED_IDS carries. The
//! published document is the §4.5 shape only — the panel derives
//! loading / error / empty from `{loading, error, cards, tracks}` alone.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cxx_qt_lib::QString;
use qbz_app::shell::AppRuntime;
use qbz_core::LoggingAdapter;
use serde::Serialize;

/// Recommended-track target count (Tauri `slice(0, 10)`).
const REC_LIMIT: usize = 10;
/// Sparse threshold below which the artist-tracks fallback runs (Tauri `< 5`).
const SPARSE_THRESHOLD: usize = 5;
/// Artist-tracks fallback page size (Tauri `limit: 30`).
const FALLBACK_LIMIT: u32 = 30;
/// Max curated playlist cards (Tauri `slice(0, 2)`).
const MAX_PLAYLIST_CARDS: usize = 2;
/// Book-collage cover count per playlist card (Tauri 3).
const BOOK_COVERS: usize = 3;
/// Diamond-collage cover count for the radio card (Tauri max 4).
const RADIO_COVERS: usize = 4;

// ---------------------------------------------------------------------------
// The §4.5 document (EXACT Slint field set, translated)
// ---------------------------------------------------------------------------

/// One suggestion card (`state.slint:872-894 SuggestionCard`). `coverUrls`
/// carries the (up to 4) remote collage URLs — Slint's `cover-urls` input
/// array; the decoded cover0..3 slots are the Slint artwork pipeline's
/// business, not the document's.
#[derive(Clone, Default, Serialize)]
pub struct CardDoc {
    /// "playlist" | "radio".
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    #[serde(rename = "coverUrls")]
    pub cover_urls: Vec<String>,
    /// Playlist-card target id ("" for the radio card).
    #[serde(rename = "playlistId")]
    pub playlist_id: String,
    /// Radio-card seed (empty for playlist cards).
    #[serde(rename = "seedTrackId")]
    pub seed_track_id: String,
    #[serde(rename = "seedTrackName")]
    pub seed_track_name: String,
    #[serde(rename = "seedArtistId")]
    pub seed_artist_id: String,
    /// "qobuz" (playlist) | "qbz" (radio) — drives the corner badge glyph.
    pub badge: String,
    /// Radio card: true while the Song Radio session is building (spinner).
    pub loading: bool,
}

/// One recommended track row (the §4.5 translation of the Slint TrackItem
/// fields the panel reads: id/title/artist/duration/artwork, plus artistId
/// and explicit per the contract shape).
#[derive(Clone, Default, Serialize)]
pub struct TrackDoc {
    pub id: String,
    pub title: String,
    pub artist: String,
    #[serde(rename = "artistId")]
    pub artist_id: String,
    /// Pre-formatted "m:ss" (Slint `mmss(track.duration)`).
    pub duration: String,
    #[serde(rename = "artUrl")]
    pub art_url: String,
    pub explicit: bool,
}

/// The §4.5 top-level shape. `error` is a BOOL (the contract's translation
/// of Slint's `error: "" | "error"` string); `loading` mirrors
/// `SuggestionsState.loading`.
#[derive(Clone, Default, Serialize)]
pub struct SuggestionsDoc {
    pub loading: bool,
    pub error: bool,
    pub cards: Vec<CardDoc>,
    pub tracks: Vec<TrackDoc>,
}

/// The live document + the dedup identity. One Mutex for both so the guard
/// and the doc can never disagree.
#[derive(Default)]
struct State {
    doc: SuggestionsDoc,
    /// (artist-id, seed-track-id) of the last ACCEPTED load — the dedup
    /// guard of `main.rs:16703-16712`. Empty pair = the no-track state.
    loaded: (String, String),
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut guard = STATE.lock().unwrap();
    f(guard.get_or_insert_with(State::default))
}

/// Serialize + hop the current document onto the bridge (Qt thread).
fn publish(doc: &SuggestionsDoc) {
    let json = serde_json::to_string(doc).unwrap_or_else(|_| {
        r#"{"loading":false,"error":false,"cards":[],"tracks":[]}"#.to_string()
    });
    crate::suggestions_bridge::ui(move |mut b| {
        b.as_mut()
            .set_suggestions_json(QString::from(json.as_str()));
    });
}

/// Mutate the stored document and republish.
fn mutate(f: impl FnOnce(&mut SuggestionsDoc)) {
    let doc = with_state(|s| {
        f(&mut s.doc);
        s.doc.clone()
    });
    publish(&doc);
}

// ---------------------------------------------------------------------------
// The loader (suggestions.rs:119-263, 1:1)
// ---------------------------------------------------------------------------

/// Deterministic splitmix64 step (qbz-radio's RNG family) — verbatim from
/// `suggestions.rs:80-86`; the shuffle MUST stay reproducible per
/// (artist, track) or the panel reshuffles on every reload.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Fisher-Yates shuffle seeded off the (artist, track) ids.
fn shuffle_tracks(tracks: &mut [qbz_models::Track], seed: u64) {
    let mut state = seed ^ 0xD1B54A32D192ED03;
    for i in (1..tracks.len()).rev() {
        let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
        tracks.swap(i, j);
    }
}

/// Collage cover URL for a track's album. Suggestion-card collage tiles draw
/// at <= ~144 CSS px (SuggestionsPanel wells); the full variant (best()) is
/// downscaled to the drawn size by the RoundedImage derivative layer — the
/// thumbnail down-tier was reverted after the 2026-08-15 owner smoke
/// (contract 04 §3).
fn track_album_cover(track: &qbz_models::Track) -> Option<String> {
    track
        .album
        .as_ref()
        .and_then(|a| a.image.best().cloned())
        .filter(|s| !s.is_empty())
}

/// Album id of a track (for distinct-cover dedupe in the book collage).
fn track_album_id(track: &qbz_models::Track) -> Option<String> {
    track
        .album
        .as_ref()
        .map(|a| a.id.clone())
        .filter(|s| !s.is_empty())
}

/// "m:ss" (Slint `mmss`, playlist.rs:630).
fn mmss(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// `Track` -> the §4.5 track row (the fields of `playlist::to_item` the
/// panel reads, plus `artistId` + `explicit` per the contract shape).
fn to_track_doc(track: &qbz_models::Track) -> TrackDoc {
    TrackDoc {
        id: track.id.to_string(),
        title: track.title.clone(),
        artist: track
            .performer
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_default(),
        artist_id: track
            .performer
            .as_ref()
            .map(|p| p.id.to_string())
            .unwrap_or_default(),
        duration: mmss(track.duration),
        art_url: track_album_cover(track).unwrap_or_default(),
        explicit: track.parental_warning,
    }
}

/// A resolved playlist card (book collage of up to 3 distinct album covers).
struct PlaylistCard {
    id: String,
    name: String,
    track_count: u32,
    cover_urls: Vec<String>,
}

/// Build a `CardDoc` for a playlist (book collage) — `playlist_to_card`,
/// `suggestions.rs:266-294`.
fn playlist_to_card(card: &PlaylistCard) -> CardDoc {
    CardDoc {
        kind: "playlist".to_string(),
        title: card.name.clone(),
        subtitle: qbz_i18n::tf(
            "{} track",
            "{} tracks",
            card.track_count as i64,
            &[&card.track_count.to_string()],
        ),
        cover_urls: card.cover_urls.clone(),
        playlist_id: card.id.clone(),
        seed_track_id: String::new(),
        seed_track_name: String::new(),
        seed_artist_id: String::new(),
        badge: "qobuz".to_string(),
        loading: false,
    }
}

/// Build the seed "Song Radio" card (diamond collage) — `radio_card`,
/// `suggestions.rs:297-319`. The seed triple comes from the payload.
fn radio_card(
    seed_track_id: &str,
    seed_track_name: &str,
    seed_artist_id: &str,
    radio_cover_urls: &[String],
) -> CardDoc {
    CardDoc {
        kind: "radio".to_string(),
        title: qbz_i18n::t("Song Radio"),
        subtitle: seed_track_name.to_string(),
        cover_urls: radio_cover_urls.to_vec(),
        playlist_id: String::new(),
        seed_track_id: seed_track_id.to_string(),
        seed_track_name: seed_track_name.to_string(),
        seed_artist_id: seed_artist_id.to_string(),
        badge: "qbz".to_string(),
        loading: false,
    }
}

/// `load_suggestions` (`suggestions.rs:119-263`): build the (cards, tracks)
/// pair for `artist_id` + `current_track_id`. On the top-level
/// artist-detail failure returns `Err` (drives the panel's error branch);
/// individual playlist-cover fetch failures are tolerated.
async fn load_suggestions(
    runtime: &Arc<AppRuntime<LoggingAdapter>>,
    artist_id: u64,
    current_track_id: u64,
    seed_track_name: &str,
) -> Result<SuggestionsDoc, ()> {
    let artist = match runtime
        .core()
        .get_artist_detail(artist_id, None, None)
        .await
    {
        Ok(a) => a,
        Err(e) => {
            log::error!("[qbz-qt] suggestions get_artist_detail({artist_id}) failed: {e}");
            return Err(());
        }
    };

    // ---- Recommended tracks --------------------------------------------
    // Base = tracks_appears_on (current track filtered, deduped by title).
    let mut rec: Vec<qbz_models::Track> = Vec::new();
    let mut seen_titles: HashSet<String> = HashSet::new();
    if let Some(container) = artist.tracks_appears_on.as_ref() {
        for track in &container.items {
            if track.id == current_track_id {
                continue;
            }
            let key = track.title.to_lowercase().trim().to_string();
            if key.is_empty() || !seen_titles.insert(key) {
                continue;
            }
            rec.push(track.clone());
        }
    }

    // Sparse fallback: merge artist popular tracks (dedupe by title + id).
    if rec.len() < SPARSE_THRESHOLD {
        match runtime
            .core()
            .get_artist_tracks(artist_id, FALLBACK_LIMIT, 0)
            .await
        {
            Ok(container) => {
                let existing_ids: HashSet<u64> = rec.iter().map(|t| t.id).collect();
                for track in container.items {
                    if track.id == current_track_id || existing_ids.contains(&track.id) {
                        continue;
                    }
                    let key = track.title.to_lowercase().trim().to_string();
                    if key.is_empty() || !seen_titles.insert(key) {
                        continue;
                    }
                    rec.push(track);
                }
            }
            Err(e) => log::warn!("[qbz-qt] suggestions artist-tracks fallback failed: {e}"),
        }
    }

    // Shuffle (deterministic per artist+track), take 10.
    let seed = (artist_id << 1) ^ current_track_id.wrapping_add(1);
    shuffle_tracks(&mut rec, seed);
    rec.truncate(REC_LIMIT);

    // Radio diamond collage: up to 4 distinct rec-track album covers.
    let mut radio_cover_urls: Vec<String> = Vec::new();
    for track in &rec {
        if let Some(url) = track_album_cover(track) {
            if !radio_cover_urls.contains(&url) {
                radio_cover_urls.push(url);
                if radio_cover_urls.len() >= RADIO_COVERS {
                    break;
                }
            }
        }
    }

    // ---- Curated playlist cards (first 2) ------------------------------
    let mut playlist_cards: Vec<PlaylistCard> = Vec::new();
    if let Some(playlists) = artist.playlists.as_ref() {
        for playlist in playlists.iter().take(MAX_PLAYLIST_CARDS) {
            // Fetch the full playlist to harvest up to 3 distinct album covers.
            let mut cover_urls: Vec<String> = Vec::new();
            match runtime.core().get_playlist(playlist.id).await {
                Ok(full) => {
                    if let Some(container) = full.tracks.as_ref() {
                        let mut seen_albums: HashSet<String> = HashSet::new();
                        for track in &container.items {
                            let (Some(url), Some(album_id)) =
                                (track_album_cover(track), track_album_id(track))
                            else {
                                continue;
                            };
                            if seen_albums.insert(album_id) {
                                cover_urls.push(url);
                                if cover_urls.len() >= BOOK_COVERS {
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!(
                        "[qbz-qt] suggestions get_playlist({}) failed: {e}",
                        playlist.id
                    );
                }
            }
            // Fallback to the playlist's own images when no track covers found.
            if cover_urls.is_empty() {
                if let Some(images) = playlist.images.as_ref() {
                    if let Some(img) = images.iter().find(|s| !s.is_empty()) {
                        cover_urls.push(img.clone());
                    }
                }
            }
            playlist_cards.push(PlaylistCard {
                id: playlist.id.to_string(),
                name: playlist.name.clone(),
                track_count: playlist.tracks_count,
                cover_urls,
            });
        }
    }

    // `apply_suggestions` (suggestions.rs:323-343): playlist cards first,
    // the radio card always trailing (Tauri order).
    let mut cards: Vec<CardDoc> = playlist_cards.iter().map(playlist_to_card).collect();
    cards.push(radio_card(
        &current_track_id.to_string(),
        seed_track_name,
        &artist_id.to_string(),
        &radio_cover_urls,
    ));
    let tracks = rec.iter().map(to_track_doc).collect();

    Ok(SuggestionsDoc {
        loading: false,
        error: false,
        cards,
        tracks,
    })
}

// ---------------------------------------------------------------------------
// The invokable arms (main.rs:16684-16845)
// ---------------------------------------------------------------------------

/// `SuggestionsActions.load(track-id)` — entry + now-playing-change refresh
/// (`main.rs:16695-16722` → `navigate_suggestions` :3650-3679). Reads the
/// artist-id + title off the now-playing model (the Qt NowPlayingState).
/// An unparseable track id resets to the empty state; the dedup guard skips
/// a reload when the panel already shows this (artist, track).
pub(crate) fn load(track_id: String) {
    let (artist_id, track_name) = crate::now_playing::seed_meta();
    let (Ok(aid), Ok(tid)) = (artist_id.parse::<u64>(), track_id.parse::<u64>()) else {
        // No track / no artist -> reset to the empty state and stop
        // (`navigate_suggestions`'s empty-payload arm, :3662-3667).
        with_state(|s| {
            s.loaded = (String::new(), String::new());
        });
        mutate(|doc| {
            doc.loading = false;
            doc.error = false;
            doc.cards.clear();
            doc.tracks.clear();
        });
        return;
    };
    // Dedup (main.rs:16706-16712): the changed-watcher can refire on
    // unrelated now-playing churn.
    let dup = with_state(|s| s.loaded.0 == artist_id && s.loaded.1 == track_id);
    if dup {
        return;
    }
    with_state(|s| s.loaded = (artist_id.clone(), track_id.clone()));
    // `reset_suggestions` (:372-378): clear both lists, error off, loading on.
    mutate(|doc| {
        doc.cards.clear();
        doc.tracks.clear();
        doc.error = false;
        doc.loading = true;
    });
    let runtime = crate::app();
    crate::spawn(async move {
        match load_suggestions(&runtime, aid, tid, &track_name).await {
            Ok(doc) => publish(&doc),
            Err(()) => {
                // Top-level failure -> the error branch (`apply_suggestions`
                // with `error: true`, :341).
                mutate(|doc| {
                    doc.loading = false;
                    doc.error = true;
                    doc.cards.clear();
                    doc.tracks.clear();
                });
            }
        }
    });
}

/// `set_radio_loading` (`suggestions.rs:348-369`): flip the radio card's
/// `loading` flag (the building spinner). The radio card is found by
/// `kind == "radio"`, falling back to the LAST card (Tauri order:
/// playlists then radio).
fn set_radio_loading(loading: bool) {
    mutate(|doc| {
        if doc.cards.is_empty() {
            return;
        }
        let idx = doc
            .cards
            .iter()
            .position(|c| c.kind == "radio")
            .unwrap_or(doc.cards.len() - 1);
        doc.cards[idx].loading = loading;
    });
}

/// `start-radio` (main.rs:16789-16818): build the Song Radio off the seed
/// track via core, then start it (set-queue + play) through the shared
/// track-list play seam. The radio-card spinner flips on optimistically and
/// clears on completion (success OR failure, :16805).
pub(crate) fn start_radio(seed_track_id: String, seed_track_name: String, seed_artist_id: String) {
    let (Ok(tid), Ok(aid)) = (seed_track_id.parse::<u64>(), seed_artist_id.parse::<u64>()) else {
        return;
    };
    set_radio_loading(true);
    let runtime = crate::app();
    crate::spawn(async move {
        let result = runtime
            .core()
            .create_smart_track_radio(tid, aid, seed_track_name)
            .await;
        set_radio_loading(false);
        match result {
            Ok(tracks) if !tracks.is_empty() => {
                // The shared play_tracks seam: flat queue from the top, no
                // shuffle — `foryou_qt::play_flat`'s shape (:1144-1157).
                let queue: Vec<qbz_models::QueueTrack> = tracks
                    .iter()
                    .map(crate::foryou_qt::to_queue_track)
                    .collect();
                if let Err(e) = crate::playback_qt::play_track_list(&runtime, queue, 0, false).await
                {
                    log::warn!("[qbz-qt] song radio play failed: {e}");
                }
            }
            Ok(_) => log::warn!("[qbz-qt] song radio returned no tracks"),
            Err(e) => log::error!("[qbz-qt] song radio failed: {e}"),
        }
    });
}

/// `play-track` (main.rs:16827-16844): play a single recommended track NOW.
/// Qt twin: the same `play_single_track` the track-row click uses
/// (`crate::play_track`, main.rs:1195-1202).
pub(crate) fn play_track(track_id: String) {
    let Ok(tid) = track_id.parse::<u64>() else {
        return;
    };
    let runtime = crate::app();
    crate::spawn(async move {
        if let Err(e) = crate::playback_qt::play_single_track(&runtime, tid).await {
            log::error!("[qbz-qt] suggestions play-track {tid} failed: {e}");
        }
    });
}

/// `play-playlist` (main.rs:16732-16738): fetch + play the whole playlist
/// from the top — the card-level `playlist_qt::play_playlist_by_id` seam.
pub(crate) fn play_playlist(playlist_id: String) {
    let Ok(id) = playlist_id.parse::<u64>() else {
        return;
    };
    let runtime = crate::app();
    crate::spawn(async move {
        if let Err(e) = crate::playlist_qt::play_playlist_by_id(&runtime, id).await {
            log::error!("[qbz-qt] suggestions play-playlist {id} failed: {e}");
        }
    });
}

/// `queue-playlist` (main.rs:16746-16758): append the whole playlist.
pub(crate) fn queue_playlist(playlist_id: String) {
    let Ok(id) = playlist_id.parse::<u64>() else {
        return;
    };
    let runtime = crate::app();
    crate::spawn(async move {
        if let Err(e) = crate::playlist_qt::enqueue_playlist_by_id(&runtime, id, "queue").await {
            log::error!("[qbz-qt] suggestions queue-playlist {id} failed: {e}");
        }
    });
}

/// `play-next-playlist` (main.rs:16766-16778): the whole playlist after the
/// current track.
pub(crate) fn play_next_playlist(playlist_id: String) {
    let Ok(id) = playlist_id.parse::<u64>() else {
        return;
    };
    let runtime = crate::app();
    crate::spawn(async move {
        if let Err(e) = crate::playlist_qt::enqueue_playlist_by_id(&runtime, id, "next").await {
            log::error!("[qbz-qt] suggestions play-next-playlist {id} failed: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §4.5 doc shape, field-for-field: the four top-level keys and the
    /// EXACT card/track field sets (incl. badge + the seed triple) — a
    /// rename or a dropped key turns a QML read into `undefined` silently,
    /// so the shape is asserted here against the serialized JSON.
    #[test]
    fn doc_shape_is_the_contract_field_set() {
        let doc = SuggestionsDoc {
            loading: true,
            error: false,
            cards: vec![
                // Playlist card: badge "qobuz", EMPTY seed triple.
                CardDoc {
                    kind: "playlist".to_string(),
                    title: "Deep Cuts".to_string(),
                    subtitle: "42 tracks".to_string(),
                    cover_urls: vec![
                        "https://a/1.jpg".to_string(),
                        "https://a/2.jpg".to_string(),
                        "https://a/3.jpg".to_string(),
                    ],
                    playlist_id: "123".to_string(),
                    badge: "qobuz".to_string(),
                    ..CardDoc::default()
                },
                // Radio card: badge "qbz", FULL seed triple.
                CardDoc {
                    kind: "radio".to_string(),
                    title: "Song Radio".to_string(),
                    subtitle: "Seed Song".to_string(),
                    cover_urls: vec!["https://b/1.jpg".to_string()],
                    playlist_id: String::new(),
                    seed_track_id: "777".to_string(),
                    seed_track_name: "Seed Song".to_string(),
                    seed_artist_id: "888".to_string(),
                    badge: "qbz".to_string(),
                    loading: true,
                },
            ],
            tracks: vec![TrackDoc {
                id: "42".to_string(),
                title: "Song".to_string(),
                artist: "Artist".to_string(),
                artist_id: "7".to_string(),
                duration: "3:45".to_string(),
                art_url: "https://c/1.jpg".to_string(),
                explicit: true,
            }],
        };
        let v: serde_json::Value = serde_json::to_value(&doc).unwrap();

        // Top level: EXACTLY {loading, error, cards, tracks}.
        let top: HashSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(top, HashSet::from(["loading", "error", "cards", "tracks"]));
        assert_eq!(v["loading"], true);
        assert_eq!(v["error"], false);

        // Card: EXACTLY the Slint SuggestionCard field set (§4.5).
        let card_keys: HashSet<&str> = v["cards"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            card_keys,
            HashSet::from([
                "kind",
                "title",
                "subtitle",
                "coverUrls",
                "playlistId",
                "seedTrackId",
                "seedTrackName",
                "seedArtistId",
                "badge",
                "loading",
            ])
        );
        // The badge + seed triple survive the round-trip.
        assert_eq!(v["cards"][0]["badge"], "qobuz");
        assert_eq!(v["cards"][0]["playlistId"], "123");
        assert_eq!(v["cards"][1]["badge"], "qbz");
        assert_eq!(v["cards"][1]["seedTrackId"], "777");
        assert_eq!(v["cards"][1]["seedTrackName"], "Seed Song");
        assert_eq!(v["cards"][1]["seedArtistId"], "888");
        assert_eq!(v["cards"][1]["loading"], true);
        assert_eq!(v["cards"][0]["coverUrls"].as_array().unwrap().len(), 3);

        // Track: EXACTLY the §4.5 track field set.
        let track_keys: HashSet<&str> = v["tracks"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            track_keys,
            HashSet::from(["id", "title", "artist", "artistId", "duration", "artUrl", "explicit",])
        );
        assert_eq!(v["tracks"][0]["duration"], "3:45");
        assert_eq!(v["tracks"][0]["explicit"], true);
    }

    #[test]
    fn default_doc_is_full_shape_and_not_loading() {
        // The pre-publish frame: QML must see the empty state, never "{}".
        let v: serde_json::Value = serde_json::to_value(SuggestionsDoc::default()).unwrap();
        assert_eq!(v["loading"], false);
        assert_eq!(v["error"], false);
        assert!(v["cards"].as_array().unwrap().is_empty());
        assert!(v["tracks"].as_array().unwrap().is_empty());
    }

    #[test]
    fn mmss_formats_like_the_slint_mmss() {
        assert_eq!(mmss(0), "0:00");
        assert_eq!(mmss(65), "1:05");
        assert_eq!(mmss(600), "10:00");
    }

    #[test]
    fn shuffle_is_deterministic_per_seed() {
        // The splitmix64 shuffle must be reproducible: same input + seed ->
        // same permutation (the Slint loader relies on it so a reload does
        // not reshuffle the visible list).
        fn shuffled(seed: u64) -> Vec<u64> {
            let mut tracks: Vec<qbz_models::Track> = (1..=20)
                .map(|id| qbz_models::Track {
                    id,
                    ..qbz_models::Track::default()
                })
                .collect();
            shuffle_tracks(&mut tracks, seed);
            tracks.iter().map(|t| t.id).collect()
        }
        assert_eq!(shuffled(42), shuffled(42));
        assert_ne!(shuffled(42), shuffled(43));
        // A permutation, not a loss.
        let mut sorted = shuffled(42);
        sorted.sort_unstable();
        assert_eq!(sorted, (1..=20).collect::<Vec<u64>>());
    }

    #[test]
    fn radio_card_carries_the_seed_triple_and_qbz_badge() {
        // `radio_card()` sources the seed triple from the payload
        // (`suggestions.rs:297-319`) — the startRadio hover action reads
        // these back verbatim.
        let card = radio_card("11", "Seed", "22", &["https://x/1.jpg".to_string()]);
        assert_eq!(card.kind, "radio");
        assert_eq!(card.badge, "qbz");
        assert_eq!(card.seed_track_id, "11");
        assert_eq!(card.seed_track_name, "Seed");
        assert_eq!(card.seed_artist_id, "22");
        assert_eq!(card.playlist_id, "");
        assert!(!card.loading);
    }

    #[test]
    fn playlist_card_has_qobuz_badge_and_empty_seed() {
        let card = playlist_to_card(&PlaylistCard {
            id: "5".to_string(),
            name: "P".to_string(),
            track_count: 3,
            cover_urls: vec![],
        });
        assert_eq!(card.kind, "playlist");
        assert_eq!(card.badge, "qobuz");
        assert_eq!(card.playlist_id, "5");
        assert_eq!(card.seed_track_id, "");
        assert_eq!(card.seed_artist_id, "");
    }
}
