//! Musician surfaces — the resolver/dispatcher plus the two documents it
//! feeds: the full MusicianPage (`musicianJson`, route `musician`) and the
//! global MusicianModal (`modalJson`).
//!
//! Contract: `qbz-nix-docs/qt-frontend/2026-08-14-artist-scene-musician/`
//! `00-CONTRACT.md` §2.2 (the dispatcher), §5.3 (bands), §5.4/§5.5 (the two
//! Tauri data facts that look like bugs) and §13.2 (the frozen document
//! shapes). Visual reference is Tauri's `MusicianPageView.svelte` +
//! `MusicianModal.svelte`; `crates/qbz/src/musician.rs` supplied the shared-core
//! CALL sequence and nothing else (its `MusicianState.bands` is never written —
//! Slint defect S1).
//!
//! # THE DISPATCHER LIVES HERE, NOT IN QML
//!
//! Six call sites reach a musician by NAME (artist network members / member-of /
//! collaborators, album-credits performer, track-info performer, immersive
//! track-info performer). All six call ONE invokable — `resolveAndOpen` — so
//! all six behave identically and **QML never branches on confidence**. Tauri
//! branches in `+page.svelte:2178-2220`; a QML copy of that switch would be six
//! copies of it.
//!
//! The five-way dispatch, with the two traps that draft 1 of the contract
//! walked into:
//!
//! | resolve result | surface |
//! |---|---|
//! | `Confirmed` + `qobuz_artist_id > 0` | the Qobuz artist page |
//! | `Confirmed` + id absent-or-ZERO | the modal |
//! | `Contextual` | this page (route `musician`) |
//! | `Weak` / `None` / anything else | the modal |
//! | the resolve call failed | error toast + modal, synthetic `none` |
//!
//! **Trap 1 — `Some(0)` is not an id.** `qobuz_artist_id` is
//! `i64::try_from(artist.id).ok()` (`qbz-core/src/core.rs:2707`) over
//! `Artist.id: u64` declared `#[serde(default)]`
//! (`qbz-models/src/types.rs:545-546`), so an absent id in the Qobuz search
//! payload arrives as `Some(0)`. JS reads `0` as falsy and opens the modal;
//! a Rust `(Confirmed, Some(id))` arm would navigate to artist "0" and land the
//! user on a broken page. Hence the `if id > 0` guard, which is the ONLY reason
//! the modal's `confirmed` copy is reachable at all.
//!
//! **Trap 2 — the catch-all arm is a real branch.** Tauri's switch ends
//! `case 'weak': case 'none': default:` (`+page.svelte:2201-2206`), so any
//! confidence the frontend does not recognise opens the modal rather than
//! falling through silently. The `_` arm below is that `default:`.
//!
//! # What is deliberately NOT "fixed"
//!
//! * `role_on_album` is `role.clone()` on every row (`core.rs:2761`), so every
//!   card's badge shows the same string — the one the user clicked. Honest:
//!   per-album credit data does not exist. Contract §5.4.
//! * `releaseDate` carries the FULL value the backend returns — a complete ISO
//!   date like `"1994-09-27"`, not a year. Both backends pass
//!   `release_date_original` deliberately and Tauri renders it raw
//!   (`MusicianPageView.svelte:183-185`). The Qt field is named `releaseDate`
//!   rather than `year` so the NAME stops lying; the VALUE is 1:1.
//! * "Appears On" is a plain Qobuz album search on the musician's NAME
//!   (`core.rs:2745-2768`) — nothing verifies the musician appears on any
//!   returned album. Tauri is identical. Owner ruling R6: ship as-is.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use cxx_qt_lib::QString;
use qbz_integrations::musicbrainz::{MusicianConfidence, ResolvedMusician};
use serde::Serialize;

/// Appearances page size — Tauri's `ITEMS_PER_PAGE`
/// (`MusicianPageView.svelte:37`). NOT Slint's 30, and not the scene view's
/// 100: three different page sizes in three different places, all deliberate.
const PAGE_SIZE: u32 = 20;

// ---------------------------------------------------------------------------
//  Documents (contract §13.2 — FROZEN; a QML lane is written against these)
// ---------------------------------------------------------------------------

/// One "Bands & Projects" chip. The MBID rides along because the chip
/// NAVIGATES (§5.3) — Tauri's only `console.log`s a TODO.
#[derive(Clone, Serialize)]
struct BandRow {
    mbid: String,
    name: String,
}

/// One "Appears On" card.
#[derive(Clone, Serialize)]
struct AppearanceRow {
    #[serde(rename = "albumId")]
    album_id: String,
    title: String,
    #[serde(rename = "artistName")]
    artist_name: String,
    /// FULL ISO date (see the file header). Empty when the backend had none.
    #[serde(rename = "releaseDate")]
    release_date: String,
    #[serde(rename = "roleOnAlbum")]
    role_on_album: String,
    /// The REMOTE cover url, kept because it is the key
    /// `QbzShell.sidebarArtworkWindow` replies on.
    #[serde(rename = "artworkUrl")]
    artwork_url: String,
    /// Rust-resolved `file://` path for the same cover, "" until it is on
    /// disk. ADDITIVE to the frozen shape: a QML delegate that binds
    /// `artworkUrl` straight into an `Image.source` draws nothing (the cache is
    /// a Rust-side concern — `artist_releases_qt.rs:37-47` documents the same
    /// trap), and the two-pass fill below is what makes covers appear without
    /// any QML artwork plumbing at all.
    #[serde(rename = "artPath")]
    art_path: String,
}

/// `musicianJson` — the page.
#[derive(Serialize)]
struct MusicianDoc<'a> {
    /// `loading` | `error` | `ready`. Tauri's three page states
    /// (`MusicianPageView.svelte:143-155`); there is no empty state distinct
    /// from `ready` with an empty grid, which is what its Disc-icon block is.
    state: &'a str,
    name: &'a str,
    role: &'a str,
    confidence: &'a str,
    error: &'a str,
    total: usize,
    #[serde(rename = "hasMore")]
    has_more: bool,
    #[serde(rename = "loadingMore")]
    loading_more: bool,
    /// `[]` means the section is ABSENT — no spinner, no empty frame (§5.3).
    bands: &'a [BandRow],
    appearances: &'a [AppearanceRow],
}

/// `modalJson` — the global overlay. `""` (not `{}`) is the CLOSED state, so
/// the QML host tests the raw string before parsing it.
#[derive(Serialize)]
struct ModalDoc<'a> {
    name: &'a str,
    role: &'a str,
    confidence: &'a str,
    /// Tauri's modal never renders this (verified: the identifier appears
    /// nowhere in `MusicianModal.svelte`); it is carried because the CTA and
    /// the explanatory copy are chosen from confidence + this count.
    #[serde(rename = "appearsOnCount")]
    appears_on_count: usize,
    bands: &'a [BandRow],
}

// ---------------------------------------------------------------------------
//  State
// ---------------------------------------------------------------------------

/// The PAGE's state. ONE musician at a time — opening another resets it
/// wholesale.
///
/// The modal keeps its own identity below rather than sharing this one, and
/// that separation is load-bearing: the modal can open FROM the page (a band
/// chip whose group resolves weak), and a shared struct would rewrite the
/// page's header and wipe its grid underneath the overlay, leaving the wrong
/// musician on screen the moment the modal closes.
#[derive(Default)]
struct State {
    /// Bumped by every open. A page of appearances that lands for a musician
    /// the user has already left is DROPPED rather than appended — the
    /// `artist_releases_qt.rs:74-78` guard, and it matters for the same reason:
    /// the nav stack stores no payload (`nav_qt.rs:9-23`), so Back/Forward
    /// re-mounts this view against whatever document is still on the bridge.
    generation: u64,
    name: String,
    /// The role as the CALLER supplied it. Also the API argument for every
    /// appearances page, which is why it is kept verbatim and not prettified:
    /// `role_on_album` echoes it back on every row.
    role: String,
    confidence: String,
    /// MusicBrainz id of the resolved musician, `""` when MB is off or the
    /// resolve found nothing. Gates the whole bands section.
    mbid: String,
    /// `loading` | `error` | `ready`.
    page_state: String,
    error: String,
    bands: Vec<BandRow>,
    appearances: Vec<AppearanceRow>,
    total: usize,
    /// Rows the SERVER has handed over — the offset of the next page.
    /// Deliberately NOT `appearances.len()`: the dedupe below makes the
    /// rendered count smaller than the fetched one, and feeding the smaller
    /// number back re-requests rows already seen and skips the tail.
    offset: u32,
    has_more: bool,
    loading_more: bool,
}

static MUSICIAN: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = MUSICIAN.lock().unwrap();
    f(guard.get_or_insert_with(State::default))
}

/// The MODAL's identity, independent of the page (see `State`). `None` is the
/// closed state and publishes `""`.
struct Modal {
    name: String,
    role: String,
    confidence: String,
    mbid: String,
    appears_on_count: usize,
    bands: Vec<BandRow>,
}

static MODAL: Mutex<Option<Modal>> = Mutex::new(None);

// ---------------------------------------------------------------------------
//  Publish
// ---------------------------------------------------------------------------

// `musicianRev` is bumped inline in both publishers rather than through a
// helper: the Pin<&mut QbzMusician> receiver cannot cross a plain fn boundary
// without naming the bridge's generated type, and two lines twice is cheaper
// than that import. QML re-parses on the QString change already — the counter
// exists so a view can force a re-read of a document whose text happens to be
// identical (a second resolve of the same musician), the same trick
// `QbzSession.trRev` plays.

fn publish_page() {
    let json = with_state(|s| {
        let doc = MusicianDoc {
            state: &s.page_state,
            name: &s.name,
            role: &s.role,
            confidence: &s.confidence,
            error: &s.error,
            total: s.total,
            has_more: s.has_more,
            loading_more: s.loading_more,
            bands: &s.bands,
            appearances: &s.appearances,
        };
        serde_json::to_string(&doc).unwrap_or_else(|_| "{}".to_string())
    });
    crate::musician_bridge::ui(move |mut b| {
        b.as_mut().set_musician_json(QString::from(json.as_str()));
        let next = b.as_ref().musician_rev() + 1;
        b.as_mut().set_musician_rev(next);
    });
}

/// Publishes the modal document, or `""` when it is closed.
fn publish_modal() {
    let json = {
        let guard = MODAL.lock().unwrap();
        match guard.as_ref() {
            None => String::new(),
            Some(m) => {
                let doc = ModalDoc {
                    name: &m.name,
                    role: &m.role,
                    confidence: &m.confidence,
                    appears_on_count: m.appears_on_count,
                    bands: &m.bands,
                };
                serde_json::to_string(&doc).unwrap_or_else(|_| String::new())
            }
        }
    };
    crate::musician_bridge::ui(move |mut b| {
        b.as_mut().set_modal_json(QString::from(json.as_str()));
        let next = b.as_ref().musician_rev() + 1;
        b.as_mut().set_musician_rev(next);
    });
}

// ---------------------------------------------------------------------------
//  The dispatcher (contract §2.2) — the ONLY entry the six consumers call
// ---------------------------------------------------------------------------

fn confidence_label(c: MusicianConfidence) -> &'static str {
    match c {
        MusicianConfidence::Confirmed => "confirmed",
        MusicianConfidence::Contextual => "contextual",
        MusicianConfidence::Weak => "weak",
        MusicianConfidence::None => "none",
    }
}

/// Resolve a musician NAME (they carry no catalog id) and open whichever
/// surface the confidence earns. See the file header for the branch table.
///
/// The "Looking up {}..." toast fires BEFORE the call, exactly as Tauri does
/// (`+page.svelte:2179`) — the resolve is two sequential network round trips
/// (MB resolve + a Qobuz artist search, plus an album search on the miss path),
/// so without it the click looks dead for a second or more.
pub fn resolve_and_open(name: String, role: String) {
    let name = name.trim().to_string();
    if name.is_empty() {
        // No name means the caller has no performer row on screen, not that
        // something failed — the `artist_releases_qt::open` empty-id rule.
        return;
    }
    // Every branch of this dispatcher needs the network: the resolve itself,
    // the appearances search, the artist page. A dead click with no explanation
    // is worse than the toast.
    if crate::offline_fwd::engine().status().is_offline() {
        crate::toast_qt::warning(qbz_i18n::t("You're offline"));
        return;
    }
    crate::toast_qt::info(qbz_i18n::t_args("Looking up {}...", &[&name]));

    let runtime = crate::app();
    crate::spawn(async move {
        match runtime
            .core()
            .musicbrainz_resolve_musician(&name, &role)
            .await
        {
            Ok(resolved) => dispatch(resolved),
            Err(e) => {
                log::warn!("[qbz-qt] musician resolve failed for {name:?}: {e}");
                // Toast AND modal, 1:1 with Tauri's catch
                // (`+page.svelte:2208-2219`): the modal is synthesised so the
                // user still gets the name, the role and an explanation rather
                // than a toast that vanishes in five seconds.
                crate::toast_qt::error(qbz_i18n::t("Failed to look up musician"));
                // `ResolvedMusician::empty` IS Tauri's synthetic literal
                // `{confidence:'none', bands:[], appears_on_count:0}`.
                open_modal(ResolvedMusician::empty(name, role));
            }
        }
    });
}

fn dispatch(resolved: ResolvedMusician) {
    match (resolved.confidence, resolved.qobuz_artist_id) {
        // `id > 0` is load-bearing — see trap 1 in the file header.
        (MusicianConfidence::Confirmed, Some(id)) if id > 0 => {
            log::info!(
                "[qbz-qt] musician {:?} confirmed -> artist {id}",
                resolved.name
            );
            crate::open_artist(id.to_string());
        }
        (MusicianConfidence::Contextual, _) => open_page(resolved),
        // Confirmed-with-no-id, Weak, None, and any confidence added upstream
        // later — Tauri's `default:` arm.
        _ => open_modal(resolved),
    }
}

/// Route `musician`. Seeds the header from the resolve, flips the page into its
/// loading state, records the nav entry, THEN fetches — the order
/// `artist_releases_qt::open` uses, so the view mounts already spinning instead
/// of flashing an empty grid.
fn open_page(resolved: ResolvedMusician) {
    let mbid = resolved.mbid.clone().unwrap_or_default();
    let generation = reset(&resolved);
    // Navigating to the page ends the modal, whichever route got us here (a
    // band chip inside it is the live case).
    *MODAL.lock().unwrap() = None;
    // Two-file route contract (`nav_qt.rs:9-16`): this records the id,
    // `ContentRouter.qml` mounts it. A missing arm is a BLANK pane logged
    // nowhere. Not in `VIEW_TO_PREF`, so it never becomes `last_view` — a
    // restore onto this route would have no name to resolve.
    crate::nav_qt::record("musician");
    publish_page();
    publish_modal();
    fetch_appearances(generation, resolved.name, resolved.role, 0, true);
    fetch_bands(mbid);
}

/// The global overlay (ADR-009, z >= 3000). No route, no nav entry: it is a
/// published document, mounted outside the view chain.
///
/// The page document is deliberately left ALONE — see `State`. A modal opened
/// over a loaded musician page must not erase what is behind it.
fn open_modal(resolved: ResolvedMusician) {
    let mbid = resolved.mbid.clone().unwrap_or_default();
    *MODAL.lock().unwrap() = Some(Modal {
        name: resolved.name,
        role: resolved.role,
        confidence: confidence_label(resolved.confidence).to_string(),
        // `resolved.bands` is `Vec::new()` on EVERY path of EVERY backend
        // (`core.rs:2714`,`:2736`) — the "Known for" list has never rendered
        // anywhere. Seeded from the relationship memo and filled by
        // `fetch_bands`. Contract DV-2.
        bands: cached_bands(&mbid).unwrap_or_default(),
        mbid: mbid.clone(),
        appears_on_count: resolved.appears_on_count,
    });
    publish_modal();
    fetch_bands(mbid);
}

/// Seed the PAGE. Returns the new generation, which every in-flight fetch is
/// checked against.
fn reset(resolved: &ResolvedMusician) -> u64 {
    with_state(|s| {
        s.generation = s.generation.wrapping_add(1);
        s.name = resolved.name.clone();
        s.role = resolved.role.clone();
        s.confidence = confidence_label(resolved.confidence).to_string();
        let mbid = resolved.mbid.clone().unwrap_or_default();
        s.page_state = "loading".to_string();
        s.error.clear();
        // See the DV-2 note in `open_modal` — same reason, same seed.
        s.bands = cached_bands(&mbid).unwrap_or_default();
        s.mbid = mbid;
        s.appearances.clear();
        s.total = 0;
        s.offset = 0;
        s.has_more = false;
        s.loading_more = false;
        s.generation
    })
}

// ---------------------------------------------------------------------------
//  Bands & Projects (contract §5.3 / DV-2)
// ---------------------------------------------------------------------------

/// Completed relationship lookups, keyed by MBID. `qbz-core`'s own 7-day
/// SQLite cache only exists when the MB cache is installed, which the Qt build
/// does not do yet (contract B4, integration-owned), so without this memo a
/// user bouncing between two sidemen re-fetches both every time.
static BANDS_CACHE: Mutex<Option<HashMap<String, Vec<BandRow>>>> = Mutex::new(None);
/// MBIDs with a request in the air. There is NO in-flight registry in the cache
/// layer — only a completed-result cache (`core.rs:2210-2226`) — so this module
/// owns the coalescing or it fires duplicate requests for the same MBID.
static BANDS_INFLIGHT: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn cached_bands(mbid: &str) -> Option<Vec<BandRow>> {
    if mbid.is_empty() {
        return None;
    }
    let guard = BANDS_CACHE.lock().unwrap();
    guard.as_ref()?.get(mbid).cloned()
}

fn store_bands(mbid: &str, bands: &[BandRow]) {
    let mut guard = BANDS_CACHE.lock().unwrap();
    guard
        .get_or_insert_with(HashMap::new)
        .insert(mbid.to_string(), bands.to_vec());
}

/// `true` when THIS caller owns the request; `false` when one is already in the
/// air for the same MBID.
fn claim_inflight(mbid: &str) -> bool {
    let mut guard = BANDS_INFLIGHT.lock().unwrap();
    guard
        .get_or_insert_with(HashSet::new)
        .insert(mbid.to_string())
}

fn release_inflight(mbid: &str) {
    let mut guard = BANDS_INFLIGHT.lock().unwrap();
    if let Some(set) = guard.as_mut() {
        set.remove(mbid);
    }
}

/// Fetch the groups this musician is a member of.
///
/// The state table (§5.3), and every arm of it is a SILENT one:
/// no MBID -> the section is absent; the fetch failing -> the section is
/// absent, logged only. An optional block never gets an error frame, and it
/// never gets a spinner either — it appears when it has content or not at all.
fn fetch_bands(mbid: String) {
    if mbid.is_empty() {
        return;
    }
    if let Some(cached) = cached_bands(&mbid) {
        apply_bands(&mbid, cached);
        return;
    }
    if !claim_inflight(&mbid) {
        // Someone else is fetching this exact MBID; their `apply_bands` keys on
        // the MBID (not on a generation), so it will land on whatever surface
        // is open by then — including this one.
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        let result = runtime
            .core()
            .musicbrainz_get_artist_relationships(&mbid)
            .await;
        release_inflight(&mbid);
        match result {
            Ok(rels) => {
                // MB emits one relation per membership PERIOD, so a musician who
                // left and rejoined a band yields the same group twice. Dedupe on
                // MBID, keep the first.
                let mut seen: HashSet<String> = HashSet::new();
                let bands: Vec<BandRow> = rels
                    .groups
                    .into_iter()
                    .filter(|g| !g.name.trim().is_empty() && seen.insert(g.mbid.clone()))
                    .map(|g| BandRow {
                        mbid: g.mbid,
                        name: g.name,
                    })
                    .collect();
                store_bands(&mbid, &bands);
                apply_bands(&mbid, bands);
            }
            Err(e) => {
                log::debug!("[qbz-qt] musician bands unavailable for mbid {mbid}: {e}");
            }
        }
    });
}

/// Keyed on the MBID rather than on a generation, deliberately: bands belong to
/// an identity, not to a visit. A coalesced request started for one surface is
/// therefore still correct for whichever surfaces are showing that identity when
/// it lands — and both can be, since the modal opens over the page.
fn apply_bands(mbid: &str, bands: Vec<BandRow>) {
    if bands.is_empty() {
        return;
    }
    let page_hit = with_state(|s| {
        if s.mbid != mbid {
            return false;
        }
        s.bands = bands.clone();
        true
    });
    if page_hit {
        publish_page();
    }
    let modal_hit = {
        let mut guard = MODAL.lock().unwrap();
        match guard.as_mut() {
            Some(m) if m.mbid == mbid => {
                m.bands = bands;
                true
            }
            _ => false,
        }
    };
    if modal_hit {
        publish_modal();
    }
}

// ---------------------------------------------------------------------------
//  Appearances
// ---------------------------------------------------------------------------

/// Fill `art_path` from the disk cache and return the urls still missing.
/// `cached_path` is synchronous and memoised, so this is cheap to run twice.
fn attach_art(rows: &mut [AppearanceRow]) -> Vec<String> {
    let mut missing = Vec::new();
    for row in rows.iter_mut() {
        if row.artwork_url.is_empty() {
            continue;
        }
        row.art_path = crate::artwork_qt::cached_path(&row.artwork_url);
        if row.art_path.is_empty() {
            missing.push(row.artwork_url.clone());
        }
    }
    missing
}

/// One page of "Appears On".
///
/// `first` separates the two error arms Tauri keeps apart: the FIRST page
/// failing raises the page's error state (`MusicianPageView.svelte:62-64`), a
/// later page failing is `console.error`-only and leaves the grid alone
/// (`:84-86`).
fn fetch_appearances(generation: u64, name: String, role: String, offset: u32, first: bool) {
    let runtime = crate::app();
    crate::spawn(async move {
        let page = match runtime
            .core()
            .musicbrainz_get_musician_appearances(&name, &role, PAGE_SIZE, offset)
            .await
        {
            Ok(page) => page,
            Err(e) => {
                log::warn!("[qbz-qt] musician appearances failed for {name:?}@{offset}: {e}");
                fail(generation, first);
                return;
            }
        };

        let returned = page.albums.len() as u32;
        let total = page.total;
        let mut rows: Vec<AppearanceRow> = page
            .albums
            .into_iter()
            .map(|a| AppearanceRow {
                album_id: a.album_id,
                title: a.album_title,
                artist_name: a.artist_name,
                // The FULL date the backend returned — see the file header.
                release_date: a.year.unwrap_or_default(),
                role_on_album: a.role_on_album,
                artwork_url: a.album_artwork,
                art_path: String::new(),
            })
            .collect();
        let missing = attach_art(&mut rows);

        {
            let mut guard = MUSICIAN.lock().unwrap();
            let Some(s) = guard.as_mut() else { return };
            if s.generation != generation {
                return;
            }
            // Qobuz repeats rows across pages of the same search; the grid must
            // not show the same album twice.
            let seen: HashSet<String> = s.appearances.iter().map(|r| r.album_id.clone()).collect();
            s.appearances
                .extend(rows.into_iter().filter(|r| !seen.contains(&r.album_id)));
            s.offset = offset.saturating_add(returned);
            if first {
                // Tauri reads the total ONCE and never refreshes it from a
                // later page (`MusicianPageView.svelte:69-89` sets neither) —
                // kept 1:1, because the total is the Qobuz album-search total
                // and a later page's copy is no more accurate.
                s.total = total;
            }
            // Tauri's rule is `appearances.length < totalAppearances` alone
            // (`:40`), and because the search total routinely exceeds what
            // pagination will actually return, that leaves a Load More button
            // that is infinitely clickable and does nothing once the pages run
            // out. `returned > 0` terminates it — the same guard
            // `artist_releases_qt.rs:430` and `label_qt.rs:820` carry. Proposed
            // divergence, flagged to the owner; drop the second term to be
            // bug-compatible with Tauri.
            s.has_more = s.appearances.len() < s.total && returned > 0;
            s.page_state = "ready".to_string();
            s.error.clear();
            s.loading_more = false;
        }
        publish_page();

        if missing.is_empty() {
            return;
        }
        // Second pass — covers that were not on disk yet. Same shape as
        // `artist_releases_qt::fetch`: download, re-attach over the WHOLE set,
        // republish.
        crate::artwork_qt::download_missing(missing).await;
        let mut all = {
            let mut guard = MUSICIAN.lock().unwrap();
            let Some(s) = guard.as_mut() else { return };
            if s.generation != generation {
                return;
            }
            std::mem::take(&mut s.appearances)
        };
        let _ = attach_art(&mut all);
        {
            let mut guard = MUSICIAN.lock().unwrap();
            let Some(s) = guard.as_mut() else { return };
            if s.generation != generation {
                // The take above already emptied the state for a musician the
                // user has left; `reset` has since refilled it, so dropping
                // `all` on the floor is correct.
                return;
            }
            s.appearances = all;
        }
        publish_page();
    });
}

/// The two error arms (see `fetch_appearances`). Generation-guarded like every
/// other write.
fn fail(generation: u64, first: bool) {
    let changed = with_state(|s| {
        if s.generation != generation {
            return false;
        }
        if first {
            s.page_state = "error".to_string();
            // Rust-translated and travelling inside a JSON document, so it
            // cannot re-translate through `trRev` — see `republish`.
            s.error = qbz_i18n::t("Failed to load appearances");
            s.has_more = false;
        } else {
            s.loading_more = false;
        }
        true
    });
    if changed {
        publish_page();
    }
}

// ---------------------------------------------------------------------------
//  Remaining invokables
// ---------------------------------------------------------------------------

/// The page's "Load more". Every guard lives here rather than in QML so a burst
/// of clicks cannot queue two pages.
pub fn load_more() {
    let Some((generation, name, role, offset)) = with_state(|s| {
        if s.page_state != "ready" || s.loading_more || !s.has_more {
            return None;
        }
        s.loading_more = true;
        Some((s.generation, s.name.clone(), s.role.clone(), s.offset))
    }) else {
        return;
    };
    publish_page();
    fetch_appearances(generation, name, role, offset, false);
}

/// An "Appears On" card. Straight through to the shell's album router, which
/// records its own nav entry and is a no-op offline.
pub fn open_album(album_id: String) {
    if album_id.is_empty() {
        return;
    }
    crate::open_album(album_id);
}

/// A "Bands & Projects" chip.
///
/// The group arrives as an MBID and there is no MBID -> Qobuz resolver
/// anywhere in the tree, so the chip resolves the group by NAME through the
/// same dispatcher every other consumer uses: a group Qobuz knows opens its
/// artist page, one it does not opens the modal. That is strictly more than
/// §5.3's "navigates when the group resolves, inert otherwise" — there is no
/// inert state left — and it costs no new machinery.
///
/// The role literal is `"Band"`, matching Tauri's Member-Of call site
/// (`ArtistDetailView.svelte`), and it is deliberately NOT translated: the role
/// string is echoed back as `role_on_album` on every appearance card, and Tauri
/// prints English there in every locale.
pub fn open_band(mbid: String) {
    // The chip exists on BOTH surfaces, so both are searched — the modal first,
    // since when it is open it is the one the user is clicking in.
    let name = {
        let guard = MODAL.lock().unwrap();
        guard.as_ref().and_then(|m| {
            m.bands
                .iter()
                .find(|b| b.mbid == mbid)
                .map(|b| b.name.clone())
        })
    }
    .or_else(|| {
        with_state(|s| {
            s.bands
                .iter()
                .find(|b| b.mbid == mbid)
                .map(|b| b.name.clone())
        })
    });
    let Some(name) = name else {
        log::debug!("[qbz-qt] musician band chip: no band with mbid {mbid} in the open document");
        return;
    };
    resolve_and_open(name, "Band".to_string());
}

/// Escape / X / backdrop / a parent modal closing — all four dismissal paths
/// land here (contract §5.2). Only the document is cleared; the page state is
/// left alone so a modal opened over a loaded page does not destroy it.
/// Is the musician modal up? Sampled by `hotkeys_bridge::handle_escape` for
/// the §1.2 Escape stack.
///
/// It must be asked of the OWNER of the state, not of the QML overlay: the
/// modal is a self-gated global item that is always instantiated, so "is it
/// visible" is not a thing any other surface can see. Reading `MODAL` here is
/// the same shape `search_qt::cortinilla_open()` and
/// `immersive_bridge::is_open()` already use.
pub fn modal_open() -> bool {
    MODAL.lock().unwrap().is_some()
}

pub fn close_modal() {
    let was_open = MODAL.lock().unwrap().take().is_some();
    if was_open {
        publish_modal();
    }
}

/// Live language switch. The only Rust-built translated string inside either
/// document is the page's error line, so this is a no-op unless the page is
/// standing in its error state — but it is called unconditionally from
/// `apply_language` for the same reason `artist_releases_qt::republish` is:
/// `LAST_DETAIL` only ever latches "album" or "artist" and cannot reach here.
///
/// The modal carries no Rust-translated string at all (its explanatory copy is
/// chosen in QML from `confidence`), so it is not republished.
pub fn republish() {
    let has_doc = with_state(|s| {
        if s.name.is_empty() {
            return false;
        }
        if s.page_state == "error" {
            s.error = qbz_i18n::t("Failed to load appearances");
        }
        true
    });
    if has_doc {
        publish_page();
    }
}
