//! Wiring the `qbz-source` registry to this frontend's stores.
//!
//! # Why this file exists, and what it cost not to have it
//!
//! `qbz-source` deliberately does not link `qbz-app` / `qbz-core` / `qbz-player`
//! (design 02 §8), so it cannot open the Plex settings store, cannot reach the
//! ephemeral session store, and does not know which user's `library.db` to
//! read. All three are INJECTED by the frontend. Stage 1 built the sockets and
//! nothing ever plugged into them.
//!
//! That was invisible for two stages. `SourceRegistry::claim` is pure, and
//! `PlexSource::tracks` / `meta` read `plex_cache.db` through `qbz-plex`'s own
//! global — neither needs a credential or a bound pool. The first consumers
//! that DO need them are playback and artwork, i.e. stages 3 and 4, and the
//! moment those landed the seam started answering:
//!
//! ```text
//! ERROR local play: track 1099511674716 not playable: plex: not configured
//! ```
//!
//! for every Plex row, nothing at all for every local row (an unbound
//! [`DbPool`] refuses reads), and `ArtRef::Unavailable` for every Plex cover —
//! which is why the Local Library grid lost its Plex artwork. The unit tests
//! could not see any of it: they exercise pure ladders (`rating_key`,
//! `artwork_token`, `claim`), and there is no bound user in a test binary.
//!
//! **The lesson, recorded because it is the expensive one:** a structural gate
//! ("the old code is gone") and a green test suite prove that the OLD path was
//! removed, never that the NEW path is connected. A seam has to be exercised
//! against a bound user before anything is routed onto it.
//!
//! # What is injected
//!
//! | socket | source | this frontend's authority |
//! |---|---|---|
//! | [`PlexCreds`] | `PlexSource` | `local_plex::is_configured` / `settings` |
//! | [`EphemeralTracks`] | `LocalSource` | `local_ephemeral`'s session store |
//! | user dir | every source, via `bind_user` | `auth_qt::bind_per_user_stores` |
//! | album group mode | `LocalSource` | `local_state::group_mode` (a persisted UI pref) |
//!
//! Each one has exactly ONE authority, and it is the one that already existed:
//! opening a second copy of the Plex settings DB or a second ephemeral store
//! would be a second source of truth, which is the thing the seam exists to
//! remove.

use std::sync::Arc;

use qbz_library::LocalTrack;

/// `PlexSource`'s view of the credentials store this frontend already owns.
struct PlexCredsGlue;

impl qbz_source::PlexCreds for PlexCredsGlue {
    fn is_enabled(&self) -> bool {
        crate::local_plex::is_configured()
    }

    fn server(&self) -> Option<(String, String)> {
        if !crate::local_plex::is_configured() {
            return None;
        }
        let cfg = crate::local_plex::settings();
        Some((cfg.base_url, cfg.token))
    }
}

/// `JellyfinSource`'s view of the media-server settings store.
struct JellyfinGlue;

impl qbz_source::JellyfinCreds for JellyfinGlue {
    fn is_enabled(&self) -> bool {
        crate::media_servers_qt::jellyfin_server().is_some()
    }

    fn server(&self) -> Option<(String, String)> {
        crate::media_servers_qt::jellyfin_server()
    }
}

/// `SubsonicSource`'s view of the same store.
///
/// It hands over a derived `Credentials` rather than a bearer token because
/// Subsonic has no session: every request carries `md5(password + salt)`, so
/// the token is computed here, from the stored password and the install's
/// fixed salt.
struct SubsonicGlue;

impl qbz_source::SubsonicCreds for SubsonicGlue {
    fn is_enabled(&self) -> bool {
        crate::media_servers_qt::subsonic_credentials().is_some()
    }

    fn server(&self) -> Option<(String, qbz_subsonic::Credentials)> {
        crate::media_servers_qt::subsonic_credentials()
    }
}

/// `LocalSource`'s view of the session-scoped ephemeral store.
struct EphemeralGlue;

impl qbz_source::EphemeralTracks for EphemeralGlue {
    fn get_track(&self, id: i64) -> Option<LocalTrack> {
        crate::local_ephemeral::get_track(id)
    }

    fn album_tracks(&self, group_key: &str) -> Vec<LocalTrack> {
        crate::local_ephemeral::album_tracks_for(group_key)
    }
}

/// Publish the process-lifetime injections: the Plex credentials lens, the
/// ephemeral store, and the unbound-window fallback.
///
/// Called from `on_boot`, right after `init_registry`. None of these is
/// per-user — the creds glue READS the active store on every call rather than
/// caching it, so a Plex connect/disconnect mid-session is picked up with no
/// re-publish (the same clone-then-drop discipline the Qobuz client lens uses,
/// and for the same reason: a cached copy goes stale silently).
pub(crate) fn install() {
    let registry = qbz_source::registry();
    registry.plex().set_creds(Some(Arc::new(PlexCredsGlue)));
    registry.jellyfin().set_creds(Some(Arc::new(JellyfinGlue)));
    registry.subsonic().set_creds(Some(Arc::new(SubsonicGlue)));
    registry
        .local()
        .set_ephemeral(Some(Arc::new(EphemeralGlue)));
    // Reads BEFORE a user is bound (splash, session restore) must resolve the
    // way `local_state::with_db` already resolves them — through
    // `load_last_user_id` — or the window between boot and
    // `bind_per_user_stores` answers "no library" instead of the last user's
    // rows. This mirrors today's behaviour rather than changing it.
    registry
        .local()
        .set_unbound_fallback(Some(Box::new(crate::local_state::db_path)));
    sync_album_group_mode();
    log::info!("[qbz-qt] source registry: creds + ephemeral store published");
}

/// Push the persisted album-identity pref into `LocalSource`.
///
/// The grouping IS the query: in metadata mode an album's tracks come from
/// `get_album_tracks_metadata`, in folder mode from `get_album_tracks`. The
/// source defaults to Folder, so without this a metadata-mode user's album
/// play resolves a DIFFERENT track list than the grid showed them.
///
/// Called at boot, on every user bind, and from `local_state::set_album_mode`
/// when the user flips the toolbar.
pub(crate) fn sync_album_group_mode() {
    qbz_source::registry()
        .local()
        .set_album_group_mode(crate::local_state::group_mode());
}

/// Bind every source to the active user's data directory.
///
/// MUST be the FIRST statement of `auth_qt::bind_per_user_stores`:
/// `myqbz_qt::init_for_user` runs the mixtape migrations through
/// `library_db_qt::with_db(true, …)`, and against an unbound pool a fresh
/// account can never create a collection.
pub(crate) fn bind_user(dir: &std::path::Path, user_id: u64) {
    qbz_source::registry().bind_user(user_id, dir);
    sync_album_group_mode();
    report_wiring();
}

/// ASK each socket a question only a wired one can answer, and log the answers.
///
/// This is not decoration. The failure it exists to catch is silent by
/// construction: an unbound [`DbPool`] returns `None` from every read and an
/// uncredentialed `PlexSource` returns `ArtRef::Unavailable`, so an unwired
/// registry looks exactly like "this user has no local tracks and Plex is
/// switched off" — right up until the first playback attempt, several screens
/// later, blames a service that has nothing to do with the track. One line at
/// bind time turns that into something greppable at the moment it happens.
///
/// Both probes are cheap: one indexed COUNT, and one pure string
/// interpretation with no I/O at all.
fn report_wiring() {
    use qbz_source::{ArtRef, ArtSize, Source};

    let registry = qbz_source::registry();
    let tracks = registry
        .local()
        .with(|db| db.count_all_local_tracks())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "UNBOUND".to_string());
    // The exact call the Local Library grid makes for a Plex row. `Fetch`
    // means credentials resolved; `Unavailable` is the "no Plex credentials
    // configured" state that cost a release build.
    let plex = match registry
        .plex()
        .artwork_token("/library/metadata/0/thumb/0", ArtSize::Card)
    {
        ArtRef::Fetch { .. } => "connected",
        ArtRef::Unavailable(why) => why,
        _ => "not recognised",
    };
    let ephemeral = if registry.local().has_ephemeral_store() {
        "yes"
    } else {
        "NO"
    };
    // The remote pair reports what the union will actually see, which is the
    // question that matters: a server can be enabled and still contribute
    // nothing because it was never swept.
    let remote = {
        let words = crate::media_servers_qt::configured_words();
        if words.is_empty() {
            "none configured".to_string()
        } else {
            words.join("+")
        }
    };
    log::info!(
        "[qbz-qt] source registry bound: local_tracks={tracks} plex={plex} \
         ephemeral_store={ephemeral} remote={remote}"
    );
}

/// Drop every per-user handle and cache. Called from the logout path.
///
/// Overriding `teardown` is mandatory for any source holding per-user state —
/// the default is a no-op, so a source that forgets it leaks the previous
/// account's handle into a `'static` registry.
pub(crate) fn teardown() {
    qbz_source::registry().teardown();
    // The creds lens and the ephemeral glue are process-lifetime and stateless
    // (they read their authority on every call), so they are re-published
    // rather than left cleared — `PlexSource::teardown` drops the handle.
    let registry = qbz_source::registry();
    registry.plex().set_creds(Some(Arc::new(PlexCredsGlue)));
    registry.jellyfin().set_creds(Some(Arc::new(JellyfinGlue)));
    registry.subsonic().set_creds(Some(Arc::new(SubsonicGlue)));
    registry
        .local()
        .set_ephemeral(Some(Arc::new(EphemeralGlue)));
    registry
        .local()
        .set_unbound_fallback(Some(Box::new(crate::local_state::db_path)));
}
