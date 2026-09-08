//! Offline MODE engine (frontend-agnostic, ADR-006).
//!
//! The app operating without Qobuz — NOT the offline cache. Spec:
//! `qbz-nix-docs/offline-mode/2026-06-09-offline-mode-slint-port-spec.md`.
//!
//! Three states (D1):
//! - `Online` — connectivity up, Qobuz services available.
//! - `RealOffline` — detected connectivity loss, or a session started via
//!   "Start offline" with no Qobuz auth. SESSION-SCOPED: never persisted.
//! - `InducedOffline` — the user's persisted opt-in from Settings.
//!
//! Invariants:
//! - Offline (either flavor) ⇒ ZERO Qobuz services (D3). The engine owns the
//!   process-wide `qbz_qobuz::offline_gate`, flipping it on every transition.
//! - Induced wins over real for display; the raw connectivity rides along in
//!   the status so the UI can render the recovery banner logic (D2).
//! - Exiting induced offline is ALWAYS allowed (no probe gate — Tauri's
//!   trap is not ported); the state simply re-evaluates afterwards.
//! - Entering induced offline snapshots `audio_settings.stream_first_track`
//!   and forces it false; exiting restores it (issue #279 parity).

pub mod connectivity;
pub mod store;

pub use connectivity::{Connectivity, ConnectivityActor, ConnectivitySnapshot};
pub use store::{OfflineModeSettings, OfflineModeStore, QueuedScrobble};

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::watch;

use qbz_audio::settings::AudioSettingsStore;

/// The app-level offline mode (derived; see [`OfflineModeEngine`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflineMode {
    Online,
    RealOffline,
    InducedOffline,
}

/// Full status broadcast to UIs on every change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineStatus {
    pub mode: OfflineMode,
    /// Raw connectivity, independent of the mode (the banner shows when an
    /// offline SESSION is active but connectivity is back).
    pub connectivity: Connectivity,
    /// Captive-portal hint from the prober.
    pub captive_portal: bool,
    /// The persisted induced flag (mirrors Settings).
    pub induced: bool,
    /// Session was started without Qobuz auth ("Start offline" from login).
    pub offline_session: bool,
}

impl OfflineStatus {
    pub fn is_offline(&self) -> bool {
        self.mode != OfflineMode::Online
    }

    /// D2: show the one-click login banner — an unauthenticated offline
    /// session while connectivity is actually up (and the user did not opt
    /// into induced offline).
    pub fn show_recovery_banner(&self) -> bool {
        self.offline_session && !self.induced && self.connectivity == Connectivity::Up
    }
}

fn default_status() -> OfflineStatus {
    OfflineStatus {
        mode: OfflineMode::Online,
        connectivity: Connectivity::Unknown,
        captive_portal: false,
        induced: false,
        offline_session: false,
    }
}

/// The engine. One per process; frontends hold it in an `Arc`.
pub struct OfflineModeEngine {
    store: Mutex<Option<OfflineModeStore>>,
    induced: AtomicBool,
    offline_session: AtomicBool,
    status_tx: watch::Sender<OfflineStatus>,
    connectivity: Mutex<ConnectivitySnapshot>,
}

impl OfflineModeEngine {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(None),
            induced: AtomicBool::new(false),
            offline_session: AtomicBool::new(false),
            status_tx: watch::channel(default_status()).0,
            connectivity: Mutex::new(ConnectivitySnapshot::default()),
        }
    }

    /// Open the per-user settings store and load the persisted induced flag.
    /// Call on session activation (online or offline).
    pub fn init_for_user(&self, base_dir: &Path) -> Result<(), String> {
        let store = OfflineModeStore::new_at(base_dir)?;
        let induced = store.get_settings()?.manual_offline_mode;
        {
            let mut guard = self
                .store
                .lock()
                .map_err(|e| format!("offline store lock poisoned: {}", e))?;
            *guard = Some(store);
        }
        self.induced.store(induced, Ordering::Relaxed);
        self.recompute();
        Ok(())
    }

    /// Drop the per-user store AND end the session-scoped offline state
    /// (logout). Ending the session ends the session: `offline_session` is
    /// reset (it must not survive into the next login attempt — a surviving
    /// flag kept the Qobuz gate closed and refused the login itself), and the
    /// cached `induced` flag is reset too (no user ⇒ no induced opt-in
    /// active; the user's persisted preference reloads from disk on the next
    /// `init_for_user`). The final `recompute()` reopens the Qobuz gate when
    /// connectivity allows.
    pub fn teardown(&self) {
        if let Ok(mut guard) = self.store.lock() {
            *guard = None;
        }
        self.offline_session.store(false, Ordering::Relaxed);
        self.induced.store(false, Ordering::Relaxed);
        self.recompute();
    }

    /// Subscribe to status changes (UI listeners, QConnect suppressor, ...).
    pub fn subscribe(&self) -> watch::Receiver<OfflineStatus> {
        self.status_tx.subscribe()
    }

    /// Current status snapshot.
    pub fn status(&self) -> OfflineStatus {
        *self.status_tx.borrow()
    }

    /// Convenience: is ANY offline flavor active?
    pub fn is_offline(&self) -> bool {
        self.status().is_offline()
    }

    /// Read the persisted settings (Settings view).
    pub fn settings(&self) -> Result<OfflineModeSettings, String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("offline store lock poisoned: {}", e))?;
        let store = guard.as_ref().ok_or("No active session")?;
        store.get_settings()
    }

    /// Restore the Tauri manual-offline policy: submit scrobbles through the
    /// network immediately even while the Qobuz offline gate is induced.
    /// Enabling it atomically disables accumulated mode in the shared store.
    pub fn set_allow_immediate_scrobbling(&self, enabled: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("offline store lock poisoned: {}", e))?;
        let store = guard.as_ref().ok_or("No active session")?;
        store.set_allow_immediate_scrobbling(enabled)
    }

    /// Restore the Tauri physical-offline policy: retain scrobbles until the
    /// network is usable. Enabling it atomically disables immediate mode.
    pub fn set_allow_accumulated_scrobbling(&self, enabled: bool) -> Result<(), String> {
        let guard = self
            .store
            .lock()
            .map_err(|e| format!("offline store lock poisoned: {}", e))?;
        let store = guard.as_ref().ok_or("No active session")?;
        store.set_allow_accumulated_scrobbling(enabled)
    }

    /// Flip induced offline (Settings toggle). Always succeeds in either
    /// direction; persists the flag, handles the #279 snapshot/restore, then
    /// recomputes the mode (which flips the Qobuz gate).
    ///
    /// `audio` is best-effort: pre-login there may be no audio store yet.
    pub fn set_induced(
        &self,
        enabled: bool,
        audio: Option<&AudioSettingsStore>,
    ) -> Result<OfflineStatus, String> {
        let was = {
            let guard = self
                .store
                .lock()
                .map_err(|e| format!("offline store lock poisoned: {}", e))?;
            let store = guard.as_ref().ok_or("No active session")?;
            let was = store.get_settings()?.manual_offline_mode;
            store.set_manual_offline_mode(enabled)?;

            if let Some(audio_store) = audio {
                if enabled && !was {
                    // Entering: stash the current preference, force false.
                    if let Ok(settings) = audio_store.get_settings() {
                        let _ = store
                            .set_pre_offline_stream_first_track(Some(settings.stream_first_track));
                        if settings.stream_first_track {
                            let _ = audio_store.set_stream_first_track(false);
                            log::info!(
                                "[OfflineMode] stream_first_track snapshot=true; forced false while offline (#279)"
                            );
                        }
                    }
                } else if !enabled && was {
                    // Exiting: restore the stash, clear it.
                    if let Ok(Some(snapshot)) = store.get_pre_offline_stream_first_track() {
                        let _ = audio_store.set_stream_first_track(snapshot);
                        let _ = store.set_pre_offline_stream_first_track(None);
                        log::info!("[OfflineMode] stream_first_track restored to {} (#279)", snapshot);
                    }
                }
            }
            was
        };
        let _ = was;

        self.induced.store(enabled, Ordering::Relaxed);
        self.recompute();
        Ok(self.status())
    }

    /// Mark/unmark the session as an unauthenticated offline session
    /// ("Start offline" from the login screen). Session-scoped (D1): callers
    /// set it on `enter_shell_offline` and clear it after a successful login.
    pub fn set_offline_session(&self, active: bool) {
        self.offline_session.store(active, Ordering::Relaxed);
        self.recompute();
    }

    /// Feed a fresh connectivity snapshot (the engine's listener task calls
    /// this on every actor broadcast).
    pub fn on_connectivity(&self, snapshot: ConnectivitySnapshot) {
        if let Ok(mut guard) = self.connectivity.lock() {
            *guard = snapshot;
        }
        self.recompute();
    }

    /// Spawn the listener wiring an actor subscription into the engine.
    /// Returns immediately; the task lives for the process lifetime.
    pub fn attach_connectivity(self: &std::sync::Arc<Self>, actor: &ConnectivityActor) {
        let mut rx = actor.subscribe();
        let engine = std::sync::Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if rx.changed().await.is_err() {
                    return;
                }
                let snapshot = *rx.borrow();
                engine.on_connectivity(snapshot);
            }
        });
    }

    /// Derive the mode, flip the Qobuz gate, broadcast on change.
    fn recompute(&self) {
        let induced = self.induced.load(Ordering::Relaxed);
        let offline_session = self.offline_session.load(Ordering::Relaxed);
        let connectivity = self
            .connectivity
            .lock()
            .map(|guard| *guard)
            .unwrap_or_default();

        let mode = if induced {
            OfflineMode::InducedOffline
        } else if offline_session || connectivity.state == Connectivity::Down {
            OfflineMode::RealOffline
        } else {
            OfflineMode::Online
        };

        let status = OfflineStatus {
            mode,
            connectivity: connectivity.state,
            captive_portal: connectivity.captive_portal,
            induced,
            offline_session,
        };

        // D3: the single Qobuz choke point follows the mode.
        qbz_qobuz::offline_gate::set_offline(mode != OfflineMode::Online);

        let _ = self.status_tx.send_if_modified(|current| {
            if *current != status {
                log::info!(
                    "[OfflineMode] {:?} -> {:?} (connectivity {:?}, induced {}, offline_session {})",
                    current.mode,
                    status.mode,
                    status.connectivity,
                    status.induced,
                    status.offline_session
                );
                *current = status;
                true
            } else {
                false
            }
        });
    }
}

impl Default for OfflineModeEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let nonce = NONCE.fetch_add(1, AtomicOrdering::Relaxed);
        std::env::temp_dir().join(format!("qbz-app-{name}-{}-{nonce}", std::process::id()))
    }

    /// The engine flips the PROCESS-GLOBAL `qbz_qobuz::offline_gate`; these
    /// tests must not run concurrently or the gate assertions race.
    static GATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serialize() -> std::sync::MutexGuard<'static, ()> {
        GATE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn up() -> ConnectivitySnapshot {
        ConnectivitySnapshot {
            state: Connectivity::Up,
            captive_portal: false,
        }
    }

    fn down() -> ConnectivitySnapshot {
        ConnectivitySnapshot {
            state: Connectivity::Down,
            captive_portal: false,
        }
    }

    #[test]
    fn starts_online_with_unknown_connectivity() {
        let _gate = serialize();
        let engine = OfflineModeEngine::new();
        let status = engine.status();
        assert_eq!(status.mode, OfflineMode::Online);
        assert_eq!(status.connectivity, Connectivity::Unknown);
    }

    #[test]
    fn connectivity_down_is_real_offline_and_back() {
        let _gate = serialize();
        let engine = OfflineModeEngine::new();
        engine.on_connectivity(down());
        assert_eq!(engine.status().mode, OfflineMode::RealOffline);
        assert!(qbz_qobuz::offline_gate::is_offline());

        engine.on_connectivity(up());
        assert_eq!(engine.status().mode, OfflineMode::Online);
        assert!(!qbz_qobuz::offline_gate::is_offline());
    }

    #[test]
    fn induced_wins_over_connectivity() {
        let _gate = serialize();
        let dir = unique_test_dir("engine-induced");
        let engine = OfflineModeEngine::new();
        engine.init_for_user(&dir).unwrap();

        engine.on_connectivity(up());
        engine.set_induced(true, None).unwrap();
        assert_eq!(engine.status().mode, OfflineMode::InducedOffline);
        assert!(engine.status().induced);
        assert!(qbz_qobuz::offline_gate::is_offline());

        // Exit always allowed; connectivity Up => back Online.
        engine.set_induced(false, None).unwrap();
        assert_eq!(engine.status().mode, OfflineMode::Online);
        assert!(!qbz_qobuz::offline_gate::is_offline());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn induced_flag_is_persisted_and_reloaded() {
        let _gate = serialize();
        let dir = unique_test_dir("engine-persist");
        {
            let engine = OfflineModeEngine::new();
            engine.init_for_user(&dir).unwrap();
            engine.set_induced(true, None).unwrap();
        }
        {
            let engine = OfflineModeEngine::new();
            engine.init_for_user(&dir).unwrap();
            assert_eq!(engine.status().mode, OfflineMode::InducedOffline);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn offline_session_is_real_offline_even_with_connectivity_up() {
        let _gate = serialize();
        let engine = OfflineModeEngine::new();
        engine.on_connectivity(up());
        engine.set_offline_session(true);

        let status = engine.status();
        assert_eq!(status.mode, OfflineMode::RealOffline);
        assert!(status.show_recovery_banner(), "banner: session offline but net is back");

        engine.set_offline_session(false);
        assert_eq!(engine.status().mode, OfflineMode::Online);
    }

    #[test]
    fn no_banner_while_connectivity_down_or_induced() {
        let _gate = serialize();
        let dir = unique_test_dir("engine-banner");
        let engine = OfflineModeEngine::new();
        engine.init_for_user(&dir).unwrap();

        engine.set_offline_session(true);
        engine.on_connectivity(down());
        assert!(!engine.status().show_recovery_banner());

        engine.on_connectivity(up());
        engine.set_induced(true, None).unwrap();
        assert!(!engine.status().show_recovery_banner());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stream_first_snapshot_is_stashed_and_restored() {
        let _gate = serialize();
        let dir = unique_test_dir("engine-279");
        let audio_dir = unique_test_dir("engine-279-audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = AudioSettingsStore::new_at(&audio_dir).unwrap();
        audio.set_stream_first_track(true).unwrap();

        let engine = OfflineModeEngine::new();
        engine.init_for_user(&dir).unwrap();

        engine.set_induced(true, Some(&audio)).unwrap();
        assert!(!audio.get_settings().unwrap().stream_first_track, "#279: forced false");

        engine.set_induced(false, Some(&audio)).unwrap();
        assert!(audio.get_settings().unwrap().stream_first_track, "#279: restored");

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(audio_dir);
    }

    #[test]
    fn teardown_ends_offline_session_and_reopens_gate() {
        let _gate = serialize();
        let engine = OfflineModeEngine::new();
        engine.on_connectivity(up());
        engine.set_offline_session(true);
        assert_eq!(engine.status().mode, OfflineMode::RealOffline);
        assert!(qbz_qobuz::offline_gate::is_offline());

        // Logout: the session-scoped flag must NOT survive — a stale flag
        // kept the gate closed and refused the next login.
        engine.teardown();
        let status = engine.status();
        assert_eq!(status.mode, OfflineMode::Online);
        assert!(!status.offline_session);
        assert!(!qbz_qobuz::offline_gate::is_offline());
    }

    #[test]
    fn teardown_clears_induced_cache_but_disk_restores_it() {
        let _gate = serialize();
        let dir = unique_test_dir("engine-teardown-induced");
        let engine = OfflineModeEngine::new();
        engine.init_for_user(&dir).unwrap();
        engine.on_connectivity(up());
        engine.set_induced(true, None).unwrap();
        assert!(qbz_qobuz::offline_gate::is_offline());

        // Logout: no user ⇒ no induced opt-in active ⇒ gate open.
        engine.teardown();
        let status = engine.status();
        assert_eq!(status.mode, OfflineMode::Online);
        assert!(!status.induced);
        assert!(!qbz_qobuz::offline_gate::is_offline());

        // The persisted preference survives on disk: the next activation on
        // the same dir restores induced offline.
        engine.init_for_user(&dir).unwrap();
        assert_eq!(engine.status().mode, OfflineMode::InducedOffline);
        assert!(qbz_qobuz::offline_gate::is_offline());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn watch_broadcasts_on_change() {
        let _gate = serialize();
        let engine = std::sync::Arc::new(OfflineModeEngine::new());
        let mut rx = engine.subscribe();

        engine.on_connectivity(down());
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow().mode, OfflineMode::RealOffline);
    }
}
