//! Shared favourite-id cache — Slint-free port of `crates/qbz/src/fav_cache.rs`.
//!
//! A process-wide set per entity kind, so every surface that draws a heart
//! (album header, artist header, track rows, label header, cards) can stamp
//! the right state at BUILD time without a fetch, and so a toggle can pick
//! its direction from something that is populated.
//!
//! Why this exists at all: the Qt port used to derive every heart — and the
//! toggle DIRECTION — from `library_qt::LIBRARY`, which is filled only by
//! `load_library_once()`, i.e. only after the user opens the Library view.
//! Before that, every read answered `false`: hearts drew empty on items that
//! ARE favourites and every click sent `favorite/create` (a server no-op),
//! so un-favouriting was impossible from any detail page or card.
//!
//! Disk-first seeding, exactly like the reference: [`init_for_user`] binds the
//! per-user `favorites_cache.db` (same file + schema the Slint app and Tauri
//! use — `qbz_app::settings::favorites_cache`) on session activation and loads
//! the ids from disk, so hearts are correct offline and from first paint. The
//! shell entry then refreshes from the network via the `set_all_*` writers,
//! and [`set`] keeps memory and disk in step with each optimistic toggle.
//!
//! Deviation from the reference, deliberate: the reference caches tracks /
//! albums / artists / awards; this caches tracks / albums / artists / LABELS.
//! Qt has no Award view (nothing would read it), and it DOES have a label
//! landing page whose follow state had no other correct source — the store
//! already carries a `favorite_labels` table, so the label set costs a set and
//! four call sites. See `label_qt::favorite_label_ids` for the other half.
//!
//! Second deviation: a fifth set for PLAYLISTS, whose authority is NOT
//! `favorites_cache.db` and NOT Qobuz — it is the per-user `library.db`
//! (`library_db_qt`), because `/favorite/create|delete` has no `playlist_ids`
//! param and the reference routes the playlist heart to
//! `db.set_playlist_favorite` instead. The set exists purely so a card row can
//! ask the question in O(1): `library_db_qt` opens a fresh `rusqlite`
//! Connection per call, so asking it once per row would mean 40 sqlite opens
//! to build one Discover playlist rail. Seeded on session activation, written
//! through by `library_qt::toggle_playlist_favorite` (which owns the db write),
//! never written to disk from here.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{LazyLock, Mutex, RwLock};

use qbz_app::settings::favorites_cache::FavoritesCacheStore;

static FAV_TRACKS: LazyLock<RwLock<HashSet<u64>>> = LazyLock::new(|| RwLock::new(HashSet::new()));
/// Album catalog ids are STRINGS (Qobuz mixes numeric and alphanumeric).
static FAV_ALBUMS: LazyLock<RwLock<HashSet<String>>> =
    LazyLock::new(|| RwLock::new(HashSet::new()));
static FAV_ARTISTS: LazyLock<RwLock<HashSet<u64>>> = LazyLock::new(|| RwLock::new(HashSet::new()));
static FAV_LABELS: LazyLock<RwLock<HashSet<u64>>> = LazyLock::new(|| RwLock::new(HashSet::new()));
/// Hearted PLAYLIST ids — mirror of the per-user `library.db` flag, not of
/// `favorites_cache.db` (see the module docs).
static FAV_PLAYLISTS: LazyLock<RwLock<HashSet<u64>>> =
    LazyLock::new(|| RwLock::new(HashSet::new()));

/// Per-user persistent id store. `None` until a session (online or offline)
/// is activated; pure in-memory behaviour in that window.
static STORE: Mutex<Option<FavoritesCacheStore>> = Mutex::new(None);

/// Bind the per-user store and seed every set from disk (works offline).
/// Called on every session activation — login, restore, offline entry — next
/// to `sidebar_qt::init_pinned`. Best-effort: a failure logs and leaves the
/// sets empty (hearts render unfavourited, entry is never blocked).
pub fn init_for_user(base_dir: &Path) {
    let store = match FavoritesCacheStore::new_at(base_dir) {
        Ok(store) => store,
        Err(e) => {
            log::error!("[qbz-qt] favorites cache store open failed: {e}");
            return;
        }
    };
    match store.get_favorite_track_ids() {
        Ok(ids) => write_u64(&*FAV_TRACKS, ids, "track"),
        Err(e) => log::warn!("[qbz-qt] favorites cache track disk seed failed: {e}"),
    }
    match store.get_favorite_album_ids() {
        Ok(ids) => {
            let set: HashSet<String> = ids.into_iter().collect();
            log::info!(
                "[qbz-qt] favorites cache: {} album ids seeded from disk",
                set.len()
            );
            if let Ok(mut guard) = FAV_ALBUMS.write() {
                *guard = set;
            }
        }
        Err(e) => log::warn!("[qbz-qt] favorites cache album disk seed failed: {e}"),
    }
    match store.get_favorite_artist_ids() {
        Ok(ids) => write_u64(&*FAV_ARTISTS, ids, "artist"),
        Err(e) => log::warn!("[qbz-qt] favorites cache artist disk seed failed: {e}"),
    }
    match store.get_favorite_label_ids() {
        Ok(ids) => write_u64(&*FAV_LABELS, ids, "label"),
        Err(e) => log::warn!("[qbz-qt] favorites cache label disk seed failed: {e}"),
    }
    if let Ok(mut guard) = STORE.lock() {
        *guard = Some(store);
    }
    // The playlist set comes from the OTHER per-user database — `library.db`,
    // in this same directory. `base_dir` is passed explicitly rather than
    // going through `library_db_qt`'s own `db_path()`, which resolves through
    // `UserDataPaths::load_last_user_id()`: this runs during session
    // activation, and depending on a value another subsystem may not have
    // persisted yet is how a store silently seeds itself for the WRONG user.
    let playlists: HashSet<u64> = crate::library_db_qt::favorite_playlist_ids_at(base_dir)
        .into_iter()
        .collect();
    log::info!(
        "[qbz-qt] favorites cache: {} playlist ids seeded from library.db",
        playlists.len()
    );
    if let Ok(mut guard) = FAV_PLAYLISTS.write() {
        *guard = playlists;
    }
}

/// Seed one numeric set from the store's `i64` rows (the three of them read
/// identically; the album set is the odd one out because its ids are text).
fn write_u64(cell: &RwLock<HashSet<u64>>, ids: Vec<i64>, what: &str) {
    let set: HashSet<u64> = ids
        .into_iter()
        .filter_map(|id| u64::try_from(id).ok())
        .collect();
    log::info!(
        "[qbz-qt] favorites cache: {} {what} ids seeded from disk",
        set.len()
    );
    if let Ok(mut guard) = cell.write() {
        *guard = set;
    }
}

/// Drop the per-user store and every set on logout.
pub fn teardown() {
    if let Ok(mut guard) = STORE.lock() {
        *guard = None;
    }
    if let Ok(mut g) = FAV_TRACKS.write() {
        g.clear();
    }
    if let Ok(mut g) = FAV_ALBUMS.write() {
        g.clear();
    }
    if let Ok(mut g) = FAV_ARTISTS.write() {
        g.clear();
    }
    if let Ok(mut g) = FAV_LABELS.write() {
        g.clear();
    }
    if let Ok(mut g) = FAV_PLAYLISTS.write() {
        g.clear();
    }
}

// ---------------------------------------------------------------------------
// Reads — O(1), allocation-free, safe to call per row while building a document
// ---------------------------------------------------------------------------

/// Heart state for one entity, keyed the way the bridge talks: the feed
/// `kind` string plus the string id.
///
/// THIS is the function every card/row PRODUCER calls, once per row, while it
/// builds a document — the port's equivalent of the reference's
/// `fav_cache::is_album_favorite` in `home::card_to_item` /
/// `foryou::album_items`. It is a hash lookup behind an `RwLock` read: no
/// allocation, no sqlite, no network, safe at 500 rows.
///
/// Do NOT use `library_qt::is_favorite` for card rows: that one ORs in a
/// linear scan of the (up to 10k-row) Library feed, which is the right answer
/// for a single detail header and O(rows x feed) for a rail.
///
/// `playlist` answers from the library.db mirror (see the module docs).
/// Unknown kinds and non-numeric ids (local / Plex rows) answer `false`;
/// those have their own authority (`LocalFavoritesService`).
pub fn is_favorite(kind: &str, id: &str) -> bool {
    match kind {
        "album" => is_album_favorite(id),
        "track" => id.parse::<u64>().map(contains_track).unwrap_or(false),
        "artist" => id.parse::<u64>().map(is_artist_favorite).unwrap_or(false),
        "label" => id.parse::<u64>().map(is_label_favorite).unwrap_or(false),
        "playlist" => id.parse::<u64>().map(is_playlist_favorite).unwrap_or(false),
        _ => false,
    }
}

pub fn contains_track(track_id: u64) -> bool {
    FAV_TRACKS
        .read()
        .map(|g| g.contains(&track_id))
        .unwrap_or(false)
}

/// Snapshot of the whole favourite-track set — the shape the queue publisher
/// wants (one lock, then N O(1) lookups).
pub fn all_tracks() -> HashSet<u64> {
    FAV_TRACKS.read().map(|g| g.clone()).unwrap_or_default()
}

pub fn is_album_favorite(album_id: &str) -> bool {
    FAV_ALBUMS
        .read()
        .map(|g| g.contains(album_id))
        .unwrap_or(false)
}

pub fn is_artist_favorite(artist_id: u64) -> bool {
    FAV_ARTISTS
        .read()
        .map(|g| g.contains(&artist_id))
        .unwrap_or(false)
}

pub fn is_label_favorite(label_id: u64) -> bool {
    FAV_LABELS
        .read()
        .map(|g| g.contains(&label_id))
        .unwrap_or(false)
}

/// Snapshot of the followed-label id set — the label page seeds its header
/// and its More-Labels cards from this when the network read is unavailable.
pub fn all_labels() -> HashSet<u64> {
    FAV_LABELS.read().map(|g| g.clone()).unwrap_or_default()
}

/// Is this playlist hearted? Mirror of the `library.db` flag — the SAME
/// authority `library_qt::toggle_playlist_favorite` reads to pick its
/// direction, so the drawn heart and the click cannot disagree.
pub fn is_playlist_favorite(playlist_id: u64) -> bool {
    FAV_PLAYLISTS
        .read()
        .map(|g| g.contains(&playlist_id))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Writes — memory first, then the per-user store (blocking rusqlite: the
// callers below already run on a blocking hop or a tokio worker)
// ---------------------------------------------------------------------------

/// Insert / remove one id, keeping the cache consistent with an optimistic
/// toggle, and mirror it to disk so the heart survives a restart. Kind-routed
/// like [`is_favorite`]; an unroutable kind is a no-op.
///
/// `playlist` is deliberately NOT routed here: its disk authority is
/// `library.db`, not `favorites_cache.db`, and writing it into the Qobuz
/// favourites file would create a second, divergent record of the same flag.
/// Use [`set_playlist`] (memory mirror only) right after the db write.
pub fn set(kind: &str, id: &str, favorite: bool) {
    // #690: the Library feed is loaded behind a session latch; a favorite
    // mutation is exactly the event that makes that snapshot stale. Every
    // caller of this funnel is a real user mutation (the warm paths use the
    // set_all_* replacers), so this never fires on seeding.
    crate::mark_library_stale();
    match kind {
        "album" => {
            mutate_str(&*FAV_ALBUMS, id, favorite);
            with_store(|s| {
                if favorite {
                    s.add_favorite_album(id)
                } else {
                    s.remove_favorite_album(id)
                }
            });
        }
        "track" | "artist" | "label" => {
            let Ok(numeric) = id.parse::<u64>() else {
                return;
            };
            // `&*` on purpose: the three statics are distinct LazyLock items,
            // so the arms have to be derefed to the shared `&RwLock<…>` here.
            let cell: &RwLock<HashSet<u64>> = match kind {
                "track" => &*FAV_TRACKS,
                "artist" => &*FAV_ARTISTS,
                _ => &*FAV_LABELS,
            };
            mutate_u64(cell, numeric, favorite);
            let disk = numeric as i64;
            with_store(|s| match (kind, favorite) {
                ("track", true) => s.add_favorite_track(disk),
                ("track", false) => s.remove_favorite_track(disk),
                ("artist", true) => s.add_favorite_artist(disk),
                ("artist", false) => s.remove_favorite_artist(disk),
                (_, true) => s.add_favorite_label(disk),
                (_, false) => s.remove_favorite_label(disk),
            });
        }
        _ => {}
    }
}

/// Mirror one playlist heart. MEMORY ONLY: `library.db` is the authority and
/// `library_db_qt::set_favorite_playlist` has already written it — this exists
/// so the next card built (and the open playlist header) reads what just
/// happened instead of the pre-toggle value.
pub fn set_playlist(playlist_id: u64, favorite: bool) {
    crate::mark_library_stale();
    mutate_u64(&*FAV_PLAYLISTS, playlist_id, favorite);
}

/// Full replace of the playlist mirror — the Library load already reads the
/// whole `library.db` set (`library_db_qt::favorite_playlist_ids`), so it
/// refreshes this for free instead of leaving the session seed to drift.
pub fn set_all_playlists(ids: HashSet<u64>) {
    if let Ok(mut g) = FAV_PLAYLISTS.write() {
        *g = ids;
    }
}

pub fn set_all_tracks(ids: HashSet<u64>) {
    let disk: Vec<i64> = ids.iter().map(|&id| id as i64).collect();
    with_store(|s| s.sync_favorite_tracks(&disk));
    if let Ok(mut g) = FAV_TRACKS.write() {
        *g = ids;
    }
}

pub fn set_all_albums(ids: HashSet<String>) {
    let disk: Vec<String> = ids.iter().cloned().collect();
    with_store(|s| s.sync_favorite_albums(&disk));
    if let Ok(mut g) = FAV_ALBUMS.write() {
        *g = ids;
    }
}

pub fn set_all_artists(ids: HashSet<u64>) {
    let disk: Vec<i64> = ids.iter().map(|&id| id as i64).collect();
    with_store(|s| s.sync_favorite_artists(&disk));
    if let Ok(mut g) = FAV_ARTISTS.write() {
        *g = ids;
    }
}

pub fn set_all_labels(ids: HashSet<u64>) {
    let disk: Vec<i64> = ids.iter().map(|&id| id as i64).collect();
    with_store(|s| s.sync_favorite_labels(&disk));
    if let Ok(mut g) = FAV_LABELS.write() {
        *g = ids;
    }
}

fn mutate_u64(cell: &RwLock<HashSet<u64>>, id: u64, favorite: bool) {
    if let Ok(mut g) = cell.write() {
        if favorite {
            g.insert(id);
        } else {
            g.remove(&id);
        }
    }
}

fn mutate_str(cell: &RwLock<HashSet<String>>, id: &str, favorite: bool) {
    if let Ok(mut g) = cell.write() {
        if favorite {
            g.insert(id.to_string());
        } else {
            g.remove(id);
        }
    }
}

/// Run a store write when a session is bound. Before activation the cache is
/// pure memory — that is the reference's contract too, not an error.
fn with_store(f: impl FnOnce(&FavoritesCacheStore) -> Result<(), String>) {
    let Ok(guard) = STORE.lock() else {
        return;
    };
    if let Some(store) = guard.as_ref() {
        if let Err(e) = f(store) {
            log::warn!("[qbz-qt] favorites cache disk update failed: {e}");
        }
    }
}
