//! ArtistScene ("artists from the same place") — the controller behind
//! `QbzScene`, ported from Tauri's `ArtistsByLocationView.svelte` (1685 lines)
//! per `qbz-nix-docs/qt-frontend/2026-08-14-artist-scene-musician/00-CONTRACT.md`.
//!
//! Tauri is the visual authority for this module (contract §0); Slint is NOT a
//! reference, and §0.1 of the contract lists six defects in its port. Four of
//! them are RUST-SIDE and this file is where they are not reproduced:
//!
//!  - **S2** `crates/qbz/src/location_view.rs:19` sets `PAGE_SIZE = 30` with the
//!    comment "Matches the Tauri view's LIMIT". Tauri sends **100**
//!    (`ArtistsByLocationView.svelte:368`). See [`PAGE_SIZE`].
//!  - **S3** `location_view.rs:38` maps a missing `qobuz_id` to an empty string
//!    and emits the row anyway, producing cards that navigate nowhere. Tauri
//!    drops those candidates (`ArtistsByLocationView.svelte:170`). See
//!    [`map_candidate`].
//!  - **S4** `location_view.rs:32,114` puts the joined genre list in the card
//!    SUBTITLE. Tauri's payload carries `albums_count` there instead, and the
//!    genres exist only to feed the genre FILTER. Both ride the row here; which
//!    one is drawn is the view's call, and §13.1 names `albumsCount` as the
//!    subtitle.
//!  - **S5** the Slint controller swallows every failure into "loading = false"
//!    (`crates/qbz/src/main.rs:3384-3389`), so a network error is indistinguish-
//!    able from an empty scene. The three classes of contract §3/B7 —
//!    `offline` / `error` / `empty` — are three different `state` values here,
//!    and only `error` offers Retry.
//!
//! There is a fifth Slint defect that is worth naming because it explains the
//! shape of this file: its "Load more" reads a PROCESS-GLOBAL
//! `static LOCATION_PARAMS` (`crates/qbz/src/artist.rs:1518-1519`) that the next
//! artist page overwrites, so `artist A → scene A → back → artist B → forward →
//! Load more` appends artist B's page 2 into artist A's grid. Every query
//! parameter here is frozen into [`State`] by [`open`] and never re-read from
//! anywhere else.
//!
//! # The core seam (read before touching [`discover`])
//!
//! Contract B2/B3/B7/B8 landed in `qbz-core` from a parallel lane. This file
//! was written against the contract's PROSE description of them and the two
//! did not agree — argument order, the cancellation type and the callback
//! ownership were all different. That is recorded here rather than quietly
//! fixed, because it is the seam class this track has been burned by twice and
//! the reconciliation is the interesting part. The live signature is:
//!
//! ```text
//! core.discover_artists_by_location_with_progress(
//!     source_mbid: &str,
//!     area_id:     Option<&str>,
//!     area_name:   &str,
//!     country:     Option<&str>,
//!     genres:      Vec<String>,
//!     tags:        Vec<String>,
//!     limit:       usize,
//!     offset:      usize,
//!     blacklist:   &(dyn Fn(u64) -> bool + Send + Sync),        // B3
//!     cancel:      &CancelToken,                                // B8
//!     progress:    &(dyn Fn(ScenePhase, u8, String) + Send + Sync), // B2
//! ) -> Result<LocationDiscoveryResponse, SceneDiscoveryError>   // B7
//! ```
//!
//! Three consequences worth knowing before editing [`discover`]:
//!
//!  - **`blacklist` comes BEFORE `cancel`.** The assumed order had them
//!    swapped, and because both are references to callable-ish things the
//!    compiler's complaint was an inference error, not a type mismatch.
//!  - **The two `&dyn` arguments must be `let`-bound.** A closure literal in
//!    argument position has nowhere to live long enough to be borrowed as
//!    `&dyn Fn`; the error is E0282 "type annotations needed", which points at
//!    the closure and says nothing about lifetimes.
//!  - **`CancelToken` is `qbz-core`'s own type**, re-exported from its crate
//!    root for this. The assumption here was a bare `Arc<AtomicBool>` on the
//!    theory that a shared `std` type avoids a new public export; the core lane
//!    took the other view, and its type is the better one — `Clone`, `Default`,
//!    and an idempotent `cancel()` every clone observes.
//!
//! The phase argument is still IGNORED and the percentage is the authority.
//! Tauri's four emit points partition the range (contract §3/B2, mirroring
//! `src-tauri/src/commands_v2/integrations.rs:764-992`):
//! `searching` = `(genre_idx/total)*40` ⇒ `0..40`; `validating` = a flat `40`
//! then `40 + (idx/total)*55` ⇒ `40..95`; `done` = `100`. So `pct < 40` is
//! searching, `40..100` is validating, `>= 100` is done — see [`phase_for_pct`].
//!
//! `SceneDiscoveryError` is consumed for exactly one distinction:
//! [`SceneDiscoveryError::is_cancelled`]. A cancel must paint NOTHING, so
//! [`discover`] maps it to an EMPTY message and [`fail`] returns early on one.
//! Every other variant is the error state; `Ok` with zero rows is the empty
//! state, which is the whole point of B7.
//!
//! # Cancellation is not a generation guard (contract §3/B8)
//!
//! One page runs up to 100 SEQUENTIAL Qobuz artist searches
//! (`qbz-core/src/core.rs:2627-2653`) plus rate-limited MusicBrainz calls. A
//! generation check drops the final publish and leaves every one of those
//! requests in flight. [`State::cancel`] holds the token the core polls between
//! requests; it is tripped by `open` (a new scene), `retry`, `close` (view
//! deactivation) and an offline transition ([`arm_offline_watch`]). The
//! generation counter stays, because it is what keeps a LATE reply from
//! painting into a scene it does not belong to.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use cxx_qt_lib::QString;
use qbz_core::{CancelToken, ScenePhase};
use qbz_models::Album;
use serde::Serialize;

/// Rows per discovery page — Tauri's hardcoded `limit`
/// (`ArtistsByLocationView.svelte:368`). Slint's 30 (defect S2) changes
/// pagination, `has_more` and the Load-More cadence; do not "align" it.
const PAGE_SIZE: usize = 100;

/// Sidepanel albums per artist — Tauri's hardcoded `limit`, `offset` 0
/// (`ArtistsByLocationView.svelte:345-349`). There is no pagination in the
/// sidepanel: one request, then client-side categorisation.
const PANEL_ALBUM_LIMIT: u32 = 500;

// ---------------------------------------------------------------------------
//  State
// ---------------------------------------------------------------------------

struct State {
    /// Bumped by every `open` / `retry` / offline transition. A page that lands
    /// for a previous scene is DROPPED rather than painted — the same guard
    /// `artist_releases_qt.rs:78` documents, and it matters as much here: the
    /// nav stack stores no payload (`nav_qt.rs:9-23`), so back/forward remounts
    /// this view against whatever document is on the bridge.
    generation: u64,

    // --- Query parameters, frozen by `open` --------------------------------
    // Never re-read from a global. See the module header on Slint's
    // LOCATION_PARAMS defect.
    mbid: String,
    /// The MUSICBRAINZ name, not the Qobuz one — it is what the hero subtitle
    /// "Based on {artist}" prints (`ArtistsByLocationView.svelte:668`, payload
    /// built at `ArtistDetailView.svelte:2915`).
    artist_name: String,
    area_id: String,
    city: String,
    country: String,
    country_code: String,
    /// MusicBrainz location precision. Consumed by the DOOR gate on the artist
    /// page (contract §2.1, integration lane), not by discovery — it is carried
    /// so the scene can log which kind of location it was opened from, and so a
    /// future cache key has it without another plumbing round.
    precision: String,
    genres: Vec<String>,
    tags: Vec<String>,

    // --- Results ------------------------------------------------------------
    view_state: &'static str,
    error: String,
    scene_label: String,
    genre_summary: String,
    artists: Vec<SceneArtist>,
    /// Dedupe axis for appended pages: the MBID, exactly as Tauri's
    /// `existingIds` set (`ArtistsByLocationView.svelte:374-376`). NOT the
    /// Qobuz id — two MBIDs can validate onto the same Qobuz artist and Tauri
    /// keeps both.
    seen_mbids: HashSet<String>,
    /// The server's full candidate count, before the blacklist takes its cut.
    total_candidates: usize,
    /// Running count of rows the blacklist removed across every page loaded so
    /// far, so `total` stays honest as the user pages (contract §3/B3). The
    /// Slint controller recomputes this per page and therefore reports the last
    /// page's subtraction as if it were the whole scene's.
    removed_blacklisted: usize,
    offset: usize,
    has_more: bool,
    loading_more: bool,
    progress_phase: &'static str,
    progress_pct: i32,
    progress_detail: String,
    /// The token the core polls between requests (B8). `None` when nothing is
    /// in flight.
    cancel: Option<CancelToken>,

    // --- Sidepanel ----------------------------------------------------------
    panel_generation: u64,
    panel_state: &'static str,
    panel_artist_id: String,
    panel_error: String,
    discography: Vec<SceneAlbum>,
    eps_singles: Vec<SceneAlbum>,
    live_albums: Vec<SceneAlbum>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            generation: 0,
            mbid: String::new(),
            artist_name: String::new(),
            area_id: String::new(),
            city: String::new(),
            country: String::new(),
            country_code: String::new(),
            precision: String::new(),
            genres: Vec::new(),
            tags: Vec::new(),
            view_state: "loading",
            error: String::new(),
            scene_label: String::new(),
            genre_summary: String::new(),
            artists: Vec::new(),
            seen_mbids: HashSet::new(),
            total_candidates: 0,
            removed_blacklisted: 0,
            offset: 0,
            has_more: false,
            loading_more: false,
            progress_phase: "searching",
            progress_pct: 0,
            progress_detail: String::new(),
            cancel: None,
            panel_generation: 0,
            panel_state: "idle",
            panel_artist_id: String::new(),
            panel_error: String::new(),
            discography: Vec::new(),
            eps_singles: Vec::new(),
            live_albums: Vec::new(),
        }
    }
}

static SCENE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = SCENE.lock().unwrap();
    f(guard.get_or_insert_with(State::default))
}

// ---------------------------------------------------------------------------
//  The published document (contract §13.1 — FROZEN, a QML lane reads it)
// ---------------------------------------------------------------------------

/// One scene grid card.
///
/// `albumsCount` is the SUBTITLE (Tauri's payload, `:179`); `genres` ride the
/// row for the genre FILTER only and are never the subtitle (defect S4).
#[derive(Serialize)]
struct SceneArtist {
    // ---- The house card contract -----------------------------------------
    // OWNER RULING 2026-08-14 (R8), overriding the contract's own §13.1 and
    // its "do NOT carry the Slint card's affordances over" note: the grid
    // draws the STANDARD `cards/ArtistCard.qml`, with its hover overlay,
    // follow chip, pin and context menu — not a Tauri-faithful bare tile.
    // Tauri has none of that, so this is a deliberate, owner-authorised
    // departure from the 1:1 mandate for this control only.
    //
    // These four names are what `ArtistCard` reads off `item` (it uses
    // `title` with a `name` fallback, plus `subtitle`, `artUrl`, `id`), so
    // the row is published in the house shape and the view needs no
    // translation layer.
    id: String,
    title: String,
    /// The card's second line — and it is deliberately ALWAYS EMPTY.
    ///
    /// Contract §13.1 says the album count belongs here. **That is wrong, and
    /// it was corrected against the source**: Tauri sets `albums_count` on the
    /// card object (`ArtistsByLocationView.svelte:179`) and renders it
    /// NOWHERE — the field appears only in the TypeScript interface of both
    /// `VirtualizedFavoritesArtistGrid.svelte:12` and
    /// `VirtualizedFavoritesArtistList.svelte:12`, and the list carries the
    /// reason in a comment at `:269-270`: *"albums_count from Qobuz API
    /// includes compilations, tributes, etc. — misleadingly high. Removed per
    /// #169."* The contract described Tauri's DATA, not its UI.
    ///
    /// So the field stays, because `ArtistCard` reads it and an absent
    /// property is not the same as an empty one — but nothing is put in it.
    /// Filling it would re-introduce a line the reference's authors deleted on
    /// purpose, using a number they had already judged misleading. Slint put
    /// the joined genre list here instead, which is defect S4.
    subtitle: String,
    #[serde(rename = "artUrl")]
    art_url: String,

    /// Follow state, seeded from the disk-backed favourites cache so the chip
    /// is correct on the FIRST paint rather than flipping after a round-trip.
    following: bool,
    /// Pin state, same reasoning, from the sidebar's pinned store.
    #[serde(rename = "isPinned")]
    is_pinned: bool,

    // ---- Scene-specific, consumed by this view only -----------------------
    /// Kept alongside `id` because the two are not interchangeable: `id` is
    /// the Qobuz id the card acts on, `mbid` is the dedupe axis for appended
    /// pages (Tauri `:374-376`).
    #[serde(rename = "qobuzId")]
    qobuz_id: String,
    mbid: String,
    /// Raw count behind `subtitle`, so the view can re-render the phrase on a
    /// language change without a republish.
    #[serde(rename = "albumsCount")]
    albums_count: u32,
    /// FILTER input only — never the subtitle. See `subtitle` above.
    genres: Vec<String>,
}

/// One sidepanel album card.
///
/// `bitDepth` / `samplingRate` are REAL (contract DV-9). Tauri passes neither,
/// so its `QualityBadgeStatic` falls through to the `'cd'` default and stamps a
/// CD badge on 24/192 records — misinformation about audio quality is
/// load-bearing in this app, so that one wart is not reproduced. `qualityTier`
/// / `qualityDetail` are the derived pair every other Qt grid consumes
/// (`home_qt::quality_tier_from_depth`, `::quality_detail_from_parts`), and an
/// UNKNOWN bit depth yields an empty tier so the badge hides rather than lying
/// (`home_qt.rs:1183-1190`, contract §11/O1).
#[derive(Serialize, Clone)]
struct SceneAlbum {
    #[serde(rename = "albumId")]
    album_id: String,
    title: String,
    artist: String,
    #[serde(rename = "artistId")]
    artist_id: String,
    #[serde(rename = "artworkUrl")]
    artwork_url: String,
    /// 4-digit year, or "" — Tauri's `Number(release_date_original?.slice(0,4))`
    /// (`ArtistsByLocationView.svelte:760`).
    year: String,
    genre: String,
    #[serde(rename = "bitDepth")]
    bit_depth: Option<u32>,
    #[serde(rename = "samplingRate")]
    sampling_rate: Option<f64>,
    #[serde(rename = "qualityTier")]
    quality_tier: &'static str,
    #[serde(rename = "qualityDetail")]
    quality_detail: String,
    #[serde(rename = "trackCount")]
    track_count: u32,
}

#[derive(Serialize)]
struct ProgressDoc<'a> {
    phase: &'a str,
    pct: i32,
    detail: &'a str,
}

#[derive(Serialize)]
struct PanelDoc<'a> {
    state: &'a str,
    #[serde(rename = "artistId")]
    artist_id: &'a str,
    error: &'a str,
    discography: &'a [SceneAlbum],
    #[serde(rename = "epsSingles")]
    eps_singles: &'a [SceneAlbum],
    #[serde(rename = "liveAlbums")]
    live_albums: &'a [SceneAlbum],
}

#[derive(Serialize)]
struct SceneDoc<'a> {
    state: &'a str,
    #[serde(rename = "sceneLabel")]
    scene_label: &'a str,
    #[serde(rename = "genreSummary")]
    genre_summary: &'a str,
    #[serde(rename = "artistName")]
    artist_name: &'a str,
    #[serde(rename = "countryCode")]
    country_code: &'a str,
    #[serde(rename = "seedGenres")]
    seed_genres: &'a [String],
    error: &'a str,
    total: usize,
    #[serde(rename = "hasMore")]
    has_more: bool,
    #[serde(rename = "loadingMore")]
    loading_more: bool,
    progress: ProgressDoc<'a>,
    artists: &'a [SceneArtist],
    panel: PanelDoc<'a>,
}

fn publish() {
    let json = with_state(|s| {
        let doc = SceneDoc {
            state: s.view_state,
            scene_label: &s.scene_label,
            genre_summary: &s.genre_summary,
            artist_name: &s.artist_name,
            country_code: &s.country_code,
            seed_genres: &s.genres,
            error: &s.error,
            // Honest count: the server's candidate total minus everything the
            // blacklist has taken out so far (contract §3/B3).
            total: s.total_candidates.saturating_sub(s.removed_blacklisted),
            has_more: s.has_more,
            loading_more: s.loading_more,
            progress: ProgressDoc {
                phase: s.progress_phase,
                pct: s.progress_pct,
                detail: &s.progress_detail,
            },
            artists: &s.artists,
            panel: PanelDoc {
                state: s.panel_state,
                artist_id: &s.panel_artist_id,
                error: &s.panel_error,
                discography: &s.discography,
                eps_singles: &s.eps_singles,
                live_albums: &s.live_albums,
            },
        };
        serde_json::to_string(&doc).unwrap_or_else(|_| "{}".to_string())
    });
    crate::scene_bridge::ui(move |mut b| {
        b.as_mut().set_scene_json(QString::from(json.as_str()));
        // QML binds `sceneRev` to force a re-read: a QString that happens to be
        // byte-identical to the previous one emits no change signal, and the
        // progress stream produces exactly that (same document, same pct) more
        // often than any other view in the tree.
        let next = b.as_ref().scene_rev().wrapping_add(1);
        b.as_mut().set_scene_rev(next);
    });
}

// ---------------------------------------------------------------------------
//  Entry points (QbzScene invokables)
// ---------------------------------------------------------------------------

/// Door D1 (Network sidebar › Origin › the location) and door D2 (header ⋯ ›
/// "Artist Scene"). Both hand over the whole location context so nothing has to
/// be re-fetched and nothing has to be read back out of a global.
///
/// `genres_csv` / `tags_csv` are comma-separated because the QML seam has no
/// list type; empty segments are dropped, which is also how an artist with one
/// seed and a trailing comma survives.
///
/// The frozen signature (contract §13.1) carries `city` and `country` but NOT
/// the MusicBrainz `display_name`. Tauri's `areaName` is
/// `location.city || location.displayName` (`ArtistsByLocationView.svelte:364`),
/// so the fallback here is `country` instead — see [`area_name`] and the note in
/// the handoff report.
#[allow(clippy::too_many_arguments)]
pub fn open(
    mbid: String,
    artist_name: String,
    area_id: String,
    city: String,
    country: String,
    country_code: String,
    precision: String,
    genres_csv: String,
    tags_csv: String,
) {
    if mbid.is_empty() {
        // An empty MBID means the door fired before `publish_network` landed
        // (contract §2.1's second gate). Nothing failed; there is simply no
        // parameter, so there is nothing to open.
        log::warn!("[qbz-qt] scene open ignored: empty mbid");
        return;
    }
    arm_offline_watch();

    // Cancel whatever the previous scene still has in flight BEFORE the reset
    // rewrites the state it would have painted into.
    cancel_in_flight();

    let (generation, seed_count) = with_state(|s| {
        s.generation = s.generation.wrapping_add(1);
        s.mbid = mbid.clone();
        s.artist_name = artist_name;
        s.area_id = area_id;
        s.city = city;
        s.country = country;
        s.country_code = country_code;
        s.precision = precision;
        s.genres = split_csv(&genres_csv);
        s.tags = split_csv(&tags_csv);
        reset_results(s);
        reset_panel(s);
        (s.generation, s.genres.len() + s.tags.len())
    });

    // The route is a TWO-FILE contract (`nav_qt.rs:9-16`): this records the id,
    // `ContentRouter.qml` mounts it. A missing arm is a blank pane logged
    // nowhere. `scene` is deliberately NOT session-restore-safe — it needs a
    // live context, so it never becomes `last_view` (contract §4.1).
    crate::nav_qt::record("scene");

    if crate::offline_fwd::engine().status().is_offline() {
        // "Refuse entry" (contract §3/B7): the view mounts and shows the
        // offline placeholder. This is NOT the error class — there is no Retry.
        with_state(|s| s.view_state = "offline");
        publish();
        return;
    }

    // `precision` has no role in discovery — it gates the DOOR on the artist
    // page (contract §2.1, integration lane). It is logged because a scene
    // opened from a country-only location is the one shape that legitimately
    // returns nothing, and that is otherwise indistinguishable from a bug.
    log::info!(
        "[qbz-qt] scene open mbid={} precision={} seeds={}",
        mbid,
        with_state(|s| s.precision.clone()),
        seed_count
    );
    publish();
    fetch(generation, 0, true);
}

/// Explicit "Load More" — Tauri's footer button, not infinite scroll
/// (`ArtistsByLocationView.svelte:390-398`). Bails while a page is in flight or
/// when the server said there is nothing left.
pub fn load_more() {
    let Some((generation, offset)) = with_state(|s| {
        if s.loading_more || !s.has_more || s.view_state == "loading" {
            return None;
        }
        s.loading_more = true;
        // DV-5: re-arm the progress stream. Tauri's `loadMore` resets
        // `loadingProgress`/`loadingPhase` and re-attaches the listener; its
        // RETRY path forgets to, which is why the bar freezes there.
        s.progress_phase = "searching";
        s.progress_pct = 0;
        s.progress_detail.clear();
        // DV-17: a previous inline load-more failure is cleared by the retry of
        // that same action, never by a page landing somewhere else.
        s.error.clear();
        Some((s.generation, s.offset))
    }) else {
        return;
    };
    publish();
    fetch(generation, offset, false);
}

/// The error state's Retry (contract §4.2). Full restart at offset 0, with the
/// progress stream AND the cancellation token both re-armed (DV-5 / B8), and
/// deliberately WITHOUT `nav_qt::record`: the user is standing on this entry
/// already, and pushing a second one would make Back a no-op.
pub fn retry() {
    if with_state(|s| s.mbid.is_empty()) {
        return;
    }
    cancel_in_flight();
    if crate::offline_fwd::engine().status().is_offline() {
        with_state(|s| {
            s.generation = s.generation.wrapping_add(1);
            reset_results(s);
            s.view_state = "offline";
        });
        publish();
        return;
    }
    let generation = with_state(|s| {
        s.generation = s.generation.wrapping_add(1);
        reset_results(s);
        s.generation
    });
    publish();
    fetch(generation, 0, true);
}

/// A grid card / sidepanel row click: leave the scene for the artist page.
pub fn open_artist(qobuz_id: String) {
    if qobuz_id.is_empty() {
        return;
    }
    crate::open_artist(qobuz_id);
}

/// Sidepanel: load one artist's albums. Tauri refetches on every select, even
/// for the row that is already selected (`handleArtistSelect`, `:337-356`).
pub fn select_artist(qobuz_id: String) {
    if qobuz_id.is_empty() {
        return;
    }
    let generation = with_state(|s| {
        s.panel_generation = s.panel_generation.wrapping_add(1);
        s.panel_artist_id = qobuz_id.clone();
        s.panel_state = "loading";
        s.panel_error.clear();
        s.discography.clear();
        s.eps_singles.clear();
        s.live_albums.clear();
        s.panel_generation
    });
    publish();
    fetch_panel(generation, qobuz_id);
}

/// View deactivation (contract §3/B8). Stops the traffic — up to 100 sequential
/// Qobuz searches plus the rate-limited MusicBrainz calls — rather than merely
/// dropping the reply. The document is left standing so a Back into the scene
/// still shows what was loaded.
///
/// **The state a cancel leaves behind has to be a state the user can get out
/// of.** The nav stack carries no payload (`nav_qt.rs:9-23`), so Back re-mounts
/// this view against whatever document is on the bridge — nobody calls `open`
/// again. Cancelling a FIRST page and leaving `state = "loading"` would
/// therefore be a permanent spinner. It resolves to the error state instead,
/// with an EMPTY detail line (nothing failed; the user left) and Retry as the
/// way back in — Retry re-runs discovery from the parameters `open` froze, so
/// no context is lost. A cancelled LOAD MORE keeps the grid and leaves
/// `hasMore` true, so that same button is its own retry (DV-17's shape).
pub fn close() {
    cancel_in_flight();
    let changed = with_state(|s| {
        s.panel_generation = s.panel_generation.wrapping_add(1);
        let first_page_in_flight = s.view_state == "loading";
        if !first_page_in_flight && !s.loading_more {
            return false;
        }
        // Bump so the cancelled call's own Err (or a reply already in the
        // channel) cannot paint over the state chosen here.
        s.generation = s.generation.wrapping_add(1);
        s.cancel = None;
        s.loading_more = false;
        s.progress_phase = "searching";
        s.progress_pct = 0;
        s.progress_detail.clear();
        if first_page_in_flight {
            s.view_state = "error";
            s.error.clear();
            s.has_more = false;
        }
        true
    });
    if changed {
        publish();
    }
}

// ---------------------------------------------------------------------------
//  Cancellation + the offline transition
// ---------------------------------------------------------------------------

/// Trip the current token and forget it. Safe to call with nothing in flight.
fn cancel_in_flight() {
    let token = with_state(|s| s.cancel.take());
    if let Some(token) = token {
        token.cancel();
    }
}

fn new_cancel_token() -> CancelToken {
    let token = CancelToken::new();
    with_state(|s| s.cancel = Some(token.clone()));
    token
}

static OFFLINE_WATCH: OnceLock<()> = OnceLock::new();

/// Cancel discovery when the app goes offline (contract §3/B8's fourth
/// trigger). Armed lazily on the first `open` rather than at boot: a user who
/// never opens a scene should not pay for a watcher.
///
/// The engine's watch channel is the same one `offline_fwd::start_ui_forwarder`
/// consumes; a second subscriber is free (`tokio::sync::watch` is
/// multi-consumer).
fn arm_offline_watch() {
    if OFFLINE_WATCH.set(()).is_err() {
        return;
    }
    crate::spawn(async move {
        let mut rx = crate::offline_fwd::engine().subscribe();
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            // Scoped so the borrow is never held across an await point.
            let offline = { rx.borrow_and_update().is_offline() };
            if offline {
                on_offline_transition();
            }
        }
    });
}

fn on_offline_transition() {
    cancel_in_flight();
    let changed = with_state(|s| {
        if s.mbid.is_empty() || s.view_state == "offline" {
            return false;
        }
        s.generation = s.generation.wrapping_add(1);
        reset_results(s);
        reset_panel(s);
        s.view_state = "offline";
        true
    });
    if changed {
        publish();
    }
}

// ---------------------------------------------------------------------------
//  Reset helpers
// ---------------------------------------------------------------------------

fn reset_results(s: &mut State) {
    s.view_state = "loading";
    s.error.clear();
    s.scene_label.clear();
    s.genre_summary.clear();
    s.artists.clear();
    s.seen_mbids.clear();
    s.total_candidates = 0;
    s.removed_blacklisted = 0;
    s.offset = 0;
    s.has_more = false;
    s.loading_more = false;
    s.progress_phase = "searching";
    s.progress_pct = 0;
    s.progress_detail.clear();
}

fn reset_panel(s: &mut State) {
    s.panel_generation = s.panel_generation.wrapping_add(1);
    s.panel_state = "idle";
    s.panel_artist_id.clear();
    s.panel_error.clear();
    s.discography.clear();
    s.eps_singles.clear();
    s.live_albums.clear();
}

/// `"a, b ,, c"` -> `["a", "b", "c"]`.
fn split_csv(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Tauri sends `location.city || location.displayName`
/// (`ArtistsByLocationView.svelte:364`). The frozen `open` signature carries no
/// display name, so `country` is the fallback — see the module header.
fn area_name(s: &State) -> String {
    if !s.city.is_empty() {
        s.city.clone()
    } else {
        s.country.clone()
    }
}

// ---------------------------------------------------------------------------
//  Discovery
// ---------------------------------------------------------------------------

/// `pct` -> phase key. See the module header: Tauri's four emit points
/// partition the range, so this is exact and lets the progress callback ignore
/// whatever type the core lane chose for its phase argument.
fn phase_for_pct(pct: i32) -> &'static str {
    if pct >= 100 {
        "done"
    } else if pct >= 40 {
        "validating"
    } else {
        "searching"
    }
}

fn on_progress(generation: u64, pct: i32, detail: String) {
    let changed = with_state(|s| {
        // Every progress publish is generation-gated (contract §3/B2) — a
        // cancelled page's tail of events must not drive the live bar.
        if s.generation != generation {
            return false;
        }
        let pct = pct.clamp(0, 100);
        s.progress_phase = phase_for_pct(pct);
        s.progress_pct = pct;
        s.progress_detail = detail;
        true
    });
    if changed {
        publish();
    }
}

fn fetch(generation: u64, offset: usize, first: bool) {
    let cancel = new_cancel_token();
    crate::spawn(async move {
        let result = discover(generation, offset, cancel).await;
        apply_page(generation, offset, first, result);
    });
}

/// **THE core seam.** Every signature assumption of contract B2/B3/B7/B8 lives
/// in this one function; see the module header for the assumed shape. If the
/// core lane landed different names or a different token type, this body is the
/// whole fix.
async fn discover(
    generation: u64,
    offset: usize,
    cancel: CancelToken,
) -> Result<qbz_integrations::musicbrainz::LocationDiscoveryResponse, String> {
    let runtime = crate::app();
    let (mbid, area_id, area, country, genres, tags) = with_state(|s| {
        (
            s.mbid.clone(),
            s.area_id.clone(),
            area_name(s),
            s.country.clone(),
            s.genres.clone(),
            s.tags.clone(),
        )
    });

    // B3: the blacklist predicate the core applies INSIDE the pipeline, so a
    // cache hit cannot resurrect a blocked artist for 30 days. The post-filter
    // in `apply_page` is not redundant with it — it is what keeps the rendered
    // grid correct when a cache layer, present or future, short-circuits
    // validation entirely.
    let blacklisted = |id: u64| crate::artist_blacklist::is_blacklisted(id);

    // Both of these MUST be `let`-bound before the call: the core takes them as
    // `&dyn Fn`, and a closure literal in argument position has no place to
    // live long enough to be borrowed as one (E0282 — "type annotations
    // needed", which is what the compiler says instead of the real problem).
    //
    // The phase argument is ignored on purpose and the percentage is the
    // authority; see the module header on `phase_for_pct`.
    let progress = move |_phase: ScenePhase, pct: u8, detail: String| {
        on_progress(generation, pct as i32, detail);
    };

    runtime
        .core()
        .discover_artists_by_location_with_progress(
            &mbid,
            (!area_id.is_empty()).then_some(area_id.as_str()),
            &area,
            (!country.is_empty()).then_some(country.as_str()),
            genres,
            tags,
            PAGE_SIZE,
            offset,
            // ORDER: blacklist, THEN cancel. The module header's assumed shape
            // had these the other way around — the core lane landed
            // `(…, blacklist, cancel, progress)`.
            &blacklisted,
            &cancel,
            &progress,
        )
        .await
        // A cancelled run is NOT a failure to paint: the user navigated away,
        // retried, or went offline. It is dropped by the generation guard in
        // `apply_page`/`fail` (every cancel path bumps the generation first),
        // but flattening the typed error would make that guard the ONLY thing
        // standing between a cancel and an error frame — so keep the class
        // legible here rather than relying on it.
        .map_err(|e| {
            if e.is_cancelled() {
                String::new()
            } else {
                e.to_string()
            }
        })
}

fn apply_page(
    generation: u64,
    offset: usize,
    first: bool,
    result: Result<qbz_integrations::musicbrainz::LocationDiscoveryResponse, String>,
) {
    let response = match result {
        Ok(response) => response,
        Err(message) => {
            fail(generation, first, message);
            return;
        }
    };

    let changed = with_state(|s| {
        if s.generation != generation {
            return false;
        }
        s.cancel = None;

        let mut removed = 0usize;
        for candidate in &response.artists {
            let Some(row) = map_candidate(candidate, &mut removed) else {
                continue;
            };
            // Dedupe appended pages by MBID (Tauri `:374-376`).
            if !s.seen_mbids.insert(row.mbid.clone()) {
                continue;
            }
            s.artists.push(row);
        }
        s.removed_blacklisted += removed;
        s.scene_label = response.scene_label.clone();
        s.genre_summary = response.genre_summary.clone();
        s.total_candidates = response.total_candidates;
        // `next_offset` is the server's own cursor over the SCORED candidate
        // list, which is what the next page must resume from — it is not
        // `artists.len()`, because validation and the blacklist both drop rows.
        s.offset = response.next_offset.max(offset);
        s.has_more = response.has_more;
        s.loading_more = false;
        s.error.clear();
        s.progress_phase = "done";
        s.progress_pct = 100;
        s.view_state = if s.artists.is_empty() {
            "empty"
        } else {
            "ready"
        };
        true
    });
    if changed {
        publish();
    }
}

/// The two error arms.
///
/// **DV-17.** Tauri sets `error` unconditionally in `discoverArtists`'s catch
/// (`:386`), never clears it in `loadMore` (`:390-399`), and tests error BEFORE
/// grid (`:633` vs `:645`) — so one failed pagination replaces the loaded grid,
/// hero and nav bar with a full-screen error, and Retry then restarts at offset
/// 0. Here a failed page-N keeps `state = "ready"` and reports the failure in
/// `error`; the view surfaces it inline and `hasMore` stays true so the same
/// Load More is the retry. Only a failed FIRST page reaches the error state.
fn fail(generation: u64, first: bool, message: String) {
    // An EMPTY message is the cancellation sentinel `discover` maps
    // `SceneDiscoveryError::Cancelled` to. A cancel is something the user
    // asked for — navigating away, retrying, or going offline — and it must
    // paint NOTHING. The generation guard below would normally catch it (every
    // cancel path bumps the generation first), but a `close()` that cancels
    // without a new page in flight would otherwise leave an error frame with
    // no text on it, which is the worst of both.
    if message.is_empty() {
        log::debug!("[qbz-qt] scene discovery cancelled (first={first})");
        return;
    }
    log::warn!("[qbz-qt] scene discovery failed (first={first}): {message}");
    let changed = with_state(|s| {
        if s.generation != generation {
            return false;
        }
        s.cancel = None;
        s.loading_more = false;
        s.error = message;
        if first {
            s.view_state = "error";
            s.has_more = false;
        }
        true
    });
    if changed {
        publish();
    }
}

/// One validated candidate -> one grid card, or `None` when it cannot be one.
///
/// Two drops, and they are different in kind:
///  - **no Qobuz id** — Tauri filters these out (`:170`); Slint emits them with
///    an empty id and produces a card that navigates nowhere (defect S3). They
///    do NOT count against `total`, because the server never counted them as
///    available either.
///  - **blacklisted** — counted into `removed`, so `total` can be decremented
///    and stay honest (contract §3/B3).
fn map_candidate(
    candidate: &qbz_integrations::musicbrainz::LocationCandidate,
    removed: &mut usize,
) -> Option<SceneArtist> {
    let qobuz_id = candidate.qobuz_id?;
    if qobuz_id <= 0 {
        return None;
    }
    if crate::artist_blacklist::is_blacklisted(qobuz_id as u64) {
        *removed += 1;
        return None;
    }
    let id = qobuz_id.to_string();
    let albums_count = candidate.qobuz_albums_count.unwrap_or(0);
    Some(SceneArtist {
        // `qobuz_name || mb_name` (Tauri `:176`, and `location_view.rs:39`
        // agrees) — the catalog spelling wins because it is what the artist
        // page will show once the card is clicked.
        title: candidate
            .qobuz_name
            .clone()
            .unwrap_or_else(|| candidate.mb_name.clone()),
        subtitle: String::new(),
        art_url: candidate.qobuz_image.clone().unwrap_or_default(),
        // Seeded from the disk-backed caches so the chips are right from the
        // FIRST paint, the same way the reference's own card payload does it
        // (`crates/qbz/src/location_view.rs:105-117`). Neither store has a
        // change-notify, so these are a snapshot: the card owns its optimistic
        // local state after a click.
        following: crate::fav_cache_qt::is_artist_favorite(qobuz_id as u64),
        is_pinned: crate::sidebar_qt::is_pinned("artist", &id),
        qobuz_id: id.clone(),
        id,
        mbid: candidate.mbid.clone(),
        albums_count,
        genres: candidate.genres.clone(),
    })
}

// ---------------------------------------------------------------------------
//  Sidepanel albums
// ---------------------------------------------------------------------------

fn fetch_panel(generation: u64, qobuz_id: String) {
    let runtime = crate::app();
    crate::spawn(async move {
        let Ok(id) = qobuz_id.parse::<u64>() else {
            panel_fail(generation, format!("invalid artist id {qobuz_id}"));
            return;
        };
        let albums = match runtime
            .core()
            .get_artist_albums(id, Some(PANEL_ALBUM_LIMIT), Some(0))
            .await
        {
            Ok(albums) => albums,
            Err(e) => {
                panel_fail(generation, e.to_string());
                return;
            }
        };

        let mut discography = Vec::new();
        let mut eps_singles = Vec::new();
        let mut live_albums = Vec::new();
        for album in &albums.items {
            // Qt house invariant, not Tauri's: a blacklisted album or a
            // blacklisted primary artist never renders in a grid
            // (`artist_blacklist::card_blacklisted`, the same predicate every
            // other Qt album grid uses). Tauri has no blacklist at this layer.
            if crate::artist_blacklist::card_blacklisted(&album.id, &album.artist.id.to_string()) {
                continue;
            }
            match categorize_album(album, id) {
                AlbumCategory::Albums => discography.push(map_album(album)),
                AlbumCategory::Eps => eps_singles.push(map_album(album)),
                AlbumCategory::Live => live_albums.push(map_album(album)),
                // Tributes and Others are DROPPED, exactly as Tauri's switch
                // does (`ArtistsByLocationView.svelte:322-329`) — which is why
                // an artist with releases can legitimately show "No albums
                // found".
                AlbumCategory::Tributes | AlbumCategory::Others => {}
            }
        }

        let changed = with_state(|s| {
            if s.panel_generation != generation {
                return false;
            }
            let empty = discography.is_empty() && eps_singles.is_empty() && live_albums.is_empty();
            s.discography = discography;
            s.eps_singles = eps_singles;
            s.live_albums = live_albums;
            s.panel_state = if empty { "empty" } else { "ready" };
            s.panel_error.clear();
            true
        });
        if changed {
            publish();
        }
    });
}

fn panel_fail(generation: u64, message: String) {
    log::warn!("[qbz-qt] scene sidepanel albums failed: {message}");
    let changed = with_state(|s| {
        if s.panel_generation != generation {
            return false;
        }
        s.panel_state = "error";
        // Tauri renders the raw `String(err)` under the headline
        // (`.error-detail`, spec 01 §9); the headline itself is the view's
        // translated string.
        s.panel_error = message;
        true
    });
    if changed {
        publish();
    }
}

fn map_album(album: &Album) -> SceneAlbum {
    // Nested `audio_info` first, flat `maximum_*` as the fallback — the mapping
    // pattern every Qt album mapper uses (`album_qt.rs:1113-1122`). The V2
    // shapes nest it; the older flat shape does not.
    let bit_depth = album
        .audio_info
        .as_ref()
        .and_then(|ai| ai.maximum_bit_depth)
        .or(album.maximum_bit_depth);
    let sampling_rate = album
        .audio_info
        .as_ref()
        .and_then(|ai| ai.maximum_sampling_rate)
        .or(album.maximum_sampling_rate);
    let quality_tier = crate::home_qt::quality_tier_from_depth(bit_depth);
    // An empty tier means the depth is unknown; fabricating "16-bit / 44.1 kHz"
    // there would be the exact misinformation DV-9 exists to remove.
    let quality_detail = if quality_tier.is_empty() {
        String::new()
    } else {
        crate::home_qt::quality_detail_from_parts(bit_depth, sampling_rate)
    };
    let release_date = album
        .dates
        .as_ref()
        .and_then(|d| d.original.clone())
        .or_else(|| album.release_date_original.clone())
        .unwrap_or_default();
    SceneAlbum {
        album_id: album.id.clone(),
        title: album.title.clone(),
        artist: album.artist.name.clone(),
        artist_id: album.artist.id.to_string(),
        // `image.small || image.large` — Tauri's exact order
        // (`ArtistsByLocationView.svelte:757`), not `ImageSet::best()`: these
        // are 220 px cards and the mega asset is a waste of bandwidth.
        artwork_url: album
            .image
            .small
            .clone()
            .or_else(|| album.image.large.clone())
            .unwrap_or_default(),
        year: release_date.chars().take(4).collect(),
        genre: album
            .genre
            .as_ref()
            .map(|g| g.name.clone())
            .unwrap_or_default(),
        bit_depth,
        sampling_rate,
        quality_tier,
        quality_detail,
        track_count: track_count(album),
    }
}

// ---------------------------------------------------------------------------
//  Album categorisation — the port of `qobuzAdapters.ts:114-459`
// ---------------------------------------------------------------------------
//
// Tauri classifies the sidepanel's 500 albums CLIENT-SIDE; there is no server
// bucket for this (`get_releases_grid`'s buckets are a different taxonomy). The
// priority order is load-bearing and is Tauri's own:
//
//     tributes -> others(unofficial) -> others(compilation) -> live
//              -> albums(studio) -> eps -> albums(default)
//
// Only `albums` / `eps` / `live` reach the UI.
//
// The source is a pile of JavaScript regexes. `qbz-qt` has no `regex`
// dependency and this lane may not add one, so every pattern is expressed as a
// word-boundary substring test ([`has_word`]) using JS's own `\w` definition
// ([A-Za-z0-9_], ASCII only — which is why the CJK live markers below are plain
// substring tests, exactly as they are in the source regex, where they sit
// OUTSIDE the `\b(...)\b` group).

#[derive(Clone, Copy, PartialEq)]
enum AlbumCategory {
    Tributes,
    Others,
    Live,
    Eps,
    Albums,
}

/// JS `\w` — deliberately ASCII-only, so a boundary exists between "live" and a
/// following CJK character exactly as it does in the browser.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `\bneedle\b` over an already-lowercased haystack. `needle` may contain
/// spaces (`"in concert"`); only its outer edges are boundary-tested, which is
/// what the JS alternation does too.
fn has_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0usize;
    while let Some(pos) = hay[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before_ok = hay[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_ok = hay[end..].chars().next().is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
        // `needle` is ASCII and matched at `start`, so `start + 1` is a char
        // boundary.
        from = start + 1;
    }
    false
}

fn has_any_word(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| has_word(hay, n))
}

/// The V2 wire spells it `track_count`, the flat shape `tracks_count`
/// (`qbz-models/src/types.rs`). Tauri only ever sees the flat one.
fn track_count(album: &Album) -> u32 {
    album.tracks_count.or(album.track_count).unwrap_or(0)
}

/// `qobuzAdapters.ts:424-459`.
fn categorize_album(album: &Album, main_artist_id: u64) -> AlbumCategory {
    if is_tribute_album(album, main_artist_id) {
        return AlbumCategory::Tributes;
    }
    if is_unofficial_release(album) {
        return AlbumCategory::Others;
    }
    if is_compilation_album(album) {
        return AlbumCategory::Others;
    }
    if is_live_album(album) {
        return AlbumCategory::Live;
    }
    // Studio BEFORE EP, so a deluxe edition is not classified as an EP.
    if is_studio_album(album) {
        return AlbumCategory::Albums;
    }
    if is_ep_or_single(album) {
        return AlbumCategory::Eps;
    }
    AlbumCategory::Albums
}

/// `qobuzAdapters.ts:123-140`.
///
/// The JS builds a `tributePatterns` regex and then returns `true` for EVERY
/// different-artist album regardless of it (`:136`), so the regex is dead code
/// there. It is not ported: reproducing a test whose both branches return the
/// same value would only invite someone to "fix" it later.
///
/// `mainArtistId` is falsy-guarded in JS (`:115`); id `0` here is the same
/// no-id case and must not make every album a tribute.
fn is_tribute_album(album: &Album, main_artist_id: u64) -> bool {
    if main_artist_id == 0 || album.artist.id == 0 {
        return false;
    }
    album.artist.id != main_artist_id
}

/// `qobuzAdapters.ts:145-168`.
fn is_unofficial_release(album: &Album) -> bool {
    let title = album.title.to_lowercase();
    let label = album
        .label
        .as_ref()
        .map(|l| l.name.to_lowercase())
        .unwrap_or_default();
    let tracks = track_count(album);
    let duration = album.duration.unwrap_or(0);

    // `\b(fm broadcasts?|radio broadcasts?|broadcasts?|bootleg|unofficial|
    // pirate)\b` — the fm/radio arms are subsumed by the bare one.
    if has_any_word(
        &title,
        &["broadcast", "broadcasts", "bootleg", "unofficial", "pirate"],
    ) {
        return true;
    }
    // `\b(interviews?|speaks|talking|in their own words|the interviews?|
    // memories:?\s*the)\b`
    if has_any_word(
        &title,
        &[
            "interview",
            "interviews",
            "speaks",
            "talking",
            "in their own words",
            "memories: the",
            "memories the",
        ],
    ) {
        return true;
    }
    if has_any_word(
        &label,
        &[
            "leftfield media",
            "purple pyramid",
            "cleopatra",
            "laser media",
            "radio lu",
            "broadcast archives",
        ],
    ) {
        return true;
    }
    // Podcast-like: an episodic word, ONE track, 20+ minutes.
    tracks == 1
        && duration >= 1200
        && (has_any_word(&title, &["episode", "report"])
            || word_followed_by_number(
                &title,
                &["episode", "report", "podcast", "show", "program", "special"],
            ))
}

/// `\b(word)\s*\d+` — the first half of the JS podcast pattern.
fn word_followed_by_number(hay: &str, words: &[&str]) -> bool {
    for word in words {
        let mut from = 0usize;
        while let Some(pos) = hay[from..].find(word) {
            let start = from + pos;
            let end = start + word.len();
            let before_ok = hay[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_word_char(c));
            if before_ok {
                let rest = hay[end..].trim_start_matches(' ');
                if rest.starts_with(|c: char| c.is_ascii_digit()) {
                    return true;
                }
            }
            from = start + 1;
        }
    }
    false
}

/// `qobuzAdapters.ts:174-184`.
fn is_compilation_album(album: &Album) -> bool {
    let title = album.title.to_lowercase();
    has_any_word(
        &title,
        &[
            "greatest hits",
            "best of",
            "anthology",
            "the very best",
            "essential",
            "essentials",
            "definitive collection",
            "gold",
            "platinum",
            "hits collection",
            "complete collection",
            "hit collection",
            "b-sides",
            "rarities",
        ],
    ) || has_any_word(
        &title,
        // `\b(various artists|v\.?a\.?|original soundtrack|ost)\b` — the `v.a.`
        // arm is enumerated rather than pattern-matched; `\b` around an
        // all-punctuation tail is one of the places JS regex and any port
        // disagree, and guessing wrong here mislabels an album, not a pixel.
        &[
            "various artists",
            "va",
            "v.a",
            "v.a.",
            "original soundtrack",
            "ost",
        ],
    )
}

/// `qobuzAdapters.ts:189-235`.
fn is_live_album(album: &Album) -> bool {
    let title = album.title.to_lowercase();
    let genre = album
        .genre
        .as_ref()
        .map(|g| g.name.to_lowercase())
        .unwrap_or_default();

    // Broadcasts are Others, never Live — and this early return runs before the
    // genre check, so a "live" genre cannot rescue them.
    if has_any_word(&title, &["broadcast", "broadcasts"]) {
        return false;
    }
    // Substring, not word — the JS is `genre.includes('live')`.
    if genre.contains("live") {
        return true;
    }
    if has_any_word(
        &title,
        &["live", "alive", "in concert", "unplugged", "on stage"],
    ) {
        return true;
    }
    if has_any_word(
        &title,
        &[
            "en vivo",
            "en directo",
            "ao vivo",
            "dal vivo",
            "en direct",
            "konzert",
            "liveopname",
        ],
    ) {
        return true;
    }
    // ライブ / 라이브 / 콘서트 — these sit OUTSIDE the `\b(...)\b` group in the
    // source regex, so they are plain substring tests there too. Written as
    // escapes so the pattern survives any editor that normalises the file.
    if title.contains("\u{30e9}\u{30a4}\u{30d6}")          // ライブ
        || title.contains("\u{b77c}\u{c774}\u{be0c}")      // 라이브
        || title.contains("\u{cf58}\u{c11c}\u{d2b8}")
    // 콘서트
    {
        return true;
    }

    let studio_variant = has_any_word(&title, &["remaster", "deluxe", "anniversary", "expanded"]);
    // `\b(tour|on tour)\b(?!.*\bedition\b)` — a lookahead over the REST of the
    // string, which is why "edition" is tested against the whole title here.
    if has_word(&title, "tour") && !has_word(&title, "edition") && !studio_variant {
        return true;
    }
    // City + year: a live recording's location and date.
    if !studio_variant && has_any_word(&title, LIVE_CITIES) && has_live_year(&title) {
        return true;
    }
    false
}

/// The city list from `qobuzAdapters.ts:222`, verbatim and in order.
const LIVE_CITIES: &[&str] = &[
    "seattle",
    "tokyo",
    "london",
    "paris",
    "new york",
    "los angeles",
    "chicago",
    "dallas",
    "amsterdam",
    "berlin",
    "sydney",
    "melbourne",
    "montreal",
    "toronto",
    "rio",
    "sao paulo",
    "mexico city",
    "denver",
    "boston",
    "philadelphia",
    "san francisco",
    "cleveland",
    "detroit",
    "atlanta",
    "miami",
    "phoenix",
    "houston",
    "minneapolis",
    "st. louis",
    "st louis",
    "kansas city",
    "nashville",
    "memphis",
    "austin",
    "portland",
    "oakland",
    "stockholm",
    "oslo",
    "dublin",
    "manchester",
    "birmingham",
    "glasgow",
    "brussels",
    "madrid",
    "barcelona",
    "rome",
    "milan",
    "vienna",
    "munich",
    "hamburg",
    "zurich",
    "prague",
    "warsaw",
    "moscow",
    "seoul",
    "osaka",
    "singapore",
    "hong kong",
    "taipei",
    "manila",
    "jakarta",
    "mumbai",
    "delhi",
    "bangalore",
    "cape town",
    "johannesburg",
];

/// `\b(19[6-9]\d|20[0-2]\d)\b` — a standalone 4-digit token in 1960..=2029.
fn has_live_year(title: &str) -> bool {
    title
        .split(|c: char| !is_word_char(c))
        .filter(|token| token.len() == 4 && token.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|token| token.parse::<u32>().ok())
        .any(|year| (1960..=2029).contains(&year))
}

/// `qobuzAdapters.ts:237-259`.
///
/// The JS anchors its EP markers with `\b` in positions where a `\b` can never
/// assert (`\b(- ep|...)`, where the char before `-` is a space), so those arms
/// are largely dead in the browser. Ported by INTENT rather than by mechanism:
/// the markers are matched as written and the numeric rules — which are what
/// actually classify almost everything, since [`is_studio_album`] has already
/// claimed anything with 7+ tracks — are exact.
fn is_ep_or_single(album: &Album) -> bool {
    let title = album.title.to_lowercase();
    let tracks = track_count(album);
    let duration = album.duration.unwrap_or(0);

    if title.contains("- ep") || title.contains(".ep") || title.contains("(ep)") {
        return true;
    }
    if ends_with_word(&title, "ep") {
        return true;
    }
    if title.contains("- single") || title.contains("(single)") {
        return true;
    }
    if has_word(&title, "single") && !has_any_word(&title, &["version", "mix", "edit"]) {
        return true;
    }
    if tracks > 0 && tracks <= 4 {
        return true;
    }
    if duration > 0 && duration <= 900 {
        return true;
    }
    if (5..=6).contains(&tracks) && duration > 0 && duration <= 1200 {
        return true;
    }
    false
}

/// `\bword\s*$`.
fn ends_with_word(hay: &str, word: &str) -> bool {
    let trimmed = hay.trim_end();
    if !trimmed.ends_with(word) {
        return false;
    }
    trimmed[..trimmed.len() - word.len()]
        .chars()
        .next_back()
        .is_none_or(|c| !is_word_char(c))
}

/// `qobuzAdapters.ts:265-287`.
fn is_studio_album(album: &Album) -> bool {
    let title = album.title.to_lowercase();
    let tracks = track_count(album);
    let duration = album.duration.unwrap_or(0);

    // A single 20-minute track is a radio show, not an album.
    if tracks == 1 && duration >= 1200 {
        return false;
    }
    let studio_variant = has_any_word(
        &title,
        &[
            "deluxe",
            "remaster",
            "anniversary",
            "expanded",
            "special edition",
            "collector",
            "box set",
        ],
    );
    if studio_variant && (tracks >= 7 || duration >= 2100) {
        return true;
    }
    tracks >= 7 || duration >= 1500
}
