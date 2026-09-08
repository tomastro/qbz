// TODO(converge: qconnect-glue) — ported from crates/qbz/src/qconnect_service.rs
// `sync_local_queue_if_changed` @ f18960ba; do not fix bugs here without fixing
// the source, and vice versa.
//
//! Daemon-side local-queue -> Connect-cloud publish (the desktop's
//! `sync_local_queue_if_changed`, qconnect_service.rs:875).
//!
//! The daemon was queue RECEIVE-ONLY: a daemon-originated queue (CLI/TUI/MPRIS/
//! restored session) never reached the cloud, so controllers rendered a
//! different queue than the one actually playing (design doc had flagged this
//! as knowingly unported — design-input/qconnect-headless.md:250-252).
//!
//! The gates are the desktop's EXACT set, in the same order: live runtime ->
//! `is_local_renderer_active` (a peer owns playback -> the peer publishes) ->
//! offline-only skip -> non-empty -> echo-suppress vs the cloud's last-applied
//! queue -> per-session `last_pushed_queue_ids` latch -> resolvable projection
//! (local/Plex tracks are dropped while Qobuz and offline `qobuz_download`
//! tracks stay eligible — the latter carry their real Qobuz id). The desktop
//! toasts when rows are dropped; the daemon logs.
//!
//! Trigger: the desktop calls it on every track transition from its poll loop.
//! The daemon instead runs a debounced `CoreEvent::QueueUpdated` subscriber
//! (same pattern as `daemon.rs::spawn_queue_persist`), which ALSO covers queue
//! edits while paused/stopped — a transition-only hook would miss those.

use std::sync::{Arc, Mutex as StdMutex};

use qbz_app::shell::AppRuntime;
use qbz_models::{CoreEvent, QueueTrack};
use qconnect_app::{
    arm_local_queue_takeover, is_local_renderer_active, local_queue_takeover_needs_retry,
    qconnect_queue_track_is_resolvable, set_local_playback_conflict_pending, QueueCommandType,
};
use serde_json::json;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::authority::{AuthorityCell, AuthorityStamp};
use super::DaemonQconnectInner;
use crate::adapter::DaemonAdapter;

/// Push the local core queue to the Connect session when it differs from the
/// cloud's. No-op under any of the gates listed in the module docs. Echo-safe
/// by construction: the inbound materialize path sets `last_applied_queue_state`
/// to the very queue it materialized locally, so a controller-pushed queue
/// compares equal and is never bounced back.
pub async fn publish_local_queue_if_changed(
    inner: &Arc<StdMutex<DaemonQconnectInner>>,
    runtime: &Arc<AppRuntime<DaemonAdapter>>,
    authority: &AuthorityCell,
    expected_stamp: AuthorityStamp,
) {
    let _ = publish_local_queue(inner, runtime, authority, expected_stamp, false).await;
}

/// Takeover variant used when the daemon is already playing and no peer is.
/// It bypasses only the active-renderer echo gate; all authority, offline, and
/// provenance checks remain in force.
pub async fn publish_local_queue_for_takeover(
    inner: &Arc<StdMutex<DaemonQconnectInner>>,
    runtime: &Arc<AppRuntime<DaemonAdapter>>,
    authority: &AuthorityCell,
    expected_stamp: AuthorityStamp,
) -> bool {
    publish_local_queue(inner, runtime, authority, expected_stamp, true).await
}

fn resolvable_queue_projection(
    tracks: &[QueueTrack],
    current_index: Option<usize>,
) -> (Vec<u64>, Option<usize>, usize) {
    let clicked = current_index.unwrap_or(0);
    let mut kept = Vec::with_capacity(tracks.len());
    let mut projected_start = None;
    let mut dropped = 0;
    for (index, track) in tracks.iter().enumerate() {
        if !qconnect_queue_track_is_resolvable(track) {
            dropped += 1;
            continue;
        }
        if projected_start.is_none() && index >= clicked {
            projected_start = Some(kept.len());
        }
        kept.push(track.id);
    }
    if projected_start.is_none() && !kept.is_empty() {
        projected_start = Some(kept.len() - 1);
    }
    (kept, projected_start, dropped)
}

async fn publish_local_queue(
    inner: &Arc<StdMutex<DaemonQconnectInner>>,
    runtime: &Arc<AppRuntime<DaemonAdapter>>,
    authority: &AuthorityCell,
    expected_stamp: AuthorityStamp,
    force_takeover: bool,
) -> bool {
    let (app, sync_state, stamp) = {
        let guard = inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.runtime.as_ref() {
            Some(rt) if rt.stamp == expected_stamp => {
                (Arc::clone(&rt.app), Arc::clone(&rt.sync_state), rt.stamp)
            }
            None => return false,
            Some(_) => return false,
        }
    };
    if !authority.is_current(stamp) {
        return false;
    }

    // Only push while WE are the active renderer (the user is driving the
    // daemon). When a peer owns playback, the peer publishes its own queue.
    {
        let state = sync_state.lock().await;
        if !authority.is_current(stamp) {
            return false;
        }
        if !force_takeover && !is_local_renderer_active(&state.session) {
            return false;
        }
    }

    // A queue built from an OFFLINE-ONLY local playlist never reaches the
    // Connect cloud. Debug level — this runs after every queue mutation and
    // must not spam the log.
    if runtime.core().queue_is_offline_only() {
        log::debug!("[QConnect] queue is from an offline-only playlist; skipping cloud push");
        return false;
    }

    let (tracks, current_index) = runtime.core().get_all_queue_tracks().await;
    if !authority.is_current(stamp) {
        return false;
    }
    if tracks.is_empty() {
        return false;
    }
    let source_ordered_ids: Vec<u64> = tracks.iter().map(|track| track.id).collect();
    let takeover_retry = {
        let state = sync_state.lock().await;
        if !authority.is_current(stamp) {
            return false;
        }
        local_queue_takeover_needs_retry(&state)
    };

    // Echo-suppress: skip when this is the cloud's current queue (materialized
    // inbound) so our own adoption / a remote queue change never bounces back.
    if !force_takeover && !takeover_retry {
        let state = sync_state.lock().await;
        if !authority.is_current(stamp) {
            return false;
        }
        if let Some(applied) = &state.last_applied_queue_state {
            let applied_ids: Vec<u64> = applied
                .queue_items
                .iter()
                .map(|item| item.track_id)
                .collect();
            if applied_ids == source_ordered_ids {
                return false;
            }
        }
    }
    // ...and skip when we already pushed this exact queue (cloud echo pending).
    if !authority.is_current(stamp) {
        return false;
    }
    if !force_takeover && !takeover_retry {
        let guard = inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !installed_runtime_matches(&guard, stamp) {
            return false;
        }
        if guard.last_pushed_queue_ids.as_deref() == Some(source_ordered_ids.as_slice()) {
            return false;
        }
    }

    let (ordered_ids, projected_start, dropped) =
        resolvable_queue_projection(&tracks, current_index);
    if dropped > 0 {
        log::info!(
            "[QConnect] Queue projection skipped {dropped} non-Qobuz track(s) before cloud sync"
        );
    }
    if ordered_ids.is_empty() {
        let mut guard = inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !installed_runtime_matches(&guard, stamp) {
            return false;
        }
        guard.last_pushed_queue_ids = Some(source_ordered_ids);
        return false;
    }

    let count = ordered_ids.len();
    let track_ids: Vec<i64> = ordered_ids.iter().map(|id| *id as i64).collect();
    let start_index = projected_start.unwrap_or(0);
    let payload = json!({
        "track_ids": track_ids,
        "queue_position": start_index,
        "shuffle_mode": false,
        "shuffle_pivot_index": start_index,
        "context_uuid": Uuid::new_v4().to_string(),
        "autoplay_reset": true,
        "autoplay_loading": false,
    });
    let command = app
        .build_queue_command(QueueCommandType::CtrlSrvrQueueLoadTracks, payload)
        .await;
    if !authority.is_current(stamp) {
        return false;
    }
    match app.send_queue_command(command).await {
        Ok(action_uuid) => {
            if !authority.is_current(stamp) {
                return false;
            }
            if force_takeover || takeover_retry {
                let mut state = sync_state.lock().await;
                if !authority.is_current(stamp) {
                    return false;
                }
                arm_local_queue_takeover(&mut state, ordered_ids, action_uuid);
                set_local_playback_conflict_pending(&mut state, false);
            }
            log::info!(
                "[QConnect] Pushed local queue to Connect ({count} tracks, start={start_index})"
            );
            if !authority.is_current(stamp) {
                return false;
            }
            let mut guard = inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !installed_runtime_matches(&guard, stamp) {
                return false;
            }
            guard.last_pushed_queue_ids = Some(source_ordered_ids);
            true
        }
        Err(err) if authority.is_current(stamp) => {
            log::warn!("[QConnect] Failed to push local queue: {err}");
            false
        }
        Err(_) => false,
    }
}

fn installed_runtime_matches(inner: &DaemonQconnectInner, stamp: AuthorityStamp) -> bool {
    inner.runtime.as_ref().map(|runtime| runtime.stamp) == Some(stamp)
}

fn capture_installed_stamp(
    inner: &StdMutex<DaemonQconnectInner>,
    authority: &AuthorityCell,
) -> Option<AuthorityStamp> {
    let stamp = inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .runtime
        .as_ref()
        .map(|runtime| runtime.stamp)?;
    authority.is_current(stamp).then_some(stamp)
}

/// The queue-publish subscriber: debounces `CoreEvent::QueueUpdated` bursts by
/// 2 s (same ritual as `daemon.rs::spawn_queue_persist`), then runs
/// [`publish_local_queue_if_changed`]. Non-queue events are drained WITHOUT
/// extending the debounce window, so they can never starve the publish. Holds
/// `Arc` clones of the qconnect inner + the runtime, so the handle is
/// aborted+joined in `QconnectHandle::shutdown` ahead of `drop(booted)` (the
/// #521 ordering), exactly like the report scheduler.
pub fn spawn_queue_cloud_publish(
    inner: Arc<StdMutex<DaemonQconnectInner>>,
    runtime: Arc<AppRuntime<DaemonAdapter>>,
    authority: Arc<AuthorityCell>,
    mut rx: broadcast::Receiver<CoreEvent>,
) -> JoinHandle<()> {
    use tokio::sync::broadcast::error::RecvError;
    const DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(2);
    tokio::spawn(async move {
        loop {
            // Block until the FIRST queue mutation of a burst.
            let mut event_stamp = match rx.recv().await {
                Ok(CoreEvent::QueueUpdated { .. }) => {
                    let Some(stamp) = capture_installed_stamp(&inner, &authority) else {
                        continue;
                    };
                    stamp
                }
                Ok(_) => continue,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            };
            // Debounce: a fixed deadline that only a further QueueUpdated extends.
            let mut deadline = tokio::time::Instant::now() + DEBOUNCE;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    r = rx.recv() => match r {
                        Ok(CoreEvent::QueueUpdated { .. }) => {
                            let Some(stamp) = capture_installed_stamp(&inner, &authority) else {
                                break;
                            };
                            event_stamp = stamp;
                            deadline = tokio::time::Instant::now() + DEBOUNCE;
                        }
                        Ok(_) => {}
                        Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => return,
                    }
                }
            }
            publish_local_queue_if_changed(&inner, &runtime, &authority, event_stamp).await;
        }
    })
}
