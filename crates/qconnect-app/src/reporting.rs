//! Shared renderer playback-report projection.
//!
//! Frontend adapters provide a player snapshot and queue-item resolution. This
//! module alone owns the QConnect buffer mapping and JSON payload shape.

use qbz_player::player::{PlaybackBufferState, PlaybackEvent};
use qconnect_core::QueueVersion;
use qconnect_protocol::{RendererBufferState, RendererReport, RendererReportType};

use crate::renderer::{PLAYING_STATE_PAUSED, PLAYING_STATE_PLAYING};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RendererPlaybackSnapshot {
    pub playing_state: i32,
    pub buffer_state: PlaybackBufferState,
    pub position_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub current_queue_item_id: Option<u64>,
    pub next_queue_item_id: Option<u64>,
}

pub const fn renderer_buffer_state(state: PlaybackBufferState) -> RendererBufferState {
    match state {
        PlaybackBufferState::Idle | PlaybackBufferState::Ready => RendererBufferState::Ok,
        PlaybackBufferState::InitialBuffering => RendererBufferState::Buffering,
        PlaybackBufferState::Underrun => RendererBufferState::Underrun,
        PlaybackBufferState::Error => RendererBufferState::Error,
    }
}

/// Project player state onto the official renderer intent semantics.
///
/// During initial buffering the audio device has not started yet, so
/// `PlaybackEvent::is_playing` is false. Official clients nevertheless retain
/// the requested PLAYING intent alongside BUFFERING. An underrun likewise
/// remains PLAYING while audio is temporarily starved.
pub const fn renderer_playing_state(is_playing: bool, buffer_state: PlaybackBufferState) -> i32 {
    if matches!(
        buffer_state,
        PlaybackBufferState::InitialBuffering | PlaybackBufferState::Underrun
    ) || is_playing
    {
        PLAYING_STATE_PLAYING
    } else {
        PLAYING_STATE_PAUSED
    }
}

/// Pick the identity atomically paired with buffer state while a play is
/// loading. Once idle, the audible/current track remains authoritative.
pub const fn qconnect_report_track_id(event: &PlaybackEvent) -> u64 {
    if !matches!(event.buffer_state, PlaybackBufferState::Idle) && event.buffer_track_id != 0 {
        event.buffer_track_id
    } else {
        event.track_id
    }
}

pub fn build_renderer_playback_report(
    action_uuid: impl Into<String>,
    queue_version: QueueVersion,
    snapshot: RendererPlaybackSnapshot,
) -> RendererReport {
    RendererReport::new(
        RendererReportType::RndrSrvrStateUpdated,
        action_uuid,
        queue_version,
        serde_json::json!({
            "playing_state": snapshot.playing_state,
            "buffer_state": renderer_buffer_state(snapshot.buffer_state).as_i32(),
            "current_position": snapshot.position_ms,
            "duration": snapshot.duration_ms,
            "current_queue_item_id": snapshot.current_queue_item_id,
            "next_queue_item_id": snapshot.next_queue_item_id,
            "queue_version": {
                "major": queue_version.major,
                "minor": queue_version.minor
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_buffer_states_map_to_official_wire_values() {
        assert_eq!(renderer_buffer_state(PlaybackBufferState::Idle).as_i32(), 2);
        assert_eq!(
            renderer_buffer_state(PlaybackBufferState::InitialBuffering).as_i32(),
            1
        );
        assert_eq!(
            renderer_buffer_state(PlaybackBufferState::Ready).as_i32(),
            2
        );
        assert_eq!(
            renderer_buffer_state(PlaybackBufferState::Underrun).as_i32(),
            4
        );
        assert_eq!(
            renderer_buffer_state(PlaybackBufferState::Error).as_i32(),
            3
        );
    }

    #[test]
    fn buffering_and_underrun_preserve_playing_intent() {
        assert_eq!(
            renderer_playing_state(false, PlaybackBufferState::InitialBuffering),
            PLAYING_STATE_PLAYING
        );
        assert_eq!(
            renderer_playing_state(false, PlaybackBufferState::Underrun),
            PLAYING_STATE_PLAYING
        );
        assert_eq!(
            renderer_playing_state(false, PlaybackBufferState::Ready),
            PLAYING_STATE_PAUSED
        );
        assert_eq!(
            renderer_playing_state(true, PlaybackBufferState::Ready),
            PLAYING_STATE_PLAYING
        );
    }

    #[test]
    fn playback_report_builder_owns_the_wire_shape() {
        let version = QueueVersion { major: 7, minor: 3 };
        let report = build_renderer_playback_report(
            "action",
            version,
            RendererPlaybackSnapshot {
                playing_state: 2,
                buffer_state: PlaybackBufferState::Underrun,
                position_ms: Some(118_000),
                duration_ms: Some(317_000),
                current_queue_item_id: Some(4),
                next_queue_item_id: Some(5),
            },
        );

        assert_eq!(report.report_type, RendererReportType::RndrSrvrStateUpdated);
        assert_eq!(report.payload["buffer_state"], 4);
        assert_eq!(report.payload["current_position"], 118_000);
        assert_eq!(report.payload["duration"], 317_000);
        assert_eq!(report.payload["queue_version"]["major"], 7);
        assert_eq!(report.payload["queue_version"]["minor"], 3);
    }
}
