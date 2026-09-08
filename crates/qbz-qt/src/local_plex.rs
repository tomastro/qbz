//! Plex glue for the Local Library (the Qt port of `plex_settings.rs` +
//! the browse-facing half of `plex_auth.rs`).
//!
//! ADR-006: the persisted model + SQLite store live in the shared
//! `qbz_app::settings::plex` module and the runtime lives in `qbz-plex`.
//! This file only owns:
//!
//!  1. a process-global `PlexSettingsState` bound to the ACTIVE user
//!     (`<data_dir>/qbz/users/<uid>/plex_settings.db` — the same file the
//!     Slint frontend writes, so a user authenticates Plex once);
//!  2. the gates the Local Library reads — `is_enabled` (settings UI only),
//!     `is_configured` (`canUsePlexRequests`: enabled + LAN address +
//!     resolved base url + token) and `cache_db_path` (the configured-only
//!     `ATTACH` path that turns the album query into a local+Plex UNION);
//!  3. Plex-cache reads mapped into the `qbz_library::LocalTrack` shape so
//!     Plex rows flow through the SAME mapping/playback pipeline as local
//!     files (`map_cached_to_local_track`, 1:1 with the Slint);
//!  4. the manual sync (`sync_now`) the Local Library header button fires:
//!     sections fetch -> save -> default-select-ALL -> prune -> per-section
//!     track fetch + cache save.
//!
//! The PIN/auth flow (`plex_auth_pin_*`) belongs to `plex_pin_qt`; successful
//! pairing and the manual-token fallback both enter this module through
//! `connect_manual`, while `disconnect` clears the shared store.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, RwLock};

use qbz_app::user_data::UserDataPaths;
use qbz_library::LocalTrack;

pub use qbz_app::settings::plex::{PlexSettings, PlexSettingsState};

/// Namespace for Plex track ids so they can never collide with a real
/// `local_tracks.id` (1:1 with the Slint's `PLEX_TRACK_ID_FLOOR`).
pub const PLEX_TRACK_ID_FLOOR: u64 = 1 << 40;

/// True when this row id belongs to the Plex namespace.
pub fn is_plex_track_id(id: i64) -> bool {
    (id as u64) & PLEX_TRACK_ID_FLOOR != 0
}

// ---------------------------------------------------------------------------
// The per-user store handle
// ---------------------------------------------------------------------------

static STATE: LazyLock<PlexSettingsState> = LazyLock::new(PlexSettingsState::new_empty);
/// The user id the store is currently bound to (None = session-less).
static BOUND: Mutex<Option<u64>> = Mutex::new(None);

/// The last read of [`settings`], kept so the accessor is not a database hit.
///
/// # Why this exists — it is a measured regression, not a micro-optimisation
///
/// `settings()` runs `ensure_bound()` (a `load_last_user_id` file read plus a
/// mutex) and then a SQLite query. That was fine while its callers were gates
/// asked a handful of times per view.
///
/// Design 02 §9 stage 4 put it on a PER-ROW path: `local_rows::art_ref` asks
/// the owning source what a row's artwork token means, and `PlexSource`'s
/// answer depends on whether Plex is connected — so every Plex row in the grid
/// paid a file read, a mutex and a query. Measured on the owner's library:
///
/// ```text
///     29 local rows   map 0.06 ms   ->  2 µs/row
///   1271 Plex rows    map 50-117 ms -> 39-92 µs/row
/// ```
///
/// 25-45x per row, on a document that is rebuilt on every visit to Local
/// Library. The grid had been tuned to a good place and this is what took it
/// away.
///
/// The cache is INVALIDATED by every writer in this module and by
/// `init_for_user` / `reset`, so a connect, a disconnect or a user switch is
/// still seen immediately — the property `PlexCredsGlue` documents (it reads
/// the live store rather than caching a copy) is preserved, because the
/// authority itself is what caches now.
static CACHE: RwLock<Option<PlexSettings>> = RwLock::new(None);

/// Drop the memo. Call from EVERY writer — a stale `enabled` here is a Plex
/// library that will not disappear when the user switches it off.
fn invalidate_cache() {
    if let Ok(mut c) = CACHE.write() {
        *c = None;
    }
}

/// Bind (or re-bind) the store to the ACTIVE user. Called lazily by every
/// accessor, so no glue in the login path is required; `init_for_user` is
/// provided for the shell to call explicitly on session activation.
fn ensure_bound() {
    let Some(uid) = UserDataPaths::load_last_user_id() else {
        return;
    };
    let mut bound = BOUND.lock().unwrap();
    if *bound == Some(uid) {
        return;
    }
    // `CACHE` is process-global too. A user switch observed through the lazy
    // path must discard the previous profile before any gate can answer from
    // it; otherwise the old user's enabled Plex leaks into the new session.
    invalidate_cache();
    let dir = crate::sidebar_qt::user_dir().unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_default()
            .join("qbz")
            .join("users")
            .join(uid.to_string())
    });
    match STATE.init_at(&dir) {
        Ok(()) => {
            *bound = Some(uid);
            log::info!("[qbz-qt] plex settings bound to user {uid}");
        }
        Err(e) => log::warn!("[qbz-qt] plex settings init failed: {e}"),
    }
}

/// Explicit bind on session activation (optional — accessors self-bind).
pub fn init_for_user(base_dir: &std::path::Path) {
    invalidate_cache();
    if let Err(e) = STATE.init_at(base_dir) {
        log::warn!("[qbz-qt] plex settings init failed: {e}");
        return;
    }
    *BOUND.lock().unwrap() = UserDataPaths::load_last_user_id();
    // Repair only the rare Plex rows whose flat track response omitted art.
    // It is background/LAN-only and the derived catalog receives one coalesced
    // catch-up when new covers land.
    start_album_artwork_repair();
}

/// Forget the binding (logout) so the next read re-binds to the new user.
pub fn reset() {
    cancel_sync();
    invalidate_cache();
    *BOUND.lock().unwrap() = None;
    let _ = STATE.teardown();
}

/// Current persisted settings (defaults when there is no session).
pub fn settings() -> PlexSettings {
    // Bind before consulting the memo: `last_user_id` can change between two
    // calls even when the explicit auth callback has not run yet.
    ensure_bound();
    if let Ok(c) = CACHE.read() {
        if let Some(s) = c.as_ref() {
            return s.clone();
        }
    }
    let fresh = STATE.get_settings().unwrap_or_default();
    if let Ok(mut c) = CACHE.write() {
        *c = Some(fresh.clone());
    }
    fresh
}

// ---------------------------------------------------------------------------
// URL helpers (`normalizePlexServerUrl` / `isLocalPlexAddress` /
// `resolvePlexBaseUrl`, ported from plex_auth.rs)
// ---------------------------------------------------------------------------

fn normalize_server_url(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

/// (scheme, host-without-port, port) for `http(s)://host[:port][/...]`.
fn parse_url(normalized: &str) -> Option<(String, String, Option<String>)> {
    let (scheme, rest) = if let Some(r) = normalized.strip_prefix("http://") {
        ("http", r)
    } else if let Some(r) = normalized.strip_prefix("https://") {
        ("https", r)
    } else {
        return None;
    };
    let authority_end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return None;
    }
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rfind(':') {
        Some(idx)
            if !authority[idx + 1..].is_empty()
                && authority[idx + 1..].chars().all(|c| c.is_ascii_digit()) =>
        {
            (
                authority[..idx].to_string(),
                Some(authority[idx + 1..].to_string()),
            )
        }
        _ => (authority.to_string(), None),
    };
    if host.is_empty() {
        return None;
    }
    Some((scheme.to_string(), host, port))
}

fn is_private_ipv4(host: &str) -> bool {
    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() != 4 {
        return false;
    }
    let parsed = octets
        .iter()
        .map(|x| x.parse::<u32>().ok().filter(|v| *v <= 255))
        .collect::<Option<Vec<u32>>>();
    let Some(o) = parsed else {
        return false;
    };
    o[0] == 10
        || o[0] == 127
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 172 && (16..=31).contains(&o[1]))
}

/// `isLocalPlexAddress` — QBZ only talks to a LAN Plex server.
pub fn is_local_address(url_input: &str) -> bool {
    let normalized = normalize_server_url(url_input);
    if normalized.is_empty() {
        return false;
    }
    let Some((_, host, _)) = parse_url(&normalized) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host == "::1"
        || host.ends_with(".local")
        || host.ends_with(".lan")
        || !host.contains('.')
        || is_private_ipv4(&host)
}

/// `resolvePlexBaseUrl`: normalize + default port 32400 -> `proto://host:port`.
pub fn resolve_base_url(server_url: &str) -> String {
    let normalized = normalize_server_url(server_url);
    if normalized.is_empty() {
        return String::new();
    }
    let Some((scheme, host, port)) = parse_url(&normalized) else {
        return String::new();
    };
    if scheme != "http" && scheme != "https" {
        return String::new();
    }
    format!(
        "{scheme}://{host}:{}",
        port.unwrap_or_else(|| "32400".into())
    )
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

/// Master toggle only. Browse and source access use [`is_configured`] so an
/// incomplete or disabled integration never opens the installation-wide
/// cache; this accessor remains the authority for the settings UI itself.
pub fn is_enabled() -> bool {
    settings().enabled
}

/// `canUsePlexRequests`: enabled && LAN && resolved base url && token. This
/// is `plex-available` — it gates the header Sync button and every request
/// that leaves the process.
pub fn is_configured() -> bool {
    let cfg = settings();
    cfg.enabled
        && is_local_address(&cfg.base_url)
        && !resolve_base_url(&cfg.base_url).is_empty()
        && !cfg.token.trim().is_empty()
}

/// `<data_dir>/qbz/plex_cache.db`, gated on a complete, enabled setup. `None`
/// means browse code does not even open the installation-wide cache; a stale
/// cache from another account must not make Plex part of this user's library.
pub fn cache_db_path() -> Option<PathBuf> {
    if !is_configured() {
        return None;
    }
    let path = dirs::data_dir()?.join("qbz").join("plex_cache.db");
    path.exists().then_some(path)
}

// ---------------------------------------------------------------------------
// Artwork
// ---------------------------------------------------------------------------

/// A Plex thumb is a SERVER-RELATIVE path, never a filesystem path.
pub fn is_thumb_path(path: &str) -> bool {
    path.starts_with("/library/") || path.starts_with("/photo/")
}

/// Tokenized, size-transcoded thumb URL for `path` ("" when Plex is not
/// usable). `qbz_models::plex_thumb_url` is the shared builder, so the Qt
/// and Slint frontends hit the SAME cache keys.
pub fn thumb_url(path: &str, size: Option<u32>) -> String {
    if !is_configured() {
        return String::new();
    }
    let cfg = settings();
    qbz_models::plex_thumb_url(&cfg.base_url, &cfg.token, path, size)
}

// ---------------------------------------------------------------------------
// Cache reads, mapped into the local-library shapes
// ---------------------------------------------------------------------------

/// Plex reports a codec/container word. The DSD arm is not decoration: the
/// identical omission in `LibraryDatabase::parse_format` is what made every
/// local DSD track read back as `Unknown`, print "UNKNOWN" as its format and
/// wear the CD badge. A Plex server serving DSD hits the same fold, and a fold
/// to `Unknown` is a VALID answer — which is why no test ever sees it.
fn parse_audio_format(s: &str) -> qbz_library::AudioFormat {
    use qbz_library::AudioFormat;
    match s.to_ascii_lowercase().as_str() {
        "flac" => AudioFormat::Flac,
        "alac" => AudioFormat::Alac,
        "wav" | "wave" => AudioFormat::Wav,
        "aiff" | "aif" => AudioFormat::Aiff,
        "ape" => AudioFormat::Ape,
        "mp3" => AudioFormat::Mp3,
        "dsd" | "dsf" | "dff" => AudioFormat::Dsd,
        _ => AudioFormat::Unknown,
    }
}

/// Map a Plex-cache row to `LocalTrack` (1:1 with the Slint's
/// `map_plex_cached_to_local_track`): `file_path` carries the `rating_key`
/// the playback resolve needs, `artwork_path` stays the RAW `/library/...`
/// thumb path (tokenized at fetch time), `source` is `"plex"`, and the id is
/// namespaced so it cannot collide with a local row.
pub fn map_cached_to_local_track(t: qbz_plex::PlexCachedTrack) -> LocalTrack {
    let native_album_id = t
        .parent_rating_key
        .clone()
        .filter(|key| !key.is_empty())
        .unwrap_or_else(|| t.album_key.clone());
    let source_instance = crate::remote_metadata_qt::active_source_instance("plex");
    let mut track = LocalTrack {
        id: (PLEX_TRACK_ID_FLOOR | (t.id & (PLEX_TRACK_ID_FLOOR - 1))) as i64,
        file_path: t.rating_key,
        title: t.title,
        artist: t.artist,
        album: t.album.clone(),
        // Per-EDITION key so two same-titled Plex albums don't interleave;
        // pre-resync rows without a parent key fall back to the album hash.
        album_group_key: t
            .parent_rating_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .map(|k| format!("plex:album:{k}"))
            .unwrap_or_else(|| t.album_key.clone()),
        album_group_title: t.album.clone(),
        track_number: t.track_number,
        disc_number: t.disc_number,
        year: t.year,
        duration_secs: t.duration_secs,
        format: parse_audio_format(&t.format),
        bit_depth: t.bit_depth,
        sample_rate: t.sample_rate as f64,
        artwork_path: t.artwork_path,
        collection_artwork_path: t.collection_artwork_path,
        source: Some("plex".to_string()),
        ..Default::default()
    };
    crate::remote_metadata_qt::apply(&mut track, "plex", &source_instance, &native_album_id);
    track
}

/// The full Plex track set matching `query`, in the `LocalTrack` shape.
/// Tracks uses [`search_tracks_page`] so its candidate set stays bounded.
pub fn search_tracks(query: &str) -> Vec<LocalTrack> {
    if !is_configured() {
        return Vec::new();
    }
    qbz_plex::plex_cache_search_tracks(query.trim().to_string(), None)
        .unwrap_or_default()
        .into_iter()
        .map(map_cached_to_local_track)
        .collect()
}

pub fn search_tracks_page(
    query: &str,
    offset: u64,
    limit: u64,
    sort: &str,
    formats: &[String],
    other_formats: bool,
    quality_tiers: &[String],
) -> Vec<LocalTrack> {
    if !is_configured() {
        return Vec::new();
    }
    qbz_plex::plex_cache_search_tracks_page_filtered(
        query.trim().to_string(),
        offset,
        limit,
        sort,
        formats,
        other_formats,
        quality_tiers,
    )
    .unwrap_or_default()
    .into_iter()
    .map(map_cached_to_local_track)
    .collect()
}

/// One Plex album's tracks, by the legacy content hash (`plex:<hash>`) or the
/// source-native edition key (`plex:album:<parentRatingKey>`).
pub fn album_tracks(album_key: &str) -> Vec<LocalTrack> {
    if !is_configured() {
        return Vec::new();
    }
    qbz_plex::plex_cache_get_album_tracks(album_key.to_string())
        .unwrap_or_default()
        .into_iter()
        .map(map_cached_to_local_track)
        .collect()
}

/// The content-hash album key for a played Plex track (its
/// `album_group_key` is the per-edition split key, which the album cache is
/// NOT keyed by).
pub fn album_key_for(artist: &str, album: &str) -> String {
    qbz_plex::plex_album_key(artist, album)
}

pub fn cached_artists() -> Vec<qbz_plex::PlexCachedArtist> {
    if !is_configured() {
        return Vec::new();
    }
    qbz_plex::plex_cache_get_artists().unwrap_or_default()
}

pub fn cached_track_count() -> i64 {
    if !is_configured() {
        return 0;
    }
    qbz_plex::plex_cache_count_tracks().unwrap_or(0) as i64
}

/// Cached library sections + which of them are selected (Settings panel).
pub fn cached_sections() -> (Vec<qbz_plex::PlexMusicSection>, Vec<String>) {
    if !is_configured() {
        return (Vec::new(), Vec::new());
    }
    (
        qbz_plex::plex_cache_get_sections().unwrap_or_default(),
        settings().selected_section_keys,
    )
}

// ---------------------------------------------------------------------------
// Mutations (the settings panel drives the SAME store)
// ---------------------------------------------------------------------------

pub fn set_enabled(value: bool) {
    invalidate_cache();
    ensure_bound();
    if let Err(e) = STATE.set_enabled(value) {
        log::error!("[qbz-qt] plex set_enabled failed: {e}");
    }
}

/// The stable per-install identifier plex.tv keys a PIN against. Generated
/// once and persisted by the shared store; `X-Plex-Client-Identifier` must be
/// the SAME value on the pin/start and pin/check calls or the check never
/// sees the authorization.
pub fn client_id() -> String {
    ensure_bound();
    STATE.get_or_create_client_id().unwrap_or_else(|e| {
        log::error!("[qbz-qt] plex client id failed: {e}");
        String::new()
    })
}

/// Manual-token mode: the user pasted an `X-Plex-Token` instead of signing in
/// through a PIN. The PIN path clears it on success so the panel goes back to
/// showing Authorize (`plex_auth.rs:531`).
pub fn set_manual_token_mode(value: bool) {
    invalidate_cache();
    ensure_bound();
    if let Err(e) = STATE.set_manual_token_mode(value) {
        log::error!("[qbz-qt] plex set_manual_token_mode failed: {e}");
    }
}

/// Record the server's `machineIdentifier` from a successful ping.
///
/// Its READER has been live since the port landed (`local_plex.rs`'s cache
/// rows carry `server_id`), but the only WRITER was the Slint build's
/// `ping_inner` — so a Qt-only install stamped `server_id = NULL` on every
/// cached row. Porting the ping is what closes that.
pub fn set_machine_id(value: &str) {
    invalidate_cache();
    ensure_bound();
    if let Err(e) = STATE.set_machine_id(value) {
        log::error!("[qbz-qt] plex set_machine_id failed: {e}");
    }
}

/// The persisted (resolved) base url + token, for callers that need to talk
/// to the server without going through a fresh connect.
pub fn credentials() -> (String, String) {
    let cfg = settings();
    (cfg.base_url, cfg.token)
}

/// Settings > Local Library > Plex ("write metadata back to Plex").
pub fn set_metadata_write_enabled(value: bool) {
    invalidate_cache();
    ensure_bound();
    if let Err(e) = STATE.set_metadata_write_enabled(value) {
        log::error!("[qbz-qt] plex set_metadata_write_enabled failed: {e}");
    }
}

/// Persist a manually entered server + token (`persistPlexConfig`): the URL
/// is RESOLVED (`proto://host:32400`) before it is stored. Returns the
/// resolved base url ("" when the input is unusable).
pub fn connect_manual(server_url: &str, token: &str) -> String {
    invalidate_cache();
    ensure_bound();
    let base = resolve_base_url(server_url);
    if base.is_empty() {
        return base;
    }
    if let Err(e) = STATE.set_credentials(&base, token.trim()) {
        log::error!("[qbz-qt] plex set_credentials failed: {e}");
        return String::new();
    }
    let _ = STATE.set_manual_token_mode(true);
    base
}

pub fn set_selected_sections(keys: &[String]) {
    invalidate_cache();
    ensure_bound();
    if let Err(e) = STATE.set_selected_section_keys(keys) {
        log::error!("[qbz-qt] plex set_selected_section_keys failed: {e}");
    }
}

/// Sign out of Plex: clear creds + sections + machine id (keeps `enabled`,
/// `client_id`, `metadata_write_enabled`) and purge the shared cache DB so
/// no Plex rows survive in the browse union.
pub fn disconnect() {
    cancel_sync();
    invalidate_cache();
    ensure_bound();
    if let Err(e) = STATE.disconnect() {
        log::error!("[qbz-qt] plex disconnect failed: {e}");
    }
    if let Err(e) = qbz_plex::plex_cache_clear() {
        log::warn!("[qbz-qt] plex cache clear failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// Manual sync (#573)
// ---------------------------------------------------------------------------

static SYNCING: AtomicBool = AtomicBool::new(false);
static SYNC_CANCEL: AtomicBool = AtomicBool::new(false);
static ALBUM_ARTWORK_REPAIRING: AtomicBool = AtomicBool::new(false);
const SYNC_PAGE_SIZE: u64 = 250;

pub fn is_syncing() -> bool {
    SYNCING.load(Ordering::SeqCst)
}

pub fn cancel_sync() {
    SYNC_CANCEL.store(true, Ordering::Release);
}

/// `sync_now` + `syncSelectedPlexLibraries`: fetch the music sections, save
/// them, default-select ALL when nothing is persisted, PRUNE the de-selected
/// sections, then fetch each selected section in bounded pages. Page rows and
/// their resume checkpoint commit together; an old generation is pruned only
/// after the section reaches its declared total. Returns the exact completed
/// track count, including rows written before a resumed checkpoint.
///
/// Re-entrancy: a second call while a sync is in flight returns `Err` instead
/// of racing the cache writes.
pub async fn sync_now() -> Result<usize, String> {
    if SYNCING.swap(true, Ordering::SeqCst) {
        return Err("Plex sync already running".to_string());
    }
    SYNC_CANCEL.store(false, Ordering::Release);
    let result = sync_inner().await;
    SYNCING.store(false, Ordering::SeqCst);
    if result.is_ok() {
        start_album_artwork_repair();
    }
    result
}

/// Hydrate album art only for parent ids whose Track rows omit both `thumb`
/// and `parentThumb`. Plex's own album metadata is the authority; in
/// particular, inherited artwork can point at another rating key, so a URL
/// synthesized from `parentRatingKey` is not safe.
fn start_album_artwork_repair() {
    if SYNCING.load(Ordering::Acquire) || ALBUM_ARTWORK_REPAIRING.swap(true, Ordering::AcqRel) {
        return;
    }
    let cfg = settings();
    if !cfg.enabled
        || !is_local_address(&cfg.base_url)
        || resolve_base_url(&cfg.base_url).is_empty()
        || cfg.token.trim().is_empty()
    {
        ALBUM_ARTWORK_REPAIRING.store(false, Ordering::Release);
        return;
    }
    let server_id = (!cfg.machine_id.trim().is_empty()).then_some(cfg.machine_id.clone());
    crate::spawn(async move {
        let candidate_server = server_id.clone();
        let candidates = tokio::task::spawn_blocking(move || {
            qbz_plex::plex_cache_album_artwork_candidates(candidate_server)
        })
        .await;
        let candidates = match candidates {
            Ok(Ok(candidates)) => candidates,
            Ok(Err(error)) => {
                log::warn!("[plex-art] candidate query failed: {error}");
                ALBUM_ARTWORK_REPAIRING.store(false, Ordering::Release);
                return;
            }
            Err(error) => {
                log::warn!("[plex-art] candidate worker failed: {error}");
                ALBUM_ARTWORK_REPAIRING.store(false, Ordering::Release);
                return;
            }
        };
        if candidates.is_empty() {
            ALBUM_ARTWORK_REPAIRING.store(false, Ordering::Release);
            return;
        }

        let mut resolved = Vec::with_capacity(candidates.len());
        let mut fetch_failures = 0usize;
        for parent_rating_key in candidates {
            match qbz_plex::plex_get_album_artwork_path(
                cfg.base_url.clone(),
                cfg.token.clone(),
                parent_rating_key.clone(),
            )
            .await
            {
                Ok(path) => resolved.push((parent_rating_key, path)),
                Err(error) => {
                    fetch_failures = fetch_failures.saturating_add(1);
                    log::debug!("[plex-art] album metadata skipped: {error}");
                }
            }
        }
        let covers = resolved.iter().filter(|(_, path)| path.is_some()).count();
        let save_server = server_id.clone();
        let saved = tokio::task::spawn_blocking(move || {
            qbz_plex::plex_cache_save_album_artwork(save_server, resolved)
        })
        .await;
        match saved {
            Ok(Ok(checked)) => {
                log::info!(
                    "[plex-art] metadata checked={checked} covers={covers} failures={fetch_failures}"
                );
                if covers > 0 {
                    crate::local_catalog_qt::request_catch_up();
                }
            }
            Ok(Err(error)) => log::warn!("[plex-art] cache update failed: {error}"),
            Err(error) => log::warn!("[plex-art] cache worker failed: {error}"),
        }
        ALBUM_ARTWORK_REPAIRING.store(false, Ordering::Release);
    });
}

async fn interrupt_section(section_key: String, generation: u64, restart: bool) {
    let _ = tokio::task::spawn_blocking(move || {
        if restart {
            qbz_plex::plex_cache_restart_section_sync(section_key, generation)
        } else {
            qbz_plex::plex_cache_interrupt_section_sync(section_key, generation)
        }
    })
    .await;
}

async fn sync_inner() -> Result<usize, String> {
    let cfg = settings();
    if !is_configured() {
        return Err("Plex is not configured".to_string());
    }
    let base = cfg.base_url.clone();
    let token = cfg.token.clone();

    let sections =
        qbz_plex::plex_get_music_sections(base.trim().to_string(), token.trim().to_string())
            .await
            .map_err(|e| e.to_string())?;

    let machine_id = settings().machine_id;
    let server_id = (!machine_id.is_empty()).then_some(machine_id);

    {
        let sections = sections.clone();
        let server_id = server_id.clone();
        tokio::task::spawn_blocking(move || {
            qbz_plex::plex_cache_save_sections(server_id, sections)
        })
        .await
        .map_err(|e| format!("Plex sections cache worker failed: {e}"))??;
    }

    // Default-select ALL when the persisted selection is empty / stale.
    let available: std::collections::HashSet<String> =
        sections.iter().map(|s| s.key.clone()).collect();
    let persisted: Vec<String> = settings()
        .selected_section_keys
        .into_iter()
        .filter(|k| available.contains(k))
        .collect();
    let selected: Vec<String> = if persisted.is_empty() {
        sections.iter().map(|s| s.key.clone()).collect()
    } else {
        persisted
    };
    set_selected_sections(&selected);

    // Selection changes are authoritative, but selected sections retain their
    // current and incomplete generations (including hydrated quality).
    {
        let keep = selected.clone();
        tokio::task::spawn_blocking(move || qbz_plex::plex_cache_prune_sections(&keep))
            .await
            .map_err(|e| format!("Plex section prune worker failed: {e}"))??;
    }

    let mut total = 0usize;
    for key in &selected {
        if SYNC_CANCEL.load(Ordering::Acquire) {
            return Err("Plex sync cancelled".to_string());
        }
        let begin_server = server_id.clone();
        let begin_key = key.clone();
        let mut state = tokio::task::spawn_blocking(move || {
            qbz_plex::plex_cache_begin_section_sync(begin_server, begin_key)
        })
        .await
        .map_err(|e| format!("Plex section start worker failed: {e}"))??;
        log::info!(
            "[plex-sync] section={} generation={} resumed={} checkpoint={}",
            key,
            state.generation,
            state.resumed,
            state.next_start,
        );

        loop {
            if SYNC_CANCEL.load(Ordering::Acquire) {
                interrupt_section(key.clone(), state.generation, false).await;
                return Err("Plex sync cancelled".to_string());
            }
            let page = match qbz_plex::plex_get_section_tracks_page(
                base.trim().to_string(),
                token.trim().to_string(),
                key.clone(),
                state.next_start,
                SYNC_PAGE_SIZE,
            )
            .await
            {
                Ok(page) => page,
                Err(error) => {
                    interrupt_section(key.clone(), state.generation, false).await;
                    return Err(error);
                }
            };
            if state
                .total_size
                .is_some_and(|total_size| total_size != page.total_size)
            {
                interrupt_section(key.clone(), state.generation, true).await;
                return Err("Plex section totalSize changed during sync".to_string());
            }
            let has_more = page.has_more();
            let page_rows = page.tracks.len();
            let apply_key = key.clone();
            let generation = state.generation;
            state = match tokio::task::spawn_blocking(move || {
                qbz_plex::plex_cache_apply_section_page(apply_key, generation, page)
            })
            .await
            {
                Ok(Ok(state)) => state,
                Ok(Err(error)) => {
                    interrupt_section(key.clone(), generation, true).await;
                    return Err(error);
                }
                Err(error) => {
                    interrupt_section(key.clone(), generation, false).await;
                    return Err(format!("Plex page cache worker failed: {error}"));
                }
            };
            log::info!(
                "[plex-sync] section={} generation={} page_rows={} observed={} total={} checkpoint={} complete={}",
                key,
                state.generation,
                page_rows,
                state.observed_rows,
                state.total_size.unwrap_or(0),
                state.next_start,
                !has_more,
            );
            if SYNC_CANCEL.load(Ordering::Acquire) {
                interrupt_section(key.clone(), state.generation, false).await;
                return Err("Plex sync cancelled".to_string());
            }
            if has_more {
                continue;
            }
            let finish_key = key.clone();
            let generation = state.generation;
            let pruned = tokio::task::spawn_blocking(move || {
                qbz_plex::plex_cache_finish_section_sync(finish_key, generation)
            })
            .await
            .map_err(|e| format!("Plex section finish worker failed: {e}"))??;
            total =
                total.saturating_add(usize::try_from(state.observed_rows).unwrap_or(usize::MAX));
            log::info!(
                "[plex-sync] section={} generation={} rows={} pruned={} prune_authorized=true",
                key,
                state.generation,
                state.observed_rows,
                pruned,
            );
            crate::local_catalog_qt::request_catch_up();
            break;
        }
    }
    log::info!(
        "[qbz-qt] plex sync: {total} tracks across {} sections",
        selected.len()
    );
    Ok(total)
}
