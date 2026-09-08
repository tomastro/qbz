// TODO(converge: qconnect-glue) — copied from crates/qbz/src/qconnect_service.rs @ 5d50158e;
// do not fix bugs here without fixing the source, and vice versa.
//
//! Renderer playback-state report (the UI-free body of the desktop
//! `report_playback_state`, qconnect_service.rs:592).
//!
//! Daemon adaptation vs. the Slint copy (§1.4): the desktop `report_playback_state`
//! is a method on `SlintQconnectService` driven by the Slint playback POLL LOOP;
//! here it is a free function the T10 report tick calls on a tokio interval. No
//! behavior change — it still self-gates on `is_local_renderer_active`, resolves
//! current/next queue_item_id from the playing track, sends a
//! `RndrSrvrStateUpdated`, keeps the app's renderer position in sync, and reports
//! the live output format for the controller's quality badge. `position_ms` /
//! `duration_ms` are MILLISECONDS (the QConnect protocol unit).
#![allow(dead_code)]

use std::sync::{Arc, Mutex as StdMutex};

use qbz_app::shell::AppRuntime;
use qbz_player::player::PlaybackBufferState;
use qconnect_app::{
    build_renderer_playback_report, confirm_local_playback_state_asserted,
    is_local_renderer_active, qconnect_report_track_id, renderer_playing_state,
    QconnectFileAudioQualitySnapshot, QconnectRemoteSyncState, RendererPlaybackSnapshot,
};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::authority::{AuthorityCell, AuthorityStamp};
use super::sink::DaemonQconnectApp;
use crate::adapter::DaemonAdapter;

pub const QCONNECT_RENDERER_CHANNELS: i32 = 2;
const AUDIO_QUALITY_UNKNOWN: i32 = 0;
const AUDIO_QUALITY_MP3: i32 = 1;
const AUDIO_QUALITY_CD: i32 = 2;
const AUDIO_QUALITY_HIRES_L1: i32 = 3;
const AUDIO_QUALITY_HIRES_L2: i32 = 4;
const AUDIO_QUALITY_HIRES_L3: i32 = 5;

/// Report this device's playback state to the cloud while the daemon is the
/// ACTIVE LOCAL renderer. Self-gates on `is_local_renderer_active` (no-op when a
/// PEER owns playback), resolves the current/next queue_item_id from the playing
/// track, sends a `RndrSrvrStateUpdated`, and keeps the app's renderer position
/// in sync.
pub async fn report_playback_state(
    app: &Arc<DaemonQconnectApp>,
    sync_state: &Arc<Mutex<QconnectRemoteSyncState>>,
    runtime: &Arc<AppRuntime<DaemonAdapter>>,
    authority: &AuthorityCell,
    stamp: AuthorityStamp,
    playing_state: i32,
    position_ms: i64,
    duration_ms: i64,
    track_id: u64,
    buffer_state: PlaybackBufferState,
) {
    if !authority.is_current(stamp) {
        return;
    }
    // Only report when WE are the active renderer. When a peer renderer owns
    // playback (the daemon is acting as a controller) the renderer reports come
    // from the peer, not us.
    {
        let state = sync_state.lock().await;
        if !authority.is_current(stamp) {
            return;
        }
        if !is_local_renderer_active(&state.session) {
            return;
        }
    }

    let (current_qid, next_qid) =
        resolve_queue_item_ids_by_track_id(app, sync_state, authority, stamp, track_id).await;
    if !authority.is_current(stamp) {
        return;
    }
    let queue_version = app.queue_state_snapshot().await.version;
    if !authority.is_current(stamp) {
        return;
    }

    let report = build_renderer_playback_report(
        Uuid::new_v4().to_string(),
        queue_version,
        RendererPlaybackSnapshot {
            playing_state,
            buffer_state,
            position_ms: Some(position_ms),
            duration_ms: Some(duration_ms),
            current_queue_item_id: current_qid,
            next_queue_item_id: next_qid,
        },
    );
    if !authority.is_current(stamp) {
        return;
    }
    match app.send_renderer_report_command(report).await {
        Ok(()) if authority.is_current(stamp) => {
            let mut state = sync_state.lock().await;
            if authority.is_current(stamp) {
                confirm_local_playback_state_asserted(&mut state);
            }
        }
        Ok(()) => return,
        Err(err) => {
            if !authority.is_current(stamp) {
                return;
            }
            log::warn!("[QConnect] Failed to report playback state: {err}");
        }
    }

    if position_ms >= 0 && authority.is_current(stamp) {
        app.update_renderer_position(position_ms as u64).await;
    }
    if !authority.is_current(stamp) {
        return;
    }

    // Report the live output format so the controller shows the correct quality
    // badge (CD / Hi-Res). Reads the player's current output (sample_rate/
    // bit_depth); channels default to stereo. Both reports dedup internally in
    // qconnect-app, so calling them every report tick is cheap.
    let player = runtime.core().player();
    let sample_rate = player.state.get_sample_rate();
    let bit_depth = player.state.get_bit_depth();
    if let Some(snapshot) =
        build_file_audio_quality_snapshot(sample_rate, bit_depth, QCONNECT_RENDERER_CHANNELS)
    {
        if !authority.is_current(stamp) {
            return;
        }
        if let Err(err) = app
            .report_file_audio_quality_if_changed(queue_version, snapshot)
            .await
        {
            if !authority.is_current(stamp) {
                return;
            }
            log::warn!("[QConnect] Failed to report file audio quality: {err}");
        }
        if !authority.is_current(stamp) {
            return;
        }
        if let Err(err) = app
            .report_device_audio_quality_if_changed(
                queue_version,
                snapshot.sampling_rate,
                snapshot.bit_depth,
                snapshot.nb_channels,
            )
            .await
        {
            if !authority.is_current(stamp) {
                return;
            }
            log::warn!("[QConnect] Failed to report device audio quality: {err}");
        }
    }
}

/// Classify a (sample_rate, bit_depth) output into the QConnect AudioQuality
/// level. Pure mirror of the Tauri `classify_qconnect_audio_quality`.
fn classify_audio_quality(sample_rate: u32, bit_depth: u32) -> i32 {
    if sample_rate == 0 || bit_depth == 0 {
        AUDIO_QUALITY_UNKNOWN
    } else if sample_rate >= 384_000 {
        AUDIO_QUALITY_HIRES_L3
    } else if sample_rate >= 192_000 {
        AUDIO_QUALITY_HIRES_L2
    } else if bit_depth > 16 || sample_rate > 48_000 {
        AUDIO_QUALITY_HIRES_L1
    } else if sample_rate >= 44_100 {
        AUDIO_QUALITY_CD
    } else {
        AUDIO_QUALITY_MP3
    }
}

/// Build a file-audio-quality snapshot from the live output format, or None when
/// the format isn't known yet. Pure mirror of the Tauri
/// `build_qconnect_file_audio_quality_snapshot`.
fn build_file_audio_quality_snapshot(
    sample_rate: u32,
    bit_depth: u32,
    nb_channels: i32,
) -> Option<QconnectFileAudioQualitySnapshot> {
    if sample_rate == 0 || bit_depth == 0 {
        return None;
    }
    Some(QconnectFileAudioQualitySnapshot {
        sampling_rate: sample_rate as i32,
        bit_depth: bit_depth as i32,
        nb_channels,
        audio_quality: classify_audio_quality(sample_rate, bit_depth),
    })
}

/// Resolve the current + next `queue_item_id` for a playing `track_id` from the
/// cloud queue snapshot, caching the result into the sync accumulator. Mirrors
/// the Tauri `resolve_queue_item_ids_by_track_id`.
async fn resolve_queue_item_ids_by_track_id(
    app: &Arc<DaemonQconnectApp>,
    sync_state: &Arc<Mutex<QconnectRemoteSyncState>>,
    authority: &AuthorityCell,
    stamp: AuthorityStamp,
    track_id: u64,
) -> (Option<u64>, Option<u64>) {
    if !authority.is_current(stamp) {
        return (None, None);
    }
    let queue = app.queue_state_snapshot().await;
    if !authority.is_current(stamp) {
        return (None, None);
    }
    let (current_qid, next_qid, next_track_id) =
        qconnect_app::queue_resolution::resolve_queue_item_ids_from_queue_state(&queue, track_id);

    if let Some(current_qid) = current_qid {
        let mut state = sync_state.lock().await;
        if !authority.is_current(stamp) {
            return (None, None);
        }
        state.last_renderer_queue_item_id = Some(current_qid);
        state.last_renderer_next_queue_item_id = next_qid;
        state.last_renderer_track_id = Some(track_id);
        state.last_renderer_next_track_id = next_track_id;
        (Some(current_qid), next_qid)
    } else {
        (None, None)
    }
}

// T10 (§7.2, §3.1-7): the report-tick scheduler. The desktop reports from its
// 450 ms Slint poll loop; the daemon has no such loop, so a dedicated tokio task
// owns the cadence. It calls `report_playback_state` on the LIVE session (a no-op
// when not connected or when a peer owns playback, since the body self-gates on
// `is_local_renderer_active`).
//
// Two triggers, per §7.2 ("~2 s tokio interval while playing + edge-triggered on
// track/play-state transitions"):
//   * `notify` — the driver's `DriverAction::ReportEdge` signal (daemon.rs wires
//     `on_edge -> Notify::notify_one`). The landed T4 driver folds the ~2 s
//     periodic cadence AND the transition edges into this one signal
//     (playback.rs:4648, `transition || periodic`).
//   * a ~2 s `interval` — the periodic FLOOR of §3.1-7. Because the driver
//     already supplies the periodic edge, the interval is RESET on every wake so
//     it only elapses when the edge stream goes quiet (no double-reporting during
//     active playback); interval-driven reports are additionally gated on
//     `is_playing`, so a paused/stopped renderer stays silent like the desktop.
pub async fn run_report_scheduler(
    notify: Arc<tokio::sync::Notify>,
    inner: Arc<StdMutex<super::DaemonQconnectInner>>,
    runtime: Arc<AppRuntime<DaemonAdapter>>,
    authority: Arc<AuthorityCell>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(2_000));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let via_interval = tokio::select! {
            _ = notify.notified() => false,
            _ = interval.tick() => true,
        };
        // Reset so the periodic floor only fires after 2 s of edge silence.
        interval.reset();

        // Capture one exact installed runtime before reading player state. If a
        // handoff lands while the snapshot is read, the stamp check below drops
        // the mixed-authority sample.
        let (app, sync_state, stamp) = {
            let guard = inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match guard.runtime.as_ref() {
                Some(rt) => (Arc::clone(&rt.app), Arc::clone(&rt.sync_state), rt.stamp),
                _ => continue,
            }
        };
        if !authority.is_current(stamp) {
            continue;
        }

        // Read the live player state. Nothing loaded -> nothing to report.
        let ev = runtime.core().player().get_playback_event();
        if !authority.is_current(stamp) {
            continue;
        }
        let report_track_id = qconnect_report_track_id(&ev);
        if report_track_id == 0 {
            continue;
        }
        // The periodic floor only fires while actually playing; edge notifications
        // (transitions + the driver's periodic) always report.
        if via_interval
            && !ev.is_playing
            && matches!(
                ev.buffer_state,
                PlaybackBufferState::Idle | PlaybackBufferState::Ready
            )
        {
            continue;
        }

        let playing_state = renderer_playing_state(ev.is_playing, ev.buffer_state);
        // `report_playback_state` wants MILLISECONDS; the player reports seconds.
        let position_ms = (ev.position as i64) * 1000;
        let duration_ms = (ev.duration as i64) * 1000;
        report_playback_state(
            &app,
            &sync_state,
            &runtime,
            &authority,
            stamp,
            playing_state,
            position_ms,
            duration_ms,
            report_track_id,
            ev.buffer_state,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_audio_quality_matches_the_desktop_thresholds() {
        assert_eq!(classify_audio_quality(0, 0), AUDIO_QUALITY_UNKNOWN);
        assert_eq!(classify_audio_quality(44_100, 16), AUDIO_QUALITY_CD);
        assert_eq!(classify_audio_quality(48_000, 16), AUDIO_QUALITY_CD);
        assert_eq!(classify_audio_quality(96_000, 24), AUDIO_QUALITY_HIRES_L1);
        assert_eq!(classify_audio_quality(192_000, 24), AUDIO_QUALITY_HIRES_L2);
        assert_eq!(classify_audio_quality(384_000, 24), AUDIO_QUALITY_HIRES_L3);
        assert_eq!(classify_audio_quality(22_050, 16), AUDIO_QUALITY_MP3);
    }

    #[test]
    fn snapshot_is_none_until_format_known() {
        assert!(build_file_audio_quality_snapshot(0, 0, 2).is_none());
        let snap = build_file_audio_quality_snapshot(96_000, 24, 2).expect("known format");
        assert_eq!(snap.sampling_rate, 96_000);
        assert_eq!(snap.bit_depth, 24);
        assert_eq!(snap.nb_channels, 2);
        assert_eq!(snap.audio_quality, AUDIO_QUALITY_HIRES_L1);
    }
}
