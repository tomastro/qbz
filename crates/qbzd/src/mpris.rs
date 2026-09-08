// crates/qbzd/src/mpris.rs — MPRIS system media controls (CONSOLE ext).
//
// Publishes the daemon's playback over the standard org.mpris.MediaPlayer2
// D-Bus interface via qbz-media-controls (mpris-server on Linux, INCLUDING the
// DesktopEntry that lets GNOME/KDE resolve the app icon). This is what makes a
// KDE Plasma media widget — or a plasmoid — control the daemon with NO custom
// client code, and makes hardware media keys work.
//
// Two halves:
//   * OUTBOUND — a CoreEvent-bus subscriber pushes now-playing metadata plus
//     play/pause/position/volume into the OS controls.
//   * INBOUND — the qbz-media-controls callback maps MediaEvent (media keys,
//     the desktop widget) back onto core transport commands.
//
// The inbound callback holds only a Weak<AppRuntime> (upgraded per event), and
// the updater task upgrades a Weak once to seed then drops it — so the
// integration NEVER pins the runtime in steady state and shutdown ordering
// (#521: the audio device must release before drop(booted)) is unaffected.
//
// Enablement: on by default where a session bus exists; `QBZD_MPRIS` in
// {0,false,off,no} disables it. On a headless server with no D-Bus, `spawn`
// returns None gracefully even when enabled — the daemon runs fine without it.
use std::sync::{Arc, Weak};
use std::time::Duration;

use qbz_app::shell::AppRuntime;
use qbz_media_controls::{MediaEvent, MediaIntegration, PlaybackStatus, TrackMeta};
use qbz_models::{CoreEvent, PlaybackState, QueueTrack};
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::adapter::DaemonAdapter;
use crate::paths::ProfileRoots;
use crate::qconnect::authority::AuthorityCell;

type Runtime = Arc<AppRuntime<DaemonAdapter>>;

/// A running MPRIS integration: the OS-controls handle (kept alive so the D-Bus
/// service stays published) plus the bus→controls updater task.
pub struct MprisHandle {
    integration: Arc<dyn MediaIntegration>,
    updater: JoinHandle<()>,
}

impl MprisHandle {
    /// Abort the updater and drop the OS-controls handle (tears down the D-Bus
    /// service). The inbound callback held only a Weak<AppRuntime>, so this does
    /// not participate in the #521 audio-release ordering.
    pub async fn shutdown(self) {
        self.updater.abort();
        let _ = self.updater.await;
        drop(self.integration);
    }
}

/// Whether MPRIS should be published. The `QBZD_MPRIS` env var wins when set
/// (deploy/override knob); otherwise the persisted `daemon_prefs.mpris_enabled`
/// toggle decides (default ON), which is what the setup-TUI Playback screen and
/// `qbzd settings set playback.mpris` write.
fn enabled(roots: &ProfileRoots) -> bool {
    if let Ok(v) = std::env::var("QBZD_MPRIS") {
        return !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        );
    }
    qbz_app::settings::daemon_prefs::load_at(&roots.data).mpris_enabled
}

/// Spawn the MPRIS integration. Returns None when disabled, on a non-Linux
/// platform, or when the D-Bus backend can't start (headless server).
pub fn spawn(
    runtime: &Runtime,
    roots: ProfileRoots,
    mut bus: broadcast::Receiver<CoreEvent>,
    handle: Handle,
    authority: Arc<AuthorityCell>,
) -> Option<MprisHandle> {
    if !enabled(&roots) {
        log::info!("[mpris] disabled (playback.mpris / QBZD_MPRIS)");
        return None;
    }

    // INBOUND: media keys / desktop widget → core transport. Weak so the OS
    // integration never keeps the runtime alive.
    let weak: Weak<AppRuntime<DaemonAdapter>> = Arc::downgrade(runtime);
    let cb_handle = handle.clone();
    let cb_authority = Arc::clone(&authority);
    // Headless: the daemon owns no window, so there is no HWND to give.
    // On Linux, which is the only place qbzd ships, MPRIS keys off the D-Bus
    // name and ignores this entirely.
    let integration: Arc<dyn MediaIntegration> = Arc::from(qbz_media_controls::spawn(
        move |ev| {
            if let Some(rt) = weak.upgrade() {
                handle_media_event(&rt, &roots, &cb_handle, &cb_authority, ev);
            }
        },
        qbz_media_controls::NativeWindow::default(),
    )?);
    log::info!("[mpris] publishing org.mpris.MediaPlayer2 (desktop media controls + media keys)");

    // OUTBOUND: seed once from live state, then follow the bus.
    let seed_weak: Weak<AppRuntime<DaemonAdapter>> = Arc::downgrade(runtime);
    let updater_integ = integration.clone();
    let updater = handle.spawn(async move {
        use broadcast::error::RecvError;
        let mut last = PlaybackStatus::Stopped;

        // One-time seed so the widget isn't blank until the next event. The
        // strong Arc is dropped before the loop, keeping the task Weak-only.
        if let Some(rt) = seed_weak.upgrade() {
            let queue = rt.core().get_queue_state().await;
            if let Some(track) = queue.current_track.as_ref() {
                updater_integ.set_metadata(&track_meta(track));
            }
            let player = rt.core().player();
            let ev = player.get_playback_event();
            last = if ev.is_playing {
                PlaybackStatus::Playing
            } else if player.has_loaded_audio() {
                PlaybackStatus::Paused
            } else {
                PlaybackStatus::Stopped
            };
            updater_integ.set_playback(last, Some(Duration::from_secs(ev.position)));
            updater_integ.set_volume(ev.volume as f64);
        }

        loop {
            match bus.recv().await {
                Ok(CoreEvent::TrackStarted {
                    track,
                    position_secs,
                }) => {
                    updater_integ.set_metadata(&track_meta(&track));
                    last = PlaybackStatus::Playing;
                    updater_integ.set_playback(last, Some(Duration::from_secs(position_secs)));
                }
                Ok(CoreEvent::PlaybackStateChanged { state }) => {
                    last = map_state(state);
                    updater_integ.set_playback(last, None);
                }
                Ok(CoreEvent::PositionUpdated { position_secs, .. }) => {
                    // Keep the widget's progress bar live while playing.
                    if last == PlaybackStatus::Playing {
                        updater_integ.set_playback(last, Some(Duration::from_secs(position_secs)));
                    }
                }
                Ok(CoreEvent::VolumeChanged { volume }) => updater_integ.set_volume(volume as f64),
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            }
        }
    });

    Some(MprisHandle {
        integration,
        updater,
    })
}

// ============================ inbound ============================

/// Map one inbound `MediaEvent` onto a core transport command. Runs on the
/// mpris-server (D-Bus) thread — NOT a tokio worker — so the sync core commands
/// are called directly; the async advance ritual is spawned fire-and-forget so
/// the D-Bus thread never blocks on a network resolve. Time values are micros.
fn handle_media_event(
    rt: &Runtime,
    roots: &ProfileRoots,
    handle: &Handle,
    authority: &Arc<AuthorityCell>,
    ev: MediaEvent,
) {
    let core = rt.core();
    match ev {
        MediaEvent::Play => {
            let Some(_transport_permit) = authority.try_transport_action_permit() else {
                return;
            };
            let _ = core.resume();
        }
        MediaEvent::Pause => {
            let Some(_transport_permit) = authority.try_transport_action_permit() else {
                return;
            };
            let _ = core.pause();
        }
        MediaEvent::Toggle => {
            let Some(_transport_permit) = authority.try_transport_action_permit() else {
                return;
            };
            let player = core.player();
            if player.get_playback_event().is_playing {
                let _ = core.pause();
            } else if player.has_loaded_audio() {
                let _ = core.resume();
            }
        }
        MediaEvent::Stop => {
            let Some(_transport_permit) = authority.try_transport_action_permit() else {
                return;
            };
            let _ = core.stop();
        }
        MediaEvent::Next => {
            if let Some(permit) = authority.try_owner_action_permit() {
                spawn_advance(rt, roots, handle, permit, true);
            }
        }
        MediaEvent::Previous => {
            if let Some(permit) = authority.try_owner_action_permit() {
                spawn_advance(rt, roots, handle, permit, false);
            }
        }
        MediaEvent::SeekBy(micros) => {
            let Some(_transport_permit) = authority.try_transport_action_permit() else {
                return;
            };
            let player = core.player();
            let ev = player.get_playback_event();
            let target = (ev.position as i64 + micros / 1_000_000).max(0) as u64;
            let clamped = if ev.duration > 0 {
                target.min(ev.duration)
            } else {
                target
            };
            let _ = core.seek(clamped);
        }
        MediaEvent::SetPosition(micros) => {
            let Some(_transport_permit) = authority.try_transport_action_permit() else {
                return;
            };
            let player = core.player();
            let ev = player.get_playback_event();
            let target = (micros.max(0) as u64) / 1_000_000;
            let clamped = if ev.duration > 0 {
                target.min(ev.duration)
            } else {
                target
            };
            let _ = core.seek(clamped);
        }
        MediaEvent::SetVolume(vol) => {
            let Some(_transport_permit) = authority.try_transport_action_permit() else {
                return;
            };
            let player = core.player();
            if !player.is_dsd_direct_active() {
                let _ = core.set_volume((vol as f32).clamp(0.0, 1.0));
            }
        }
        // Headless daemon: no window to raise, and self-quit on a media-widget
        // "close" would be surprising — ignore both.
        MediaEvent::Raise | MediaEvent::Quit => {}
    }
}

/// Fire-and-forget the FULL advance ritual (skip-walk → play → prefetch →
/// persist) off the D-Bus thread, at the daemon's persisted streaming quality
/// (the same key the driver seeds at boot).
fn spawn_advance(
    rt: &Runtime,
    roots: &ProfileRoots,
    handle: &Handle,
    permit: crate::qconnect::authority::AuthorityActionPermit,
    forward: bool,
) {
    let rt = rt.clone();
    let quality = qbz_app::playback_driver::quality_from_key(
        &qbz_app::settings::daemon_prefs::load_at(&roots.data).streaming_quality,
    );
    handle.spawn(async move {
        let _permit = permit;
        let _ = qbz_app::playback_driver::advance_and_play(rt.as_ref(), quality, forward).await;
    });
}

// ============================ mapping ============================

fn track_meta(t: &QueueTrack) -> TrackMeta {
    TrackMeta {
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        duration: (t.duration_secs > 0).then(|| Duration::from_secs(t.duration_secs)),
        art_url: t.artwork_url.clone(),
        url: qbz_media_controls::xesam_url_for(t.source.as_deref(), t.id),
    }
}

fn map_state(s: PlaybackState) -> PlaybackStatus {
    match s {
        PlaybackState::Playing => PlaybackStatus::Playing,
        PlaybackState::Paused => PlaybackStatus::Paused,
        PlaybackState::Stopped => PlaybackStatus::Stopped,
        // Buffering is still "playing" from the user's point of view.
        PlaybackState::Loading => PlaybackStatus::Playing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_state_covers_every_playback_state() {
        assert_eq!(map_state(PlaybackState::Playing), PlaybackStatus::Playing);
        assert_eq!(map_state(PlaybackState::Paused), PlaybackStatus::Paused);
        assert_eq!(map_state(PlaybackState::Stopped), PlaybackStatus::Stopped);
        assert_eq!(map_state(PlaybackState::Loading), PlaybackStatus::Playing);
    }

    #[test]
    fn enabled_defaults_on_and_respects_falsey_overrides() {
        // Default (unset) is ON; only explicit falsey values disable. We can't
        // safely mutate process env in parallel tests, so assert the pure
        // classification the getter uses.
        for v in ["0", "false", "off", "no", "FALSE", " Off "] {
            assert!(
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                ),
                "{v:?} should read as disabled"
            );
        }
        for v in ["1", "true", "on", "yes", "anything"] {
            assert!(
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                ),
                "{v:?} should read as enabled"
            );
        }
    }
}
