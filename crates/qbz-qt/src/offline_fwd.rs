//! Qt-side glue for the shared offline-MODE engine — the cxx-qt port of
//! `crates/qbz/src/offline_mode.rs` MINUS the Slint parts.
//!
//! Offline MODE = the app operating without Qobuz — NOT the offline CACHE
//! (downloads). The engine, connectivity actor and persisted settings are
//! frontend-agnostic (`qbz_app::offline_mode`, ADR-006); this module owns
//! only the process globals, the per-user binding, and the status -> QML
//! forwarder (`engine().subscribe()` watch -> `CxxQtThread::queue`).
//!
//! Remaining parity debt: the subscription-expiry purge consumer is not
//! spawned. Connectivity recheck, Settings wiring, the download cache and the
//! offline-playback eligibility gate are live.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use qbz_app::offline_mode::{
    Connectivity, ConnectivityActor, OfflineMode, OfflineModeEngine, OfflineStatus,
};
use qbz_app::settings::subscription::SubscriptionStateStore;
use qbz_app::user_data::UserDataPaths;

/// Process-global engine. Exists from first use; per-user state binds via
/// [`init_for_user`], connectivity via [`start`].
static ENGINE: LazyLock<Arc<OfflineModeEngine>> =
    LazyLock::new(|| Arc::new(OfflineModeEngine::new()));

/// The connectivity actor, spawned once per process by [`start`].
static CONNECTIVITY: OnceLock<ConnectivityActor> = OnceLock::new();

/// Per-user subscription state (D4). `None` until a session (online or
/// offline) is activated; consumers fail open in that window.
static SUBSCRIPTION: Mutex<Option<SubscriptionStateStore>> = Mutex::new(None);

pub fn engine() -> Arc<OfflineModeEngine> {
    Arc::clone(&ENGINE)
}

/// Exclude registered network folders only when connectivity is confirmed
/// down. Manual offline mode with a live LAN intentionally keeps NAS content
/// visible; `Unknown` also fails open to avoid hiding it during boot probes.
pub fn exclude_network_folders_now() -> bool {
    engine().status().connectivity == Connectivity::Down
}

/// Spawn the connectivity actor and attach it to the engine. Called once
/// during boot (from a tokio context — the spawns need the runtime); the
/// monitoring runs for the whole app lifetime, login screen included (the
/// restore flow and the D2 recovery banner read it).
pub fn start() {
    if CONNECTIVITY.get().is_some() {
        return;
    }
    let actor = ConnectivityActor::spawn();
    engine().attach_connectivity(&actor);
    if CONNECTIVITY.set(actor).is_err() {
        log::warn!("[qbz-qt] offline mode: connectivity actor already started");
    } else {
        log::info!("[qbz-qt] offline mode: connectivity monitoring started");
    }
}

/// Settings > Offline "Check now": ask the actor for an immediate probe
/// instead of waiting for its next scheduled one.
///
/// The passthrough is the whole gap this closes — `ConnectivityActor::
/// request_recheck` has been public since the actor landed
/// (`qbz-app/src/offline_mode/connectivity.rs:471`) and the reference exposes
/// it the same way (`crates/qbz/src/offline_mode.rs:61-64`); this module just
/// never handed out its private `OnceLock`.
///
/// A no-op before boot, which is when there is nothing to re-check anyway.
pub fn request_recheck() {
    match CONNECTIVITY.get() {
        Some(actor) => actor.request_recheck(),
        None => log::warn!("[qbz-qt] offline recheck ignored: connectivity actor not started"),
    }
}

/// Mirror every engine status change into the bridge properties (the login
/// affordances + the D2 recovery banner read them). Also seeds
/// `hasPreviousSession` once; `enter_shell` refreshes it after a successful
/// login. Spawned once from the boot sequence.
pub fn start_ui_forwarder() {
    let has_previous = UserDataPaths::load_last_user_id().is_some();
    crate::session_bridge::ui(move |mut b| b.as_mut().set_has_previous_session(has_previous));

    crate::spawn(async move {
        let mut rx = engine().subscribe();
        let mut previous_connectivity = None;
        loop {
            let status = *rx.borrow_and_update();
            let network_visibility_changed = previous_connectivity
                .map(|previous| {
                    (previous == Connectivity::Down) != (status.connectivity == Connectivity::Down)
                })
                .unwrap_or(false);
            previous_connectivity = Some(status.connectivity);
            crate::session_bridge::ui(move |b| apply_status(b, status));
            if network_visibility_changed && crate::local_state::has_library() {
                crate::local_bridge_ops::reload_browse();
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });
}

/// Push one engine status snapshot into the bridge (queued on the Qt
/// thread via [`crate::ui`]).
fn apply_status(
    mut b: core::pin::Pin<&mut crate::session_bridge::qbz_session::QbzSession>,
    status: OfflineStatus,
) {
    b.as_mut().set_offline(status.is_offline());
    b.as_mut().set_offline_mode(match status.mode {
        OfflineMode::Online => 0,
        OfflineMode::RealOffline => 1,
        OfflineMode::InducedOffline => 2,
    });
    b.as_mut().set_connectivity(match status.connectivity {
        Connectivity::Unknown => 0,
        Connectivity::Up => 1,
        Connectivity::Down => 2,
    });
    b.as_mut().set_captive_portal(status.captive_portal);
    b.as_mut()
        .set_show_recovery_banner(status.show_recovery_banner());
    // The header badge's "Logged out" state needs the raw session flag —
    // show_recovery_banner() is false while connectivity is down.
    b.as_mut().set_offline_session(status.offline_session);
}

/// `<data_dir>/qbz/users/<user_id>/` — the per-user directory both the
/// engine store and the subscription store live in. Matches the Tauri and
/// Slint per-user path.
pub fn user_data_dir(user_id: u64) -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("qbz")
            .join("users")
            .join(user_id.to_string()),
    )
}

/// Bind the engine + subscription store to the active user's data dir.
/// Called on every session activation (login, restore, offline entry).
/// Best-effort: failures are logged, never block entry.
///
/// The Slint version also spawns the subscription purge check here; that
/// consumer remains the module's one documented parity gap.
pub fn init_for_user(base_dir: &Path) {
    if let Err(e) = engine().init_for_user(base_dir) {
        log::error!("[qbz-qt] offline mode engine init failed: {e}");
    }
    match SubscriptionStateStore::new_at(base_dir) {
        Ok(store) => {
            if let Ok(mut guard) = SUBSCRIPTION.lock() {
                *guard = Some(store);
            }
        }
        // Fail-open: no store means no recorded invalidity, playback allowed.
        Err(e) => log::error!("[qbz-qt] subscription state store open failed: {e}"),
    }
}

/// Drop the per-user state on logout. The engine also ends the
/// session-scoped offline state (offline_session + cached induced flag) and
/// reopens the Qobuz gate when connectivity allows — a logged-out user must
/// always be able to sign back in.
pub fn teardown() {
    engine().teardown();
    if let Ok(mut guard) = SUBSCRIPTION.lock() {
        *guard = None;
    }
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Whether full offline-cache tracks may be served inside the subscription
/// grace window. Missing state fails open, matching the shared engine and the
/// reference frontend: only an explicit expired verdict may hide playback.
pub fn offline_playback_allowed() -> bool {
    let now = now_unix_secs();
    SUBSCRIPTION
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .map(|store| store.offline_playback_allowed(now).unwrap_or(true))
        })
        .unwrap_or(true)
}
