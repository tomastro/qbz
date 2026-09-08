//! Per-user artist-blacklist lifecycle + access wrapper — Slint-free port of
//! `crates/qbz/src/artist_blacklist.rs` (1:1; only the log tag changed).
//!
//! A process-global singleton over the headless
//! `qbz_app::settings::artist_blacklist::BlacklistService` (ADR-006: all model
//! logic — schema, O(1) lookup set, enable flag, mutations — lives in
//! `qbz-app`; this module only owns the per-user store lifecycle and the thin
//! accessors the Qt surfaces call). The shared service is consumed AS-IS.
//!
//! Lifecycle mirrors `fav_cache_qt`: a process-global `Mutex<Option<Service>>`
//! bound per session via [`init_for_user`] / [`teardown`], next to the other
//! per-user stores. The service keeps its own in-memory `HashSet` + enabled
//! flag, so reads never round-trip SQLite; there is no separate cache here and
//! — matching `fav_cache_qt` — no change-notify mechanism: callers re-read
//! after mutating (`blacklist_qt::publish` re-pushes the document, and the
//! `blacklistChanged` signal settles the optimistic glyphs).
//!
//! This REPLACES the per-publish throwaway service that
//! `settings_qt::devtools::blacklist_counts` used to open (a fresh SQLite open
//! + schema init + two set loads on the Qt thread, per settings publish, and
//! unusable for mutations because every other reader in the process stayed
//! stale).
//!
//! Fail-open everywhere: with no session bound (`None`), checks behave as "not
//! blacklisted" / "enabled", snapshots are empty, and mutations return the exact
//! Tauri error string so the UI shows the same message.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use qbz_app::settings::artist_blacklist::{
    BlacklistService, BlacklistedAlbum, BlacklistedArtist, DB_FILE_NAME,
};

/// Per-user blacklist service. `None` outside an active session (online or
/// offline); pure fail-open behavior in that window.
static SERVICE: Mutex<Option<BlacklistService>> = Mutex::new(None);

/// The exact error string the Tauri build returns for a mutation attempted
/// with no active session. Kept verbatim so the UI surfaces the same message.
const NO_SESSION_ERR: &str = "No active session - please log in";

// ---------------------------------------------------------------------------
// Lifecycle (mirrors fav_cache_qt::{init_for_user, teardown})
// ---------------------------------------------------------------------------

/// Bind the per-user store from `<dir>/artist_blacklist.db`. Called on every
/// session activation — login, restore, AND offline entry — next to
/// `fav_cache_qt::init_for_user`. Best-effort: a store-open failure logs and
/// leaves the singleton `None` (fail-open: nothing is blacklisted, the feature
/// reads as enabled, never blocks entry). The offline binding is NOT optional:
/// it is the fix for the Tauri gap where the blacklist was never initialized in
/// offline mode and blacklisted artists leaked into the offline surfaces.
pub fn init_for_user(base_dir: &Path) {
    let db_path = base_dir.join(DB_FILE_NAME);
    match BlacklistService::new(&db_path) {
        Ok(service) => {
            if let Ok(mut guard) = SERVICE.lock() {
                *guard = Some(service);
            }
        }
        Err(e) => log::error!("[qbz-qt] artist blacklist store open failed: {e}"),
    }
}

/// Drop the per-user store on logout. Mirrors `fav_cache_qt::teardown` — the
/// next account must not inherit this one's blacklist.
pub fn teardown() {
    if let Ok(mut guard) = SERVICE.lock() {
        *guard = None;
    }
}

// ---------------------------------------------------------------------------
// Accessors (fail-open when no session is bound)
// ---------------------------------------------------------------------------

/// Run a closure against the bound service, or `default` when there is none /
/// the lock is poisoned.
fn with_service<T>(default: T, f: impl FnOnce(&BlacklistService) -> T) -> T {
    SERVICE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(f))
        .unwrap_or(default)
}

/// Settings > Import / Export: the bound store as a portable bundle.
pub fn export_portable() -> Result<qbz_app::settings::blacklist_portable::BlacklistBundle, String> {
    with_service(Err("no blacklist store is bound".to_string()), |s| {
        qbz_app::settings::blacklist_portable::export(s)
    })
}

/// Settings > Import / Export: additive merge of a portable bundle into the
/// bound store (see `blacklist_portable::import` for what "additive" means).
pub fn import_portable(
    bundle: &qbz_app::settings::blacklist_portable::BlacklistBundle,
) -> Result<qbz_app::settings::blacklist_portable::ImportReport, String> {
    with_service(Err("no blacklist store is bound".to_string()), |s| {
        qbz_app::settings::blacklist_portable::import(s, bundle)
    })
}

/// True when the artist id is blacklisted (and the feature is enabled).
/// Fail-open `false` when no session is bound.
pub fn is_blacklisted(artist_id: u64) -> bool {
    with_service(false, |s| s.is_blacklisted(artist_id))
}

/// True when the string-form artist id parses and is blacklisted. Non-numeric
/// ids (local artists) are never blacklisted. For row code that carries string
/// ids — which, in this port, is every id that crosses the bridge.
pub fn is_blacklisted_id_str(artist_id: &str) -> bool {
    let Ok(id) = artist_id.parse::<u64>() else {
        return false;
    };
    is_blacklisted(id)
}

/// True when the album id is blocked (and the feature is enabled). Album ids
/// are alphanumeric strings; an empty id never matches. Fail-open `false` when
/// no session is bound. Orthogonal to artist blocking — an album is hidden by
/// its OWN id regardless of its artist.
pub fn is_album_blacklisted(album_id: &str) -> bool {
    if album_id.is_empty() {
        return false;
    }
    with_service(false, |s| s.is_album_blacklisted(album_id))
}

/// Card-grid predicate: `true` when an album-card-shaped row (string album id +
/// string primary-artist id) should be hidden from a grid/carousel. Honors the
/// enabled gate + no-session fail-open via the underlying checks. Album axis
/// (own id) OR artist axis (primary-artist id). Use for the read-only album
/// grids that map an `AlbumCard`-like row rather than a typed `Album`.
// Consumed by the data-layer filter slice (spec 03 §9.2, items F5/F9/F11/F14),
// which lands separately from the manager.
pub fn card_blacklisted(album_id: &str, artist_id: &str) -> bool {
    is_album_blacklisted(album_id) || is_blacklisted_id_str(artist_id)
}

/// Stamp value for a track row's `isBlacklisted` cell. The single rule every
/// track controller (album / playlist / favorites / the four Q-mixes) reuses so
/// render and the queue filters agree on what "blacklisted" means per row:
///
/// - **HARD local/Plex guard** — a non-Qobuz `source` is NEVER blacklisted
///   (local copies with a numeric Qobuz id must still stay playable).
///   `qobuz_download` rows render `source == "qobuz"`, so they are treated as
///   Qobuz here — that matches Tauri (VTL keys on `!isLocal`).
/// - Resolve the artist from the candidate string ids in order; the first
///   non-empty, numeric, blacklisted id wins (performer OR composer; album rows
///   that lack a performer fall back to the album's primary artist).
/// - Missing / zero / non-numeric ids => fail-open (`false`).
///
/// The enabled-flag gate and the no-session fail-open live in
/// [`is_blacklisted`], so this never blocks when the feature is off or no
/// session is bound.
///
/// Live re-stamp contract: there is no change-notify here (the fav_cache
/// pattern). Every controller calls this at LOAD time, so navigating to a view
/// always shows correct state; to refresh an ALREADY-loaded list after a
/// mutation, the mutation site re-runs that controller's existing reload path.
/// There is intentionally no global listener/observer.
// Consumed by the data-layer filter slice (spec 03 §9.2, items F7/F12/F15-F18).
pub fn stamp_row(source: &str, artist_ids: &[&str], album_id: Option<&str>) -> bool {
    // Local / Plex / ephemeral rows are protected — never blacklisted.
    if source != "qobuz" {
        return false;
    }
    // Album axis (orthogonal): the row's own album id being blocked drops it
    // regardless of artist. Then the artist axis (performer/composer/primary).
    if album_id.is_some_and(is_album_blacklisted) {
        return true;
    }
    artist_ids.iter().any(|id| is_blacklisted_id_str(id))
}

/// THE single queue/playback predicate. Returns `true` when this track should
/// be DROPPED from any play / shuffle / queue-next / queue-later / radio
/// builder. Implemented in terms of the exact same source-guard + per-id check
/// as [`stamp_row`] so the queue filter and the row greyout can NEVER diverge:
/// a row that greys out is the row that drops from the queue, and vice versa.
///
/// - **HARD local/Plex guard** — delegates to [`stamp_row`]'s guard by passing
///   `source` through.
/// - Performer OR composer — pass both numeric ids; either one being
///   blacklisted drops the track.
/// - Missing / `None` ids => fail-open (`false`), so id-less tracks always play.
// Consumed by the data-layer filter slice (spec 03 §9.2, item F18 — playback).
pub fn is_track_blacklisted(
    source: &str,
    performer_id: Option<u64>,
    composer_id: Option<u64>,
    album_id: Option<&str>,
) -> bool {
    // Reuse stamp_row's guard + checks by funneling the numeric ids through the
    // same string path — the SINGLE underlying predicate shared with rendering.
    // The album id (a blocked album hides all its tracks) is passed straight
    // through, so render greyout and queue-drop can never diverge.
    let performer = performer_id.map(|id| id.to_string()).unwrap_or_default();
    let composer = composer_id.map(|id| id.to_string()).unwrap_or_default();
    stamp_row(source, &[performer.as_str(), composer.as_str()], album_id)
}

/// True when the blacklist feature is enabled. Default-enabled (`true`) when no
/// session is bound — the one read that fail-opens to `true`.
pub fn is_enabled() -> bool {
    with_service(true, |s| s.is_enabled())
}

/// Snapshot of the full blacklisted-id set, for `qbz_core::search_all`-style
/// filtering. Empty when no session is bound. Derived from `get_all` so it
/// reflects the persisted rows (ignores the enabled flag — callers gate on
/// [`is_enabled`] separately).
// Consumed by the data-layer filter slice (spec 03 §9.2, items F1/F3/F6/F8...).
pub fn ids_snapshot() -> HashSet<u64> {
    with_service(HashSet::new(), |s| {
        s.get_all()
            .map(|list| list.into_iter().map(|a| a.artist_id).collect())
            .unwrap_or_default()
    })
}

/// All blacklisted artists (name-sorted, `COLLATE NOCASE`), for the manager
/// view. Empty on no session or query error.
pub fn get_all() -> Vec<BlacklistedArtist> {
    with_service(Vec::new(), |s| s.get_all().unwrap_or_default())
}

/// Count of blacklisted artists (ignores the enabled flag). `0` when no session
/// is bound.
pub fn count() -> usize {
    with_service(0, |s| s.count())
}

/// Snapshot of the full blocked-album-id set, for `qbz_core` album/track
/// filtering. Empty when no session is bound. Reflects persisted rows (ignores
/// the enabled flag — callers gate on [`is_enabled`] separately).
// Consumed by the data-layer filter slice (spec 03 §9.2, items F1/F3/F6/F8...).
pub fn album_ids_snapshot() -> HashSet<String> {
    with_service(HashSet::new(), |s| {
        s.get_all_albums()
            .map(|list| list.into_iter().map(|a| a.album_id).collect())
            .unwrap_or_default()
    })
}

/// All blocked albums (title-sorted, `COLLATE NOCASE`), for the manager view.
/// Empty on no session or query error.
pub fn get_all_albums() -> Vec<BlacklistedAlbum> {
    with_service(Vec::new(), |s| s.get_all_albums().unwrap_or_default())
}

/// Count of blocked albums (ignores the enabled flag). `0` when no session is
/// bound.
pub fn album_count() -> usize {
    with_service(0, |s| s.album_count())
}

// ---------------------------------------------------------------------------
// Mutations (Err with the Tauri "no active session" string when unbound)
// ---------------------------------------------------------------------------

/// Run a mutation against the bound service, returning the Tauri "no active
/// session" error string when there is none / the lock is poisoned.
fn mutate(f: impl FnOnce(&BlacklistService) -> Result<(), String>) -> Result<(), String> {
    match SERVICE.lock() {
        Ok(guard) => match guard.as_ref() {
            Some(service) => f(service),
            None => Err(NO_SESSION_ERR.into()),
        },
        Err(_) => Err(NO_SESSION_ERR.into()),
    }
}

/// Add an artist to the blacklist.
pub fn add(artist_id: u64, artist_name: &str, notes: Option<&str>) -> Result<(), String> {
    mutate(|s| s.add(artist_id, artist_name, notes))
}

/// Remove an artist from the blacklist.
pub fn remove(artist_id: u64) -> Result<(), String> {
    mutate(|s| s.remove(artist_id))
}

/// Toggle the global enable flag.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    mutate(|s| s.set_enabled(enabled))
}

/// Clear all blacklisted artists (leaves the enabled flag + albums untouched).
pub fn clear_all() -> Result<(), String> {
    mutate(|s| s.clear_all())
}

/// Add an album to the blacklist.
pub fn add_album(
    album_id: &str,
    album_title: &str,
    artist_name: &str,
    cover_url: &str,
    notes: Option<&str>,
) -> Result<(), String> {
    mutate(|s| s.add_album(album_id, album_title, artist_name, cover_url, notes))
}

/// Remove an album from the blacklist.
pub fn remove_album(album_id: &str) -> Result<(), String> {
    mutate(|s| s.remove_album(album_id))
}

/// Clear all blocked albums (leaves the enabled flag + artists untouched).
pub fn clear_all_albums() -> Result<(), String> {
    mutate(|s| s.clear_all_albums())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp dir under the system temp root (no `tempfile` dev-dep on
    /// qbz-qt). Created here, removed at the end of the test.
    fn unique_temp_dir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("qbz-qt-blacklist-test-{nanos}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// One combined test: the singleton is process-global, so splitting into
    /// parallel tests would let them clobber each other. Covers the full
    /// round-trip: empty snapshot + default-enabled after init, add reflected in
    /// both check + snapshot, then teardown restores the fail-open state.
    #[test]
    fn lifecycle_roundtrip() {
        let dir = unique_temp_dir();

        init_for_user(&dir);
        assert!(ids_snapshot().is_empty(), "fresh store has no ids");
        assert!(is_enabled(), "blacklist defaults to enabled");
        assert!(!is_blacklisted(42), "nothing blacklisted yet");

        add(42, "X", None).expect("add succeeds with a bound store");
        assert!(is_blacklisted(42), "added id is blacklisted");
        assert!(is_blacklisted_id_str("42"), "string-id check matches");
        assert!(
            !is_blacklisted_id_str("not-a-number"),
            "local ids never match"
        );
        assert!(
            ids_snapshot().contains(&42),
            "snapshot contains the added id"
        );
        assert_eq!(count(), 1);

        // Album axis: orthogonal, String-keyed, shares the enabled flag.
        assert!(!is_album_blacklisted("zzz"), "nothing album-blocked yet");
        add_album("zzz", "Bogus", "X", "", None).expect("add_album succeeds");
        assert!(is_album_blacklisted("zzz"), "added album id is blocked");
        assert!(!is_album_blacklisted(""), "empty album id never matches");
        assert!(
            album_ids_snapshot().contains("zzz"),
            "album snapshot contains the added id"
        );
        assert_eq!(album_count(), 1);
        // A blocked album drops via the shared stamp predicate, artist-independent.
        assert!(
            stamp_row("qobuz", &[], Some("zzz")),
            "album-blocked row drops"
        );
        assert!(
            !stamp_row("local", &[], Some("zzz")),
            "non-qobuz row is protected even when album-blocked"
        );
        // The queue predicate funnels through the same guard.
        assert!(
            is_track_blacklisted("qobuz", Some(42), None, None),
            "performer-blacklisted track drops"
        );
        assert!(
            !is_track_blacklisted("local", Some(42), None, None),
            "non-qobuz track is protected"
        );
        assert!(card_blacklisted("zzz", "1"), "card drops on the album axis");
        assert!(
            card_blacklisted("aaa", "42"),
            "card drops on the artist axis"
        );
        assert_eq!(count(), 1, "album add did not touch the artist count");

        teardown();
        assert!(!is_blacklisted(42), "fail-open after teardown");
        assert!(ids_snapshot().is_empty(), "empty snapshot after teardown");
        assert!(
            !is_album_blacklisted("zzz"),
            "album fail-open after teardown"
        );
        assert!(album_ids_snapshot().is_empty(), "empty album snapshot");
        assert_eq!(count(), 0);
        assert_eq!(album_count(), 0);
        assert!(is_enabled(), "default-enabled after teardown");
        assert!(
            add(1, "Y", None).is_err(),
            "mutation with no session returns the Tauri error string"
        );
        assert!(
            add_album("a", "b", "c", "", None).is_err(),
            "album mutation with no session returns the error string"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
