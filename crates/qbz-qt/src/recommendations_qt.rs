//! Discover > Recommendations (the 4th tab) — Slint-free port of
//! `crates/qbz/src/external_reco.rs`.
//!
//! The recommendation LOGIC is not reimplemented here: `qbz-external-reco` is
//! the frontend-agnostic engine (ADR-006) and this module is only its driver —
//! it assembles `RecoInputs`, calls the per-row builders, maps the resolved
//! rows onto the same `HomeSection` / `HomeCard` transport the other Discover
//! tabs use, and publishes progressively.
//!
//! Shared with the album page's Last.fm row (`external_reco_qt`): the
//! `RecoCatalog` adapter over `QbzCore`, the daily rotation seed, and — the
//! point of the exercise — the SAME `RecoCache` database, so a similar-albums
//! resolution paid for on an album page is reused here and reopening Discover
//! costs no external traffic at all.
//!
//! ## Strictly opt-in
//!
//! Every Last.fm / ListenBrainz row is built through a handle that is `None`
//! unless the service is CONNECTED (`*_is_authed()` + a username). The engine's
//! builders early-return `Vec::new()` on a `None` handle, so an unconnected
//! service produces no request at all — and an empty row is simply absent from
//! the published document (no frame, no spinner). With NEITHER connected the
//! engine's cold-start regime paints the Qobuz editorial fallback instead
//! (`is_cold_start` -> `build_editorial`), 1:1 with Slint: that is catalog
//! data, not integration data. The whole module is lazy — nothing runs until
//! the user actually opens the Recommendations tab.
//!
//! ## Progressive paint
//!
//! Slint pushes one Slint model per row and repaints that row alone. The Qt
//! transport is ONE JSON document (`recoSectionsJson`), so "progressive" here
//! means: each builder, as it resolves, writes its rows into [`ROWS`] and
//! republishes the whole document. Rows that have not resolved yet are absent
//! rather than empty, which is what the view needs anyway.
//!
//! ## Deltas vs the Slint controller
//! - "Not interested" IS wired: the dismissal ids join the exclusion set fed
//!   to `compose_artist_rails` (the composition choke point — exclusions +
//!   cross-rail dedup + display cap), and [`apply_dismissal`] drops the card
//!   from the cached rails on the click. What is NOT ported is Slint's
//!   retained per-rail overflow, so the freed slot backfills on the next
//!   refresh rather than instantly.
//! - The artist BLACKLIST store is still not consulted here (the reco
//!   dismissal store is a different one); the exclusion set is followed
//!   artists + dismissals.
//! - `LocalHistory.known_artist_ids` comes from the user's followed artists
//!   (the port has no local `reco` play-vector store); it seeds the "Deep cuts
//!   from artists you know" row exactly as the reference does with its larger
//!   played-or-favorited set.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use cxx_qt_lib::QString;
use qbz_external_reco::{
    build_deep_cut_albums, build_editorial, build_fresh_releases, build_rec_albums,
    build_rec_artists_common, build_rec_artists_recent, build_weekly_exploration,
    build_weekly_jams, compose_artist_rails, gather_history, is_cold_start, AlbumReco, ArtistReco,
    ExternalCarousels, LastFmHandle, ListenBrainzHandle, LocalHistory, RecoCache, RecoInputs,
    TrackReco, ARTIST_DISPLAY_CAP,
};
use qbz_integrations::lastfm::LastFmClient;
use qbz_integrations::listenbrainz::ListenBrainzClient;
use qbz_models::Artist;

use crate::home_qt::{HomeCard, HomeSection};

/// The resolved rows, in engine terms. Cleared on a forced refresh.
#[derive(Default, Clone)]
struct Rows {
    rec_artists_common: Vec<HomeCard>,
    rec_artists_recent: Vec<HomeCard>,
    rec_albums: Vec<HomeCard>,
    fresh_releases: Vec<HomeCard>,
    weekly_exploration: Vec<HomeCard>,
    weekly_jams: Vec<HomeCard>,
    deep_cut_albums: Vec<HomeCard>,
    top_albums: Vec<HomeCard>,
    top_artists: Vec<HomeCard>,
}

static ROWS: Mutex<Option<Rows>> = Mutex::new(None);
/// A build has completed at least once this session (the lazy latch).
static LOADED: AtomicBool = AtomicBool::new(false);
/// A build is in flight (guards against the tab being re-entered mid-load).
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

fn rows_snapshot() -> Rows {
    ROWS.lock().ok().and_then(|g| g.clone()).unwrap_or_default()
}

fn mutate(f: impl FnOnce(&mut Rows)) {
    if let Ok(mut g) = ROWS.lock() {
        f(g.get_or_insert_with(Rows::default));
    }
}

// ---------------------------------------------------------------------------
// Row -> transport mapping
// ---------------------------------------------------------------------------

fn card_from_artist(a: &ArtistReco) -> HomeCard {
    HomeCard {
        // Pin badge state from the per-user store (Slint keeps the reco
        // models live through `set_artist_row_pinned`); [`apply_pin_change`]
        // keeps it live here after the build.
        is_pinned: crate::sidebar_qt::is_pinned("artist", &a.qobuz_artist_id.to_string()),
        // Follow state from the favourite-id cache, like every other producer.
        is_favorite: crate::fav_cache_qt::is_artist_favorite(a.qobuz_artist_id),
        id: a.qobuz_artist_id.to_string(),
        title: a.name.clone(),
        // ArtistCard renders `item.subtitle` as the muted "Similar to …" line.
        subtitle: a.subtitle.clone(),
        art_url: a.image_url.clone(),
        item_kind: "artist".to_string(),
        ..HomeCard::default()
    }
}

fn card_from_album(a: &AlbumReco) -> HomeCard {
    HomeCard {
        is_pinned: crate::sidebar_qt::is_pinned("album", &a.qobuz_album_id),
        is_favorite: crate::fav_cache_qt::is_album_favorite(&a.qobuz_album_id),
        id: a.qobuz_album_id.clone(),
        title: a.title.clone(),
        artist: a.artist.clone(),
        artist_id: a.artist_id.clone(),
        year: a.year.clone(),
        quality_tier: a.quality_tier.clone(),
        quality_label: a.quality_label.clone(),
        art_url: a.artwork_url.clone(),
        ..HomeCard::default()
    }
}

fn card_from_track(t: &TrackReco) -> HomeCard {
    HomeCard {
        is_favorite: crate::fav_cache_qt::contains_track(t.qobuz_track_id),
        id: t.qobuz_track_id.to_string(),
        title: t.title.clone(),
        artist: t.artist.clone(),
        art_url: t.artwork_url.clone(),
        ..HomeCard::default()
    }
}

fn section(id: &str, title: String, kind: &str, items: Vec<HomeCard>) -> Option<HomeSection> {
    if items.is_empty() {
        return None;
    }
    Some(HomeSection {
        id: id.to_string(),
        title,
        kind: kind.to_string(),
        hint: String::new(),
        endpoint: String::new(),
        items,
    })
}

/// The two rows whose CONTENT does not travel in the document. See
/// [`publish_weeklies`] for why.
const WEEKLY_EXPLORATION: &str = "weeklyExploration";
const WEEKLY_JAMS: &str = "weeklyJams";

/// An ordering SLOT: present in the document at its configured position, with
/// no rows in it. The rows arrive on their own property.
///
/// Unlike [`section`] this never self-hides on emptiness — that is the whole
/// point. A slot that disappeared while empty and reappeared when filled would
/// change the document, which is exactly the republish this split exists to
/// avoid. The QML side hides an empty one instead (the same
/// `PinnedState.items.length > 0` gate the other out-of-document rails use).
fn slot(id: &str, title: String, kind: &str) -> Option<HomeSection> {
    Some(HomeSection {
        id: id.to_string(),
        title,
        kind: kind.to_string(),
        hint: String::new(),
        endpoint: String::new(),
        items: Vec::new(),
    })
}

/// The tab's lineup, in ExternalRecoView.slint order. Empty rows are ABSENT
/// (self-hide), never an empty frame — EXCEPT the two Weekly rows, which are
/// ordering slots (see [`slot`] / [`publish_weeklies`]).
fn sections(rows: &Rows) -> Vec<HomeSection> {
    [
        section(
            "recArtistsCommon",
            qbz_i18n::t("More like the artists you love"),
            "artists",
            rows.rec_artists_common.clone(),
        ),
        section(
            "recArtistsRecent",
            qbz_i18n::t("Based on what you've been into lately"),
            "artists",
            rows.rec_artists_recent.clone(),
        ),
        section(
            "recAlbums",
            qbz_i18n::t("Recommended Albums"),
            "album",
            rows.rec_albums.clone(),
        ),
        section(
            "freshReleases",
            qbz_i18n::t("Fresh Releases"),
            "album",
            rows.fresh_releases.clone(),
        ),
        slot(
            WEEKLY_EXPLORATION,
            qbz_i18n::t("Weekly Exploration"),
            "slimTracks",
        ),
        slot(WEEKLY_JAMS, qbz_i18n::t("Weekly Jams"), "slimTracks"),
        section(
            "deepCutAlbums",
            qbz_i18n::t("Deep cuts from artists you know"),
            "album",
            rows.deep_cut_albums.clone(),
        ),
        section(
            "topAlbums",
            qbz_i18n::t("Top albums on Qobuz"),
            "album",
            rows.top_albums.clone(),
        ),
        section(
            "topArtists",
            qbz_i18n::t("Popular artists on Qobuz"),
            "artists",
            rows.top_artists.clone(),
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

// ---------------------------------------------------------------------------
// Publish (+ the artwork dispatch every rail must join)
// ---------------------------------------------------------------------------

fn push(sections: &[HomeSection]) {
    let json = serde_json::to_string(sections).unwrap_or_else(|_| "[]".to_string());
    crate::home_bridge::ui(move |mut b| {
        b.as_mut()
            .set_reco_sections_json(QString::from(json.as_str()));
    });
}

fn set_loading(value: bool) {
    crate::home_bridge::ui(move |mut b| b.as_mut().set_reco_loading(value));
}

/// Publish the two Weekly rows on their OWN property, as
/// `{"weeklyExploration": [...], "weeklyJams": [...]}`.
///
/// WHY THEY ARE NOT IN THE DOCUMENT. They come from ListenBrainz and resolve
/// about a second AFTER the rest of the tab has already painted (measured
/// 2026-08-17: the cached blob paints at t+0, the weeklies land at t+0.9s,
/// and that is with BOTH of their own per-week caches hitting — the delay is
/// the playlist listing, not the tracks). Folding them into the document meant
/// that landing republished it, and a republish hands QML a brand-new array:
/// `QQuickItemView::setModel()` then tears down every rail's
/// QQmlDelegateModel and rebuilds it. Nine rails were being destroyed and
/// rebuilt to deliver two — visible as a flash a second after the tab settled,
/// and reported as "it loads, then it loads again".
///
/// This is the same split HomeView already runs for its three out-of-document
/// rails (pinned / radio / spotlight, see `foryou_qt.rs`), for the same
/// reason and with the same QML shape: the document carries the ordering slot,
/// the rows ride a dedicated property, and rebinding it re-creates ONLY those
/// delegates.
///
/// Covers are attached here rather than by the document pass, since the rows
/// are no longer in the document to be attached to. A late cover landing
/// republishes THIS property only — two rails, not nine.
fn publish_weeklies(download: bool) {
    let rows = rows_snapshot();
    let mut secs: Vec<HomeSection> = [
        section(
            WEEKLY_EXPLORATION,
            String::new(),
            "slimTracks",
            rows.weekly_exploration.clone(),
        ),
        section(
            WEEKLY_JAMS,
            String::new(),
            "slimTracks",
            rows.weekly_jams.clone(),
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    let missing = crate::artwork_qt::attach_cached(&mut secs);

    let mut doc = serde_json::Map::new();
    for sec in &secs {
        doc.insert(
            sec.id.clone(),
            serde_json::to_value(&sec.items).unwrap_or(serde_json::Value::Null),
        );
    }
    let json = serde_json::to_string(&doc).unwrap_or_else(|_| "{}".to_string());
    crate::home_bridge::ui(move |mut b| {
        b.as_mut()
            .set_reco_weekly_json(QString::from(json.as_str()));
    });

    if download && !missing.is_empty() {
        crate::spawn(async move {
            crate::artwork_qt::download_missing(missing).await;
            // download=false: the covers are on disk now, so this pass can
            // only attach — it can never spawn another download round.
            publish_weeklies(false);
        });
    }
}

/// Serialize the current rows, attach every cover already on disk, publish,
/// and (when `download`) fetch the misses in the background and republish —
/// the same dispatch `main::reload_home` runs for the other three tabs. A rail
/// that skipped this would render as a wall of empty frames.
fn publish_snapshot(download: bool) {
    let rows = rows_snapshot();
    let mut secs = sections(&rows);
    let missing = crate::artwork_qt::attach_cached(&mut secs);
    push(&secs);
    // Re-hand the Weekly rows alongside the document. A tab re-entry publishes
    // the document from memory, and this view is destroyed and rebuilt on
    // every entry — without this the two Weekly rails would come back empty
    // for the rest of the session.
    publish_weeklies(download);
    if download && !missing.is_empty() {
        crate::spawn(async move {
            crate::artwork_qt::download_missing(missing).await;
            // Was a second `publish_snapshot(false)`. Republishing for artwork
            // hands QML a brand-new array, and `QQuickItemView::setModel()`
            // then resets each rail's scroll offset and tears down its
            // QQmlDelegateModel — see the note at line 273 and the identical
            // reasoning in `HomeView.qml:161`. Every landing batch therefore
            // rebuilt all nine rails' delegates. The covers now travel as a
            // url-keyed PATCH, exactly as `foryou_qt::emit_foryou_art` does
            // for the For You rails and `myqbz_bridge` does for MyQBZ; the
            // document is left alone.
            //
            // The download itself is untouched: this changes only how a
            // resolved path is ANNOUNCED, never what gets fetched or cached,
            // so every other consumer of the shared image cache (the now-
            // playing bar, the queue, the `recent` rails) sees exactly what it
            // saw before.
            emit_reco_art(art_patch());
        });
    }
}

/// Every (art_url -> art_path) this tab's rows can resolve from the disk cache
/// right now. Attaches into a THROWAWAY copy of the sections, so it can never
/// mutate or republish the live document, and never downloads.
fn art_patch() -> BTreeMap<String, String> {
    let rows = rows_snapshot();
    let mut secs = sections(&rows);
    let _ = crate::artwork_qt::attach_cached(&mut secs);
    let mut patch: BTreeMap<String, String> = BTreeMap::new();
    for section in secs.iter() {
        for card in section.items.iter() {
            if !card.art_url.is_empty() && !card.art_path.is_empty() {
                patch.insert(card.art_url.clone(), card.art_path.clone());
            }
        }
    }
    patch
}

/// ONE emit per batch — the receiving QML map is a `var`, so it only notifies
/// on a new object reference, and an emit per url would copy the growing map
/// and invalidate every cover binding once per cover. Nothing paints later for
/// it: `download_missing().await` settles the whole batch first.
fn emit_reco_art(patch: BTreeMap<String, String>) {
    if patch.is_empty() {
        return;
    }
    let json = serde_json::to_string(&patch).unwrap_or_else(|_| "{}".to_string());
    crate::home_bridge::ui(move |mut b| {
        b.as_mut().reco_art_ready(QString::from(json.as_str()));
    });
}

/// Re-hand the resolved covers to a freshly mounted HomeView. HomeView is
/// destroyed and rebuilt on every navigation while this document is published
/// once per session, so without this the covers that arrived as a PATCH (i.e.
/// everything that was not already on disk at publish time) would come back
/// blank for the rest of the session — and the "still pending" probe would
/// stay true forever and pin the skeleton pulse Timer on. The exact trap
/// `foryou_qt::resolved_art_patch` exists to close.
///
/// Never downloads, never republishes; returns "" when nothing is resolved, so
/// a cold first mount emits nothing and cannot clear a map an in-flight
/// download is about to fill.
pub(crate) fn resolved_art_patch() -> String {
    if !LOADED.load(Ordering::SeqCst) {
        return String::new();
    }
    let patch = art_patch();
    if patch.is_empty() {
        return String::new();
    }
    log::debug!(
        "[qbz-qt] reco art re-handed to a fresh HomeView: {} covers",
        patch.len()
    );
    serde_json::to_string(&patch).unwrap_or_else(|_| "{}".to_string())
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Pin twin of `home_qt::apply_pin_change` for THIS tab's cache: flip the
/// `isPinned` flag on every cached row carrying `(kind, id)`. The
/// Recommendations tab is a separate document (`recoSectionsJson`) fed by
/// [`ROWS`], not by the Discover candidate cache, so the home-side patch
/// cannot reach it — and a row left stale here comes back the next time
/// this document is published (a tab re-entry, a cover landing, a language
/// switch) and un-does the badge.
///
/// It deliberately does NOT republish: the glyphs on screen are corrected
/// in place by `QbzLibrary.pinChanged`, which every card listens to, and
/// this tab's rails are instantiated as soon as it has been opened once —
/// so a publish here would tear down and rebuild its delegate models on
/// every pin click made ANYWHERE in the app. No pinned rail lives here, so
/// the cache patch is the whole job.
///
/// No-op before the first build: nothing is cached, and the eventual build
/// stamps the flags from the store itself.
pub(crate) fn apply_pin_change(kind: &str, id: &str, pinned: bool) {
    // Track rails (weekly_exploration / weekly_jams) draw no pin glyph —
    // tracks are not pinnable — so only the two card kinds reach the cache.
    if !LOADED.load(Ordering::SeqCst) || !matches!(kind, "album" | "artist") {
        return;
    }
    mutate(|rows| {
        let lists: Vec<&mut Vec<HomeCard>> = if kind == "album" {
            vec![
                &mut rows.rec_albums,
                &mut rows.fresh_releases,
                &mut rows.deep_cut_albums,
                &mut rows.top_albums,
            ]
        } else {
            vec![
                &mut rows.rec_artists_common,
                &mut rows.rec_artists_recent,
                &mut rows.top_artists,
            ]
        };
        for list in lists {
            for card in list.iter_mut() {
                if card.id == id {
                    card.is_pinned = pinned;
                }
            }
        }
    });
}

/// "Not interested": drop the artist from the cached rails RIGHT NOW.
///
/// The exclusion set above keeps a dismissed artist out of every future
/// rebuild; this is what makes the card leave the rail on the CLICK instead of
/// on the next one. The card's own comment used to say the rail would shed it
/// "on the next publish" — it never did, because nothing filtered on publish
/// either.
///
/// No backfill from a retained overflow (the reference keeps one per rail):
/// this port drops the overflow at composition, so the rail simply gets one
/// card shorter until the next refresh re-composes it from the pool. A hole is
/// not offered — the card is REMOVED, not blanked.
///
/// Un-dismissing (Settings > Blacklist > Recommendations) does not restore the
/// card live: there is nothing cached to restore it from. It comes back with
/// the next reco refresh, which is also what the reference does.
pub(crate) fn apply_dismissal(artist_id: &str) {
    if !LOADED.load(Ordering::SeqCst) {
        return;
    }
    mutate(|rows| {
        for list in [
            &mut rows.rec_artists_common,
            &mut rows.rec_artists_recent,
            &mut rows.top_artists,
        ] {
            list.retain(|card| card.id != artist_id);
        }
    });
    // `false`: no cover download pass — nothing new appeared, a card left.
    publish_snapshot(false);
}

/// Favourite twin of [`apply_pin_change`] for this tab's cache.
///
/// The reco document is published again on every tab re-entry, cover landing
/// and language switch, straight from [`ROWS`] — so a heart set on this tab
/// (or anywhere else, on a row this tab also shows) was undone by the next
/// one of those. Track rails are included: they draw hearts.
pub(crate) fn apply_favorite_change(kind: &str, id: &str, favorite: bool) {
    if !LOADED.load(Ordering::SeqCst) || !matches!(kind, "album" | "artist" | "track") {
        return;
    }
    mutate(|rows| {
        let lists: Vec<&mut Vec<HomeCard>> = match kind {
            "album" => vec![
                &mut rows.rec_albums,
                &mut rows.fresh_releases,
                &mut rows.deep_cut_albums,
                &mut rows.top_albums,
            ],
            "artist" => vec![
                &mut rows.rec_artists_common,
                &mut rows.rec_artists_recent,
                &mut rows.top_artists,
            ],
            _ => vec![&mut rows.weekly_exploration, &mut rows.weekly_jams],
        };
        for list in lists {
            for card in list.iter_mut().filter(|c| c.id == id) {
                card.is_favorite = favorite;
            }
        }
    });
}

/// Lazy first load — called when the Recommendations tab becomes visible.
/// Idempotent: a second open repaints from [`ROWS`] without touching the
/// network (and the engine's own results cache makes even a fresh process
/// cheap).
pub(crate) fn ensure_loaded() {
    if LOADED.load(Ordering::SeqCst) {
        // Republish so a tab re-entry repaints from memory (the document is a
        // property, so this is also what recovers after a language switch).
        publish_snapshot(true);
        return;
    }
    start(false);
}

/// The cache-window options the configurator offers, in HOURS. The index <->
/// hours mapping is the contract with the QbzSelect option order in
/// `DiscoverConfigModal.qml`, and it is the reference's own table verbatim
/// (`crates/qbz/src/discover_prefs.rs:100`) — the value is written into the
/// SHARED per-user prefs, so the two builds have to agree on it or a window
/// chosen in one reads back as a different one in the other.
const TTL_HOURS: [i64; 4] = [24, 36, 48, 72];

/// Stored hours -> select index. An unrecognised value resolves to index 2
/// (48 h), which is what the prefs store itself defaults to.
pub(crate) fn cache_ttl_index() -> i32 {
    let hours = crate::home_qt::load_prefs().reco_cache_ttl_hours;
    TTL_HOURS.iter().position(|&h| h == hours).unwrap_or(2) as i32
}

/// Configurator select -> persist -> republish the resolved index.
///
/// Republishing the RESOLVED index rather than trusting the one that came in
/// is what keeps the control honest when the write fails (no per-user store
/// because nobody is logged in): the select snaps back to what is actually
/// stored instead of showing a window that was never saved.
pub(crate) fn set_cache_ttl_index(index: i32) {
    let hours = *TTL_HOURS.get(index.clamp(0, 3) as usize).unwrap_or(&48);
    let Some(store) = crate::discover_config_qt::store() else {
        log::warn!("[qbz-qt] reco cache window: no per-user prefs store (not logged in?)");
        return;
    };
    let mut prefs = store.load();
    prefs.reco_cache_ttl_hours = hours;
    if let Err(e) = store.save(&prefs) {
        log::warn!("[qbz-qt] reco cache window: save failed: {e}");
        return;
    }
    log::info!("[qbz-qt] reco cache window set to {hours}h");
    publish_cache_ttl_index();
}

/// Push the stored window onto the bridge. Called at boot (so the select opens
/// on the right entry the FIRST time, not after a change) and after every
/// write.
pub(crate) fn publish_cache_ttl_index() {
    let index = cache_ttl_index();
    crate::home_bridge::ui(move |mut b| b.as_mut().set_reco_cache_ttl_index(index));
}

/// "Refresh now": drop the latch and rebuild every row, bypassing the results
/// blob (the engine still honours its per-week ListenBrainz cache).
pub(crate) fn refresh() {
    LOADED.store(false, Ordering::SeqCst);
    start(true);
}

fn start(force: bool) {
    if crate::offline_fwd::engine().status().is_offline() {
        log::info!("[qbz-qt] recommendations skipped (offline session)");
        return;
    }
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    set_loading(true);
    crate::spawn(async move {
        run(force).await;
        IN_FLIGHT.store(false, Ordering::SeqCst);
        LOADED.store(true, Ordering::SeqCst);
        set_loading(false);
    });
}

/// Followed-artist ids — the exclusion set for the artist rails AND the
/// "artists you know" seed for the deep-cut row (see the module POC-NOTEs).
async fn followed_artist_ids() -> HashSet<u64> {
    let runtime = crate::app();
    match runtime.core().get_favorites("artists", 100, 0).await {
        Ok(value) => {
            qbz_models::lenient::parse_items_array::<Artist>(&value, "artists", "reco artist")
                .into_iter()
                .map(|a| a.id)
                .collect()
        }
        Err(e) => {
            log::warn!("[qbz-qt] recommendations: followed artists fetch failed: {e}");
            HashSet::new()
        }
    }
}

/// Apply the two Recommended-Artist rails through the engine's composition
/// choke point (exclusions + cross-rail dedup + display cap), then paint.
fn apply_artist_rails(
    common_pool: Vec<ArtistReco>,
    recent_pool: Vec<ArtistReco>,
    excluded: &HashSet<u64>,
) {
    let (common, recent) =
        compose_artist_rails(common_pool, recent_pool, excluded, ARTIST_DISPLAY_CAP);
    mutate(|r| {
        r.rec_artists_common = common.visible.iter().map(card_from_artist).collect();
        r.rec_artists_recent = recent.visible.iter().map(card_from_artist).collect();
    });
    publish_snapshot(true);
}

async fn run(force: bool) {
    let cfg = crate::integrations_qt::scrobble_settings();

    use qbz_app::settings::scrobblers::HistoryService;
    let lastfm_client = LastFmClient::new();
    let lb_client = ListenBrainzClient::new();
    // History READ gate = the scrobbling gate (`*_active()`): a connected
    // account with scrobbling off is not read either. One rule, both
    // directions; the toggle's Settings text names it.
    let lastfm_allowed = cfg.history_read_allowed(HistoryService::LastFm);
    let lb_allowed = cfg.history_read_allowed(HistoryService::ListenBrainz);
    if lb_allowed {
        lb_client
            .restore_token(
                cfg.listenbrainz_token.clone(),
                cfg.listenbrainz_username.clone(),
            )
            .await;
    }
    // The core's client, not a private one: only the core instance carries
    // the `musicbrainz_enabled` opt-out (integrations_qt::start applies it),
    // and a disabled client answers every lookup with an error the builders
    // already treat as "no MusicBrainz" — zero requests leave the machine.
    let mb_client = crate::app().core().musicbrainz_client();

    // CONNECTED AND scrobbling on: a `None` handle is what makes the row
    // absent AND silent (the builders early-return before any request).
    let lastfm = if lastfm_allowed && !cfg.lastfm_username.is_empty() {
        Some(LastFmHandle {
            username: cfg.lastfm_username.clone(),
            client: &lastfm_client,
        })
    } else {
        None
    };
    let listenbrainz = if lb_allowed && !cfg.listenbrainz_username.is_empty() {
        Some(ListenBrainzHandle {
            username: cfg.listenbrainz_username.clone(),
            client: &lb_client,
        })
    } else {
        None
    };

    // The exclusion set is followed artists PLUS the reco dismissals. The
    // dismissals half was missing: "Not interested" wrote the store, toasted,
    // and NOTHING ever read `reco_dismiss_qt::ids_snapshot()` — the artist came
    // back on the very next rebuild (owner, 2026-08-16). It is the exclusion
    // set, not a post-filter, so the composition choke point can backfill the
    // freed slot from the pool instead of leaving a hole.
    let mut followed = followed_artist_ids().await;
    followed.extend(crate::reco_dismiss_qt::ids_snapshot());
    let catalog = crate::external_reco_qt::CoreRecoCatalog {
        runtime: crate::app(),
    };
    let cache = crate::external_reco_qt::cache_dir()
        .and_then(|dir| match RecoCache::open_at(&dir) {
            Ok(c) => {
                let _ = c.cleanup_expired();
                Some(c)
            }
            Err(e) => {
                log::warn!("[qbz-qt] reco cache open failed ({e}) — running uncached");
                None
            }
        })
        .map(Mutex::new);

    let inputs = RecoInputs {
        lastfm,
        listenbrainz,
        musicbrainz: mb_client.as_ref(),
        catalog: &catalog,
        cache: cache.as_ref(),
        local: LocalHistory {
            known_artist_ids: followed.clone(),
            ..Default::default()
        },
        rotation_seed: crate::external_reco_qt::rotation_seed(),
    };

    let source_key = format!(
        "results:lf={}:lb={}",
        inputs.lastfm.is_some(),
        inputs.listenbrainz.is_some()
    );
    log::info!(
        "[qbz-qt] recommendations: lastfm={} listenbrainz={} force={force}",
        inputs.lastfm.is_some(),
        inputs.listenbrainz.is_some(),
    );

    let ttl_secs = reco_cache_ttl_secs();

    // 1. Results cache — paint the non-weekly rows instantly from a fresh
    // blob. The two Weekly rows are NEVER trusted from it: they follow
    // ListenBrainz's own weekly cadence and have their own per-week cache, so
    // one transient empty build must not hide them for the whole window.
    if !force {
        let cached = inputs.cache.and_then(|c| {
            c.lock()
                .ok()
                .and_then(|g| g.get_results(&source_key, ttl_secs))
        });
        if let Some(json) = cached {
            if let Ok(result) = serde_json::from_str::<ExternalCarousels>(&json) {
                apply_all(result, &followed);
                build_and_apply_weeklies(&inputs).await;
                return;
            }
        }
    }

    // 2. Cache miss / stale / forced: build. Each branch paints as it resolves.
    let cold_start = is_cold_start(&inputs);
    let collector: Mutex<ExternalCarousels> = Mutex::new(ExternalCarousels::default());

    if cold_start {
        // No external source connected -> Qobuz editorial fallback (catalog
        // data; no integration is contacted).
        let (albums, artists) = build_editorial(&inputs).await;
        if let Ok(mut g) = collector.lock() {
            g.editorial_fallback = true;
            g.top_albums = albums.clone();
            g.top_artists = artists.clone();
        }
        mutate(|r| {
            r.top_albums = albums.iter().map(card_from_album).collect();
            r.top_artists = artists.iter().map(card_from_artist).collect();
        });
        publish_snapshot(true);
    } else {
        let history = gather_history(&inputs).await;
        let col = &collector;
        let b_artists = async {
            let (common_pool, recent_pool) = tokio::join!(
                build_rec_artists_common(&inputs, &history),
                build_rec_artists_recent(&inputs, &history),
            );
            if let Ok(mut g) = col.lock() {
                g.rec_artists_common = common_pool.clone();
                g.rec_artists_recent = recent_pool.clone();
            }
            apply_artist_rails(common_pool, recent_pool, &followed);
        };
        let b_albums = async {
            let r = build_rec_albums(&inputs, &history).await;
            if let Ok(mut g) = col.lock() {
                g.rec_albums = r.clone();
            }
            mutate(|rows| rows.rec_albums = r.iter().map(card_from_album).collect());
            publish_snapshot(true);
        };
        let b_fresh = async {
            let r = build_fresh_releases(&inputs).await;
            if let Ok(mut g) = col.lock() {
                g.fresh_releases = r.clone();
            }
            mutate(|rows| rows.fresh_releases = r.iter().map(card_from_album).collect());
            publish_snapshot(true);
        };
        let b_explore = async {
            let r = build_weekly_exploration(&inputs).await;
            if let Ok(mut g) = col.lock() {
                g.weekly_exploration = r.clone();
            }
            mutate(|rows| rows.weekly_exploration = r.iter().map(card_from_track).collect());
            publish_snapshot(true);
        };
        let b_jams = async {
            let r = build_weekly_jams(&inputs).await;
            if let Ok(mut g) = col.lock() {
                g.weekly_jams = r.clone();
            }
            mutate(|rows| rows.weekly_jams = r.iter().map(card_from_track).collect());
            publish_snapshot(true);
        };
        let b_deep = async {
            let r = build_deep_cut_albums(&inputs).await;
            if let Ok(mut g) = col.lock() {
                g.deep_cut_albums = r.clone();
            }
            mutate(|rows| rows.deep_cut_albums = r.iter().map(card_from_album).collect());
            publish_snapshot(true);
        };
        tokio::join!(b_artists, b_albums, b_fresh, b_explore, b_jams, b_deep);
    }

    // 3. Store the build for instant future opens. GUARD against poisoning the
    // cache with a transient ListenBrainz failure: if LB is connected but
    // every LB-sourced row came back empty, skip the write so the next open
    // re-fetches instead of hiding those rows for the whole window.
    if let Some(cache_mutex) = inputs.cache {
        let lb_all_empty = collector
            .lock()
            .map(|g| {
                g.weekly_exploration.is_empty()
                    && g.weekly_jams.is_empty()
                    && g.fresh_releases.is_empty()
            })
            .unwrap_or(true);
        if inputs.listenbrainz.is_some() && lb_all_empty {
            log::warn!(
                "[qbz-qt] ListenBrainz connected but all LB rows empty — skipping the \
                 results-cache write (likely transient; next open re-fetches)"
            );
        } else {
            let json = collector
                .lock()
                .ok()
                .and_then(|g| serde_json::to_string(&*g).ok());
            if let (Ok(guard), Some(json)) = (cache_mutex.lock(), json) {
                guard.put_results(&source_key, &json);
            }
        }
    }
}

/// Paint the NON-weekly rows from a cached blob (see `run`).
fn apply_all(r: ExternalCarousels, excluded: &HashSet<u64>) {
    mutate(|rows| {
        rows.rec_albums = r.rec_albums.iter().map(card_from_album).collect();
        rows.fresh_releases = r.fresh_releases.iter().map(card_from_album).collect();
        rows.deep_cut_albums = r.deep_cut_albums.iter().map(card_from_album).collect();
        rows.top_albums = r.top_albums.iter().map(card_from_album).collect();
        rows.top_artists = r.top_artists.iter().map(card_from_artist).collect();
    });
    apply_artist_rails(r.rec_artists_common, r.rec_artists_recent, excluded);
}

/// Rebuild + paint the two Weekly rows from their own per-week cache (one
/// ListenBrainz call + a SQLite read on a hit). Silent no-op when ListenBrainz
/// is not connected.
async fn build_and_apply_weeklies(inputs: &RecoInputs<'_>) {
    if inputs.listenbrainz.is_none() {
        return;
    }
    let (explore, jams) = tokio::join!(build_weekly_exploration(inputs), build_weekly_jams(inputs));
    mutate(|rows| {
        rows.weekly_exploration = explore.iter().map(card_from_track).collect();
        rows.weekly_jams = jams.iter().map(card_from_track).collect();
    });
    // NOT publish_snapshot: the document already carries both ordering slots
    // and has not changed, so republishing it would rebuild all nine rails to
    // deliver these two. See publish_weeklies.
    publish_weeklies(true);
}

/// The Recommendations cache window in SECONDS, from the shared per-user
/// discover prefs (`reco_cache_ttl_hours`, default 48h) — the same store the
/// section configurator writes.
fn reco_cache_ttl_secs() -> i64 {
    crate::home_qt::load_prefs().reco_cache_ttl_hours * 3600
}

#[cfg(test)]
mod tests {
    use super::*;

    // PRE-EXISTING FAILURE, fixed here rather than left red. `c02e7713d` split
    // the two Weekly rows out of the document and turned them into ordering
    // SLOTS — `slot()` deliberately never self-hides, and its doc comment says
    // why: a slot that vanished while empty and came back when filled would
    // change the document, which is the republish that split exists to avoid.
    // These two tests still asserted the pre-split behaviour and had been red
    // ever since. The CODE is right; the assertions were stale.
    #[test]
    fn empty_rows_publish_only_the_ordering_slots() {
        let ids: Vec<String> = sections(&Rows::default())
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec![WEEKLY_EXPLORATION, WEEKLY_JAMS]);
    }

    #[test]
    fn lineup_order_matches_the_slint_view() {
        let rows = Rows {
            rec_artists_common: vec![HomeCard::default()],
            rec_artists_recent: vec![HomeCard::default()],
            rec_albums: vec![HomeCard::default()],
            fresh_releases: vec![HomeCard::default()],
            weekly_exploration: vec![HomeCard::default()],
            weekly_jams: vec![HomeCard::default()],
            deep_cut_albums: vec![HomeCard::default()],
            top_albums: vec![HomeCard::default()],
            top_artists: vec![HomeCard::default()],
        };
        let ids: Vec<String> = sections(&rows).into_iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            vec![
                "recArtistsCommon",
                "recArtistsRecent",
                "recAlbums",
                "freshReleases",
                "weeklyExploration",
                "weeklyJams",
                "deepCutAlbums",
                "topAlbums",
                "topArtists",
            ]
        );
    }

    #[test]
    fn a_row_with_no_items_is_absent_not_empty() {
        let rows = Rows {
            rec_albums: vec![HomeCard::default()],
            ..Rows::default()
        };
        let ids: Vec<String> = sections(&rows).into_iter().map(|s| s.id).collect();
        // Every EMPTY row is gone; the two slots stay, at their place in the
        // lineup, because they are ordering and not content.
        assert_eq!(ids, vec!["recAlbums", WEEKLY_EXPLORATION, WEEKLY_JAMS]);
    }
}
