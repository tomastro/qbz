// crates/qbzd/src/events_bridge.rs — bridge playback-driver edges onto the
// CoreEvent bus.
//
// The 450 ms playback driver detects track/play-state transitions
// (`DriverAction::ReportEdge` -> `deps.on_edge`), but nothing translated those
// edges into the playback CoreEvent variants the bus consumers were written
// for: mpris.rs and scrobble_engine.rs both carry TrackStarted /
// PlaybackStateChanged / PositionUpdated / VolumeChanged arms that never
// fired, and `/api/events` (SSE) / `qbzd watch` carried no playback traffic at
// all. This task closes that gap: it wakes on the same edge Notify pulse the
// QConnect report scheduler uses (with a 2 s poll fallback, because
// `plan_tick` only reports edges while a track id is present — a stop that
// clears the track id would otherwise go unseen), reads the live player state,
// dedups against what it last published, and sends the delta to the bus.
//
// Holds only a Weak<AppRuntime> (upgraded per wake, dropped before the next
// wait), so it sits outside the #521 audio-release ordering — the caller
// aborts it for a clean shutdown, same as the queue-persist subscriber.
use std::sync::{Arc, Weak};
use std::time::Duration;

use qbz_app::shell::AppRuntime;
use qbz_models::{CoreEvent, PlaybackState};
use tokio::sync::{broadcast, Notify};
use tokio::task::JoinHandle;

use crate::adapter::DaemonAdapter;
use crate::qconnect::authority::{
    AuthorityCell, OwnerAuthorityObservation, OwnerAuthorityToken,
};

type Runtime = Arc<AppRuntime<DaemonAdapter>>;

/// The poll fallback cadence: only exercised when no edge pulse arrives (e.g.
/// a stop cleared the track id, which suppresses `ReportEdge`).
const FALLBACK_POLL: Duration = Duration::from_secs(2);

/// One playback observation plus the exact owner authority that produced it.
///
/// `None` is deliberately meaningful: the observation began while delegated
/// authority (or a handoff fence) was active and owner-only consumers must
/// treat it as a handoff, even if owner authority is restored before delivery.
#[derive(Clone, Debug)]
pub struct AuthorityStampedEvent {
    pub event: CoreEvent,
    pub owner_token: Option<OwnerAuthorityToken>,
}

/// Spawn the bridge task. Emits, deduped against the last published values:
/// * `TrackStarted` on a track-id change while playing (with the queue's
///   current-track metadata),
/// * `PlaybackStateChanged` on a playing/paused/stopped change,
/// * `PositionUpdated` on every wake while playing (~2 s cadence, the
///   scrobbler's timing source and the MPRIS progress feed),
/// * `VolumeChanged` on a volume change.
pub fn spawn(
    runtime: &Runtime,
    bus: broadcast::Sender<CoreEvent>,
    owner_bus: broadcast::Sender<AuthorityStampedEvent>,
    edge: Arc<Notify>,
    authority: Arc<AuthorityCell>,
) -> JoinHandle<()> {
    let weak: Weak<AppRuntime<DaemonAdapter>> = Arc::downgrade(runtime);
    tokio::spawn(async move {
        let mut last_track_id: u64 = 0;
        let mut last_state: Option<PlaybackState> = None;
        let mut last_volume: Option<f32> = None;
        let mut last_owner_token: Option<OwnerAuthorityToken> = None;
        loop {
            // Wake on a driver edge, or fall back to a slow poll so a
            // track-id-clearing stop still surfaces as a transition.
            let _ = tokio::time::timeout(FALLBACK_POLL, edge.notified()).await;

            // The observation is admitted before the first player/queue read.
            // Keeping the permit alive through publication makes a handoff wait
            // for this complete read -> event transaction. A delegated/fenced
            // observation remains stamped `None` even if an owner is installed
            // while the async queue lookup below is pending.
            let (owner_token, owner_permit) = match authority.observe_owner_authority() {
                OwnerAuthorityObservation::Owner { token, permit } => {
                    (Some(token), Some(permit))
                }
                OwnerAuthorityObservation::Delegated => (None, None),
                OwnerAuthorityObservation::Fenced => {
                    // A candidate activation can hold the fence while the
                    // installed authority is still owner and may ultimately
                    // fail. Do not turn that transient pause into a fabricated
                    // guest edge or disturb the dedup baseline.
                    continue;
                }
            };

            // Authority identity is part of the dedup key. In particular, a
            // guest -> owner restore of the same track/state must publish fresh
            // owner-stamped edges rather than leaving consumers on guest data.
            if owner_token != last_owner_token {
                last_track_id = 0;
                last_state = None;
                last_volume = None;
                last_owner_token = owner_token;
            }

            let Some(rt) = weak.upgrade() else { return };
            let core = rt.core();
            let player = core.player();
            let ev = player.get_playback_event();
            let has_audio = player.has_loaded_audio();

            if ev.track_id != 0 && ev.track_id != last_track_id && ev.is_playing {
                // The queue's current track carries the metadata (title/artist/
                // artwork). Skip when the cursor hasn't caught up yet — the next
                // wake retries because last_track_id only advances on a send.
                let queue = core.get_queue_state().await;
                if let Some(track) = queue.current_track.as_ref().filter(|t| t.id == ev.track_id) {
                    publish(
                        &bus,
                        &owner_bus,
                        owner_token,
                        CoreEvent::TrackStarted {
                            track: track.clone(),
                            position_secs: ev.position,
                        },
                    );
                    last_track_id = ev.track_id;
                }
            }

            // Same three-way mapping as the MPRIS seed: loaded-but-not-playing
            // audio is Paused, nothing loaded is Stopped. Dedup on the MAPPED
            // state, not `is_playing` — paused and stopped are both
            // not-playing, and a pause -> stop transition must still emit.
            let state = if ev.is_playing {
                PlaybackState::Playing
            } else if has_audio {
                PlaybackState::Paused
            } else {
                PlaybackState::Stopped
            };
            if last_state != Some(state) {
                publish(
                    &bus,
                    &owner_bus,
                    owner_token,
                    CoreEvent::PlaybackStateChanged { state },
                );
                last_state = Some(state);
                // A stop ends the "current track": forget it so replaying the
                // same track emits a fresh TrackStarted (scrobbling, hooks).
                if state == PlaybackState::Stopped {
                    last_track_id = 0;
                }
            }

            if ev.is_playing {
                publish(
                    &bus,
                    &owner_bus,
                    owner_token,
                    CoreEvent::PositionUpdated {
                        position_secs: ev.position,
                        duration_secs: ev.duration,
                    },
                );
            }

            if last_volume.is_none_or(|v| (v - ev.volume).abs() > 0.001) {
                publish(
                    &bus,
                    &owner_bus,
                    owner_token,
                    CoreEvent::VolumeChanged { volume: ev.volume },
                );
                last_volume = Some(ev.volume);
            }
            // Owner observations remain drain-visible through every async read
            // and both bus publications in this wake.
            drop(owner_permit);
        }
    })
}

fn publish(
    bus: &broadcast::Sender<CoreEvent>,
    owner_bus: &broadcast::Sender<AuthorityStampedEvent>,
    owner_token: Option<OwnerAuthorityToken>,
    event: CoreEvent,
) {
    let _ = owner_bus.send(AuthorityStampedEvent {
        event: event.clone(),
        owner_token,
    });
    let _ = bus.send(event);
}
