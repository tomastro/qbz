//! System media controls — MPRIS on Linux (what makes the KDE Plasma
//! now-playing widget, GNOME Shell's media section and the hardware media keys
//! see QBZ at all), SMTC / MediaRemote on Windows / macOS.
//!
//! The backend is the frontend-agnostic `qbz-media-controls` crate (ADR-006),
//! consumed VERBATIM — this module is only the Qt-side glue: it owns the
//! process-global handle, the metadata de-dupe, the artwork resolver MPRIS
//! needs, and it bridges inbound control events onto the transport verbs.
//! Playback metadata/state is PUSHED from `playback_qt.rs`, mirroring the tray
//! (`src/tray_qt.rs`).
//!
//! Ported from `crates/qbz/src/media_controls.rs`; where the Slint glue and
//! the daemon's (`crates/qbzd/src/mpris.rs`) disagree, the DAEMON is the
//! precedent, because qbz-qt is likewise not a Slint app:
//!
//!  - **No captured runtime.** The Slint closure captures a strong
//!    `Arc<AppRuntime>` plus a `slint::Weak<AppWindow>` and a tokio `Handle`
//!    (`media_controls.rs:29-35`); the daemon deliberately holds only a
//!    `Weak` so the integration can never pin the runtime (#521 audio-release
//!    ordering, `qbzd/src/mpris.rs:15-18`). Qt needs NEITHER: `crate::app()`
//!    (`src/main.rs:312`) and `crate::spawn()` (`:317`) are process-global
//!    `OnceLock`s, so the callback captures nothing at all and the inbound
//!    `fn` is a plain function item. Same property `tray_qt.rs:224-228` cites
//!    for the ksni thread.
//!  - **Sync core commands run on the D-Bus thread.** The callback runs on the
//!    mpris-server thread — NOT a tokio worker, NOT the Qt thread
//!    (`qbzd/src/mpris.rs:148-151`). `core().stop()` and
//!    `core().get_playback_state()` are synchronous, so they are called
//!    directly like the daemon does, and everything async is handed to the
//!    `crate::transport_*` verbs, which do their own `app()` + `spawn()`.
//!
//! ## Threading — do NOT "simplify" any of this
//!
//! OUTBOUND (`set_metadata` / `set_playback`) is free from any thread: the
//! Linux handle's methods are non-blocking sends onto the server's channel.
//! INBOUND must never block: the D-Bus thread services every method call, so
//! nothing here waits on the network — the transport verbs return immediately
//! after spawning.
//!
//! This process ALREADY publishes on D-Bus (the ksni StatusNotifierItem on its
//! own thread, plus whatever libQt6DBus opens). MPRIS adds a THIRD connection
//! on its own thread. They coexist only because zbus 4 (mpris-server) and
//! zbus 5 (ksni / ashpd / rfd) unify features per-major: **never enable a
//! `tokio` feature on any D-Bus dependency** or the async-io threads panic
//! with "there is no reactor running" (see the ksni comment in Cargo.toml).
//!
//! ## No teardown
//!
//! Like the tray (`tray_qt.rs:230-234`) and the reference (`grep
//! 'media_controls' crates/qbz/src/auth.rs` → 0 hits) the integration is
//! process-scoped: `init` is idempotent, logout does not touch it, and there
//! is no `shutdown`. Do not add one.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use qbz_media_controls::{MediaEvent, MediaIntegration};

/// The live integration, or `None` when it never started (no session bus, an
/// unsupported platform, or the name was already owned). Boxed behind a
/// `OnceLock` exactly like `tray_qt::TRAY` (`src/tray_qt.rs:159`).
static CONTROLS: OnceLock<Box<dyn MediaIntegration>> = OnceLock::new();

/// Last `(track id, resolved art URL)` pushed as metadata.
/// `refresh_now_playing` re-runs on resume / seek / republish — it has 23 call
/// sites across nine modules (`src/playback_qt.rs:1777-1779`) — so metadata is
/// only re-pushed when this key actually changes. 1:1 with the reference's
/// `MPRIS_LAST_META` (`crates/qbz/src/playback.rs:1723-1724`), including the
/// art field: the URL a widget can load changes when the cover resolves
/// differently (offline), which the track id alone cannot see.
/// `None` = nothing pushed yet / cleared.
static MPRIS_LAST_META: Mutex<Option<(u64, Option<String>)>> = Mutex::new(None);

/// Track id of the last DESKTOP NOTIFICATION raised, so only a real track
/// change can raise one. Separate from `MPRIS_LAST_META` on purpose: that key
/// includes the resolved art, so a cover that resolves differently re-pushes
/// metadata — correct for a widget, wrong for a toast. 1:1 with the
/// reference's `NOTIFY_LAST_TRACK` (`crates/qbz/src/playback.rs:1708`),
/// including the `u64::MAX` poison on stop so replaying the SAME track after a
/// stop notifies again.
static NOTIFY_LAST_TRACK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The live integration, if it started. Playback pushes metadata/state through
/// it. Mirrors `tray_qt::handle()` (`src/tray_qt.rs:165`).
pub(crate) fn handle() -> Option<&'static dyn MediaIntegration> {
    CONTROLS.get().map(|b| b.as_ref())
}

/// Start the OS media-controls integration once. No-op if already started or
/// unavailable on this platform.
///
/// NOT platform-gated, because the shared crate is not: `qbz_media_controls::
/// spawn` routes to the MPRIS backend on Linux, souvlaki on macOS/Windows and
/// returns `None` everywhere else (`crates/qbz-media-controls/src/lib.rs:36-53`),
/// so both consumers declare the dependency plain (`crates/qbz/Cargo.toml:80`,
/// `crates/qbzd/Cargo.toml:22`). Linux is the platform this port CLAIMS: under
/// Qt the souvlaki callbacks would have to be serviced by a Qt-owned run loop,
/// which nothing in this tree establishes.
///
/// A non-`None` handle does NOT mean the bus name was acquired — registration
/// happens asynchronously on the server thread and only logs on failure. The
/// smoke check is the `[mpris] registered org.mpris.MediaPlayer2.com.blitzfc.qbz`
/// line, not this one.
pub(crate) fn init() {
    if CONTROLS.get().is_some() {
        return;
    }
    // NON-WINDOWS: unchanged, deliberately. An earlier draft routed every
    // platform through the GUI hop and that was a Linux/macOS regression on
    // three counts: `tray_bridge::ui` is a NO-OP until `QbzTray::boot`
    // registers the Qt thread (`tray_bridge.rs:172`) and `QbzSession.boot()`
    // runs first (`Main.qml:254` vs `:380`), so initialization could be
    // dropped outright; MPRIS would start only when the Qt queue drained,
    // losing the first metadata push; and `main_window_hwnd()` calls
    // `QWindow::winId()`, which CREATES the platform window as a side effect
    // on platforms that had no reason to.
    #[cfg(not(target_os = "windows"))]
    {
        match qbz_media_controls::spawn(dispatch, qbz_media_controls::NativeWindow::default()) {
            Some(c) => {
                let _ = CONTROLS.set(c);
                log::info!("[media-controls] integration started");
            }
            None => log::info!("[media-controls] no integration on this platform"),
        }
    }

    #[cfg(target_os = "windows")]
    init_windows();
}

/// Windows needs the main window's HWND before SMTC can register, and it needs
/// to read it on the GUI thread. Both of the things it waits for -- the tray
/// bridge's Qt thread and a shown top-level window -- are "not yet", never
/// "not ever", so this retries on a bounded schedule instead of giving up on
/// the first miss the way a single hop would.
#[cfg(target_os = "windows")]
fn init_windows() {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Gate on QUEUED, not on CONTROLS: `init` can be called more than once,
    // and CONTROLS is only set at the END of a successful spawn, so gating on
    // it alone would let several initializers race and build duplicate
    // integrations before one wins the OnceLock.
    static QUEUED: AtomicBool = AtomicBool::new(false);
    if QUEUED.swap(true, Ordering::SeqCst) {
        return;
    }

    // A plain thread, because it is allowed to sleep: this must not occupy a
    // tokio worker, and it must not be the GUI thread it is posting to.
    let spawned = std::thread::Builder::new()
        .name("qbz-smtc-init".to_string())
        .spawn(|| {
            // 40 x 250 ms = 10 s, comfortably longer than a cold window show.
            for _ in 0..40 {
                if CONTROLS.get().is_some() {
                    return;
                }
                crate::tray_bridge::ui(|_b| {
                    if CONTROLS.get().is_some() {
                        return;
                    }
                    let native = qbz_media_controls::NativeWindow {
                        hwnd: crate::win_shell::main_window_hwnd().map(|p| p.as_ptr()),
                    };
                    if native.hwnd.is_none() {
                        // No top-level window yet. Say nothing and let the
                        // next tick try -- logging here would print forty
                        // times for a condition that resolves itself.
                        return;
                    }
                    match qbz_media_controls::spawn(dispatch, native) {
                        Some(c) => {
                            let _ = CONTROLS.set(c);
                            log::info!("[media-controls] integration started (SMTC)");
                        }
                        None => log::info!("[media-controls] no integration on this platform"),
                    }
                });
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            if CONTROLS.get().is_none() {
                log::warn!(
                    "[media-controls] SMTC never started: no top-level window after 10 s"
                );
            }
        });
    if let Err(e) = spawned {
        log::warn!("[media-controls] could not spawn the SMTC init thread: {e}");
    }
}

// ---------------------------------------------------------------------------
// Outbound — pushed from playback_qt (siblings of the tray pushes)
// ---------------------------------------------------------------------------

/// The has-track arm: metadata (de-duped) plus the OPTIMISTIC `Playing` at
/// position 0. 1:1 with `crates/qbz/src/playback.rs:2208-2223`, where
/// `set_playback` is deliberately unconditional — a stream is about to open,
/// and the poll loop's play/pause edge corrects it if it does not.
///
/// `title` is the VERSIONED title and `album` the release-variant
/// `album_display`, NOT the clean `track.album` the tray tooltip uses
/// (`playback.rs:2213` vs `:2164`) — the two are not interchangeable.
///
/// Pushing `set_playback` on every call is also what keeps the sleep inhibitor
/// alive: the Linux backend hangs #522's inhibit on the playback update, and
/// the inhibitor type is private, so there is no other way to reach it.
pub(crate) fn push_now_playing(track: &qbz_models::QueueTrack, title: &str, album: &str) {
    // The notification is NOT downstream of the MPRIS handle. A machine with no
    // session-bus MPRIS (or a platform the shared crate has no backend for)
    // still has a notification daemon, and the reference likewise raises the
    // toast outside its `if let Some(mc) = handle()` block
    // (`crates/qbz/src/playback.rs:2202` vs `:2228`). Hanging it off the early
    // return here would have made the switch dead all over again, just for a
    // smaller set of users.
    maybe_notify(track, title, album);
    push_metadata(track, title, album);
    // BEFORE the early return, and for the same reason `maybe_notify` is:
    // keeping the machine awake has nothing to do with whether the OS media
    // integration started, and a track is about to play either way.
    qbz_media_controls::set_sleep_inhibit(true);
    let Some(mc) = handle() else {
        return;
    };
    mc.set_playback(
        qbz_media_controls::PlaybackStatus::Playing,
        Some(Duration::ZERO),
    );
}

/// Republish only the metadata after an asynchronous cover download lands.
///
/// The normal track-edge publisher also raises a notification and resets the
/// optimistic MPRIS position to zero. Neither belongs to an artwork completion:
/// this narrower seam lets the de-duplication key observe the newly resolved
/// local cover without altering playback state or notifying twice.
pub(crate) fn refresh_now_playing_artwork(
    track: &qbz_models::QueueTrack,
    title: &str,
    album: &str,
) {
    push_metadata(track, title, album);
}

fn push_metadata(track: &qbz_models::QueueTrack, title: &str, album: &str) {
    let Some(mc) = handle() else {
        return;
    };
    let art = art_url_for(track);
    if meta_changed(&(track.id, art.clone())) {
        // `qbz_media_controls::TrackMeta` — NOT `crate::now_playing::TrackMeta`
        // (`src/now_playing.rs:261`), a different 13-field struct. Fully
        // qualified at every use in this module for that reason.
        mc.set_metadata(&qbz_media_controls::TrackMeta {
            title: title.to_string(),
            artist: track.artist.clone(),
            album: album.to_string(),
            duration: (track.duration_secs > 0).then(|| Duration::from_secs(track.duration_secs)),
            art_url: art,
            // `xesam:url` (#658): the public Qobuz link for catalog rows, None
            // for local/plex (`qbz-media-controls/src/types.rs:30-36`).
            url: qbz_media_controls::xesam_url_for(track.source.as_deref(), track.id),
        });
        // One line per real track edge. It exists because "Plasma sees
        // nothing" has two very different causes — the player never
        // registered, or it registered and no metadata was ever pushed — and
        // without this the log cannot tell them apart. A desktop applet
        // generally hides a player that is Stopped with empty metadata, so
        // the FIRST push is the interesting event, not the hundredth.
        log::info!(
            "[mpris] metadata pushed: {} — {} ({}s)",
            title,
            track.artist,
            track.duration_secs
        );
    }
}

/// The no-track arm: reset the de-dupe (so replaying the SAME track after a
/// stop re-pushes metadata) and tell the widget we stopped.
/// `crates/qbz/src/playback.rs:1913-1922`, in that order.
pub(crate) fn clear_now_playing() {
    if let Ok(mut last) = MPRIS_LAST_META.lock() {
        *last = None;
    }
    // Poison the notify key, so playing the SAME track again after a stop
    // raises a toast instead of being swallowed as "no change"
    // (`crates/qbz/src/playback.rs:1911`, same `u64::MAX` sentinel — a real
    // track id can never collide with it).
    NOTIFY_LAST_TRACK.store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
    crate::spawn(qbz_media_controls::withdraw_track_notification());
    if let Some(mc) = handle() {
        mc.set_playback(qbz_media_controls::PlaybackStatus::Stopped, None);
    }
}

/// The play/pause TRANSITION arm, carrying the live position in seconds.
/// `crates/qbz/src/playback.rs:5723-5730`.
///
/// Edge-triggered on purpose: `set_playback` emits a D-Bus `PropertiesChanged`,
/// so a per-tick push would spam every listening widget once a second. Between
/// transitions the MPRIS `Position` getter answers from the last value pushed
/// here — the same behaviour as the Slint build.
pub(crate) fn push_playback_state(is_playing: bool, position_secs: u64) {
    qbz_media_controls::set_sleep_inhibit(is_playing);
    let Some(mc) = handle() else {
        return;
    };
    let status = if is_playing {
        qbz_media_controls::PlaybackStatus::Playing
    } else {
        qbz_media_controls::PlaybackStatus::Paused
    };
    mc.set_playback(status, Some(Duration::from_secs(position_secs)));
}

/// Keep the reported position current, once per playback tick.
///
/// MPRIS `Position` is READ ON DEMAND — the spec keeps it out of
/// `PropertiesChanged` — so the backend serves whatever it last stored, and
/// before this it only stored one at a track start and at a play/pause edge.
/// A desktop widget therefore showed a value that could be minutes stale:
/// measured 2026-08-04 with QBZ at 4:03 and Plasma's widget stuck at 0:14.
///
/// `set_position` stores and emits nothing, so this is a channel send per
/// second and never a bus signal — it does not become the per-tick work
/// §7-M12 refuses.
pub(crate) fn push_position(position_secs: u64) {
    if let Some(mc) = handle() {
        mc.set_position(Duration::from_secs(position_secs));
    }
}

/// Report a user SEEK — a discontinuous jump, not the ordinary advance.
///
/// `push_position` fixes what a client sees when it OPENS; this is what
/// reaches one that is ALREADY open. The owner's observation is what made the
/// distinction concrete: *"el contador del seekbar del widget solo empieza a
/// contar hasta que lo abro"* — the widget extrapolates from the value it read
/// at open and never looks again, so only the `Seeked` signal can tell it the
/// ground moved.
pub(crate) fn push_seeked(position_secs: u64) {
    if let Some(mc) = handle() {
        mc.seeked(Duration::from_secs(position_secs));
    }
}

/// Compare-and-record the metadata de-dupe key. `true` when `key` differs from
/// the last pushed value (→ caller pushes now), recording it as the new
/// last-pushed value. A poisoned lock falls back to pushing.
/// `crates/qbz/src/playback.rs:1881-1893`.
fn meta_changed(key: &(u64, Option<String>)) -> bool {
    match MPRIS_LAST_META.lock() {
        Ok(mut last) => {
            if last.as_ref() == Some(key) {
                false
            } else {
                *last = Some(key.clone());
                true
            }
        }
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------
// The art resolver — `mpris:artUrl`
// ---------------------------------------------------------------------------

/// The cover URL for `mpris:artUrl`, or `None` when there is nothing a widget
/// could load (the property is then simply omitted — no placeholder).
///
/// This is NOT `artwork_qt::cached_path`. That returns Qt's `file_url`
/// (`src/artwork_qt.rs:184-199`), which escapes only `%`, `#` and `?` and
/// leaves spaces raw — good enough for `QUrl`, not a valid URI for a media
/// widget. The URI comes from `qbz_models::ArtworkRef::to_mpris_url`
/// (`crates/qbz-models/src/source.rs:161-181`), whose LocalFile arm goes
/// through `url::Url::from_file_path` (full percent-encoding), so both
/// frontends hand the desktop identical URLs.
///
/// Source-aware exactly like the reference (`crates/qbz/src/playback.rs:2000`):
/// the ref is built with the CURRENT Plex credentials at `size: None` — MPRIS
/// art wants the large image — so a Plex track yields a fetchable tokenized
/// URL instead of a `file://`-poisoned miss. Local / offline-download covers
/// are already on disk and pass through as a `file://` URI.
///
/// The offline branch is the B11 rule (`playback.rs:2168-2198`): ONLINE a
/// remote Qobuz cover keeps its `https://` URL untouched — widgets fetch it
/// themselves, and the disk cache's extension-less `<md5>.img` copy is
/// REJECTED by the freedesktop mime database (`application/vnd.efi.img`),
/// which is what made every push carry a dead URL. OFFLINE nothing can fetch
/// https, so a cache hit becomes the `file://` copy (better than nothing for
/// widgets that sniff content) and a miss becomes no art at all. The cache is
/// therefore only consulted while offline — one fewer SQLite touch per track
/// on the normal path, with an identical result.
/// The cover reference both consumers start from — MPRIS art and the desktop
/// notification. Extracted so the two can never drift on the Plex credentials
/// or on the 600-tier rewrite; they diverge only on what they do with a
/// REMOTE cover, which is the whole point of the split (see `notify_art_for`).
fn artwork_ref_for(track: &qbz_models::QueueTrack) -> qbz_models::ArtworkRef {
    // A SOURCE-TAGGED media token first, and it has to be first because
    // `qbz-models` cannot resolve it. That crate is the base of the graph — it
    // has no way to reach `qbz-source`, so `artwork_ref_with_plex` knows about
    // exactly one server, and anything else falls to its `LocalFile` arm: a
    // `jellyfin:<albumId>/<tag>` became a `file://` URI pointing at a path that
    // does not exist, which is why the bar and the queue showed the cover and
    // MPRIS did not.
    //
    // The frontend CAN resolve it, and by this point the cover is already in
    // the shared disk cache (the now-playing feed fetched it), so this is a
    // memo read. `LocalFile` rather than `Remote` on purpose: an MPRIS widget
    // fetches a remote URL itself, and this one carries credentials.
    if let Some(raw) = track.artwork_url.as_deref() {
        if raw
            .split_once(':')
            .and_then(|(w, _)| qbz_source::SourceId::from_word(w))
            .is_some_and(|id| {
                matches!(
                    id,
                    qbz_source::SourceId::JELLYFIN | qbz_source::SourceId::SUBSONIC
                )
            })
        {
            if let Some(path) = crate::artwork_qt::cached_raw_path(raw) {
                return qbz_models::ArtworkRef::LocalFile(path.to_string_lossy().into_owned());
            }
            // Not on disk yet: say NOTHING rather than hand the widget a
            // `file://` to a token. An empty ref is a missing cover; a broken
            // one is a broken cover.
            return qbz_models::ArtworkRef::LocalFile(String::new());
        }
    }
    let plex = crate::local_plex::settings();
    let artwork = track.artwork_ref_with_plex(&plex.base_url, &plex.token, None);
    // A restored queue can carry the 50px `small` Qobuz variant — the widget
    // fetches this url itself, so a tiny suffix IS the pixelation the owner
    // reported on the Plasma/GNOME art (2026-08-15). Rewrite to the 600 tier
    // (and the offline cache lookup below then hits the very file the
    // now-playing feed already downloaded at that tier).
    match artwork {
        qbz_models::ArtworkRef::Remote(u) => {
            qbz_models::ArtworkRef::Remote(qbz_models::qobuz_cover_at_px(&u, 600).unwrap_or(u))
        }
        other => other,
    }
}

pub(crate) fn art_url_for(track: &qbz_models::QueueTrack) -> Option<String> {
    let artwork = artwork_ref_for(track);
    let mut art = artwork.to_mpris_url();
    if crate::offline_fwd::engine().is_offline() {
        if let qbz_models::ArtworkRef::Remote(url) = &artwork {
            art = crate::artwork_qt::cached_raw_path(url).and_then(|path| {
                qbz_models::ArtworkRef::LocalFile(path.to_string_lossy().into_owned())
                    .to_mpris_url()
            });
        }
    }
    art
}

// ---------------------------------------------------------------------------
// The desktop "now playing" notification
// ---------------------------------------------------------------------------

/// Cover for the NOTIFICATION, which is not the same answer as `art_url_for`.
///
/// The notify pipeline strips `file://` and decodes the bytes BY CONTENT, so
/// the disk cache's extension-less `<md5>.img` copy is perfectly usable there
/// and saves a re-download — while MPRIS must NOT be handed that copy, because
/// widgets resolve `file://` through the freedesktop mime database, which maps
/// `*.img` by EXTENSION to `application/vnd.efi.img` and shows nothing (the B11
/// regression). So: a remote cover prefers the cached file here, and keeps the
/// remote URL when there is no cache hit — even offline, where the notifier's
/// own md5 cache may still serve it and the `offline` flag blocks the download.
/// 1:1 with `crates/qbz/src/playback.rs:2182-2198`.
fn notify_art_for(track: &qbz_models::QueueTrack) -> Option<String> {
    let artwork = artwork_ref_for(track);
    if let qbz_models::ArtworkRef::Remote(url) = &artwork {
        if let Some(path) = crate::artwork_qt::cached_raw_path(url) {
            return qbz_models::ArtworkRef::LocalFile(path.to_string_lossy().into_owned())
                .to_mpris_url();
        }
    }
    artwork.to_mpris_url()
}

/// Raise the desktop "now playing" toast for a real track change.
///
/// THE DEAD ROW THIS CLOSES: `system_notifications` was written by Settings and
/// read by nobody in the Qt port — the shared
/// `qbz_media_controls::show_track_notification` had exactly ONE caller in the
/// whole workspace and it was Slint's (`playback.rs:2276`). The switch shipped
/// on by default and did nothing.
///
/// Ordering here is load-bearing and matches the reference:
///   1. **De-dupe first.** `push_now_playing` runs from `refresh_now_playing`,
///      which has 23 call sites — resume, seek, quality patch, every queue
///      republish. Only a changed track id may pass.
///   2. **Then the gate.** Reading the pref costs a small JSON file read
///      (`settings_qt::pref_bool`), which is why it sits AFTER the atomic and
///      not before: once per track change, not once per republish. Deliberately
///      no cached atomic mirror of the pref — the reference needs one because
///      its notify path runs on a poll thread with no cheap access to prefs;
///      here a per-track read is both cheaper to reason about and immune to the
///      boot-order and cross-process staleness a mirrored gate invites (the
///      Slint build writes the same file).
///   3. **Then the peer guard**, inside the spawn: never announce a track that
///      a remote QConnect renderer is playing (the Svelte `skipIfRemote`).
fn maybe_notify(track: &qbz_models::QueueTrack, title: &str, album: &str) {
    use std::sync::atomic::Ordering;
    if NOTIFY_LAST_TRACK.swap(track.id, Ordering::Relaxed) == track.id {
        return;
    }
    if !crate::settings_qt::pref_bool("system_notifications", true) {
        return;
    }
    let meta = qbz_media_controls::NotificationMeta {
        title: title.to_string(),
        artist: track.artist.clone(),
        album: album.to_string(),
        bit_depth: track.bit_depth,
        sample_rate: track.sample_rate,
        art_url: notify_art_for(track),
    };
    let offline = crate::offline_fwd::engine().is_offline();
    crate::spawn(async move {
        if let Some(svc) = crate::qconnect_qt::service() {
            if svc.is_peer_active().await {
                return;
            }
        }
        qbz_media_controls::show_track_notification(meta, offline).await;
    });
}

// ---------------------------------------------------------------------------
// Inbound — media keys, the Plasma/GNOME widget, playerctl
// ---------------------------------------------------------------------------

/// Map one inbound `MediaEvent` onto a transport verb. Runs on the mpris-server
/// (D-Bus) thread — not a tokio worker, not the Qt thread — so nothing here
/// blocks: the sync core reads are cheap and every async command goes through
/// `crate::transport_*`, which spawns onto the process-global runtime.
/// Time values are MICROSECONDS (`qbz-media-controls/src/types.rs:47`).
fn dispatch(ev: MediaEvent) {
    match ev {
        // The OS only sends Play when paused and Pause when playing (it reads
        // PlaybackStatus), so routing all three through the toggle is correct
        // and keeps the cast → QConnect → local ladder the tray verbs ride
        // (`src/tray_qt.rs:487-500`).
        MediaEvent::Play | MediaEvent::Pause | MediaEvent::Toggle => {
            crate::tray_qt::dispatch_play_pause()
        }
        MediaEvent::Next => crate::tray_qt::dispatch_next(),
        MediaEvent::Previous => crate::tray_qt::dispatch_previous(),
        // The one command with no transport verb — direct on the core, like the
        // daemon (`qbzd/src/mpris.rs:90-92`) and like the two existing Qt call
        // sites (`src/cast_qt.rs:309`, `src/qconnect_engine_qt.rs:92`).
        // `QbzCore::stop` is synchronous (`crates/qbz-core/src/core.rs:1060`).
        MediaEvent::Stop => {
            let Some(_transport_action) = crate::playback_qt::begin_transport_action() else {
                return;
            };
            // Bound to a local first: `AppRuntime::core()` returns a BORROW
            // (`crates/qbz-app/src/shell.rs:144`) and `app()` hands back an
            // owned Arc, so the runtime must outlive the call — the same shape
            // `tray_qt::dispatch_volume_delta` uses (`src/tray_qt.rs:525`).
            let runtime = crate::app();
            if let Err(e) = runtime.core().stop() {
                log::warn!("[media-controls] stop failed: {e}");
            }
        }
        // Present, not show_window: with the miniplayer open a forced main-
        // window show reads as a duplicate instance (#559). `present()` was
        // left in exactly this shape for this caller and needs no event-loop
        // hop — it reads an AtomicBool mirror (`src/tray_qt.rs:442-462`).
        MediaEvent::Raise => crate::tray_qt::present(),
        MediaEvent::Quit => crate::tray_qt::quit(),
        // Local model first, then the engine — `transport_set_volume` does both
        // (`src/main.rs:1706-1711`) and its Qt hop is safe from any thread.
        MediaEvent::SetVolume(v) => crate::transport_set_volume((v as f32).clamp(0.0, 1.0)),
        MediaEvent::SetPosition(micros) => seek_to_micros(micros),
        MediaEvent::SeekBy(delta_micros) => seek_by_micros(delta_micros),
    }
}

/// Absolute seek (MPRIS `SetPosition`).
///
/// Micros → a 0..1 FRACTION, because that is this app's seek primitive
/// (`crate::transport_seek`, `src/main.rs:1701`), which alone carries the full
/// cast → QConnect → local ladder (`src/playback_qt.rs:1586-1621`). The Slint
/// glue converts the same way (`crates/qbz/src/media_controls.rs:72-95`); the
/// daemon converts to SECONDS instead only because `core.seek(secs)` is its
/// primitive.
fn seek_to_micros(micros: i64) {
    let runtime = crate::app();
    let state = runtime.core().get_playback_state();
    if let Some(fraction) = fraction_for(micros, state.duration) {
        crate::transport_seek(fraction);
    }
}

/// Relative seek (MPRIS `Seek`), signed micros.
fn seek_by_micros(delta_micros: i64) {
    // `position` / `duration` are SECONDS (`qbz-player`'s PlaybackState,
    // `crates/qbz-player/src/player/mod.rs:5447-5453`).
    let runtime = crate::app();
    let state = runtime.core().get_playback_state();
    let target = (state.position as i64) * 1_000_000 + delta_micros;
    if let Some(fraction) = fraction_for(target, state.duration) {
        crate::transport_seek(fraction);
    }
}

/// A 0..1 seek fraction for an absolute target in micros, or `None` when the
/// duration is unknown (a zero-duration seek would divide by zero — the
/// reference's `dur == 0` early return).
fn fraction_for(target_micros: i64, duration_secs: u64) -> Option<f32> {
    if duration_secs == 0 {
        return None;
    }
    let target = target_micros.max(0);
    Some((target as f64 / 1_000_000.0 / duration_secs as f64).clamp(0.0, 1.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_for_needs_a_known_duration() {
        assert_eq!(fraction_for(5_000_000, 0), None);
    }

    #[test]
    fn fraction_for_maps_micros_onto_the_track() {
        assert_eq!(fraction_for(0, 200), Some(0.0));
        assert_eq!(fraction_for(100_000_000, 200), Some(0.5));
        assert_eq!(fraction_for(200_000_000, 200), Some(1.0));
    }

    #[test]
    fn fraction_for_clamps_both_ends() {
        // A relative seek back past the start, and past the end.
        assert_eq!(fraction_for(-30_000_000, 200), Some(0.0));
        assert_eq!(fraction_for(999_000_000, 200), Some(1.0));
    }
}
