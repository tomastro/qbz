use super::*;
use crate::build_effective_renderer_snapshot;
use crate::queue_resolution::{
    resolve_controller_queue_item_from_snapshots, QconnectRemoteSkipDirection,
};
use prost::Message;
use qconnect_core::{QConnectRendererState, QueueItem};
use qconnect_protocol::{decode_queue_server_events, QConnectMessage, QConnectMessages};

// Official proto3-shaped controller frame: qid 0 is NOT on the wire.
// It is a valid current item, not an incomplete/position-only update.
fn player_event(qid: Option<i32>, playing: i32, position: Option<i32>) -> QueueServerEvent {
    let mut message = QConnectMessage::decode(
        &[
            0x08, 0x52, 0x92, 0x05, 0x08, 0x08, 0x02, 0x1a, 0x04, 0x08, 0x02, 0x1a, 0x00,
        ][..],
    )
    .unwrap();
    let player = message
        .srvr_ctrl_renderer_state_updated
        .as_mut()
        .unwrap()
        .player_state
        .as_mut()
        .unwrap();
    player.current_queue_item_id = qid;
    player.playing_state = Some(playing);
    player.current_position.as_mut().unwrap().value = position;
    let batch = QConnectMessages {
        messages: vec![message],
        ..Default::default()
    };
    decode_queue_server_events(&batch.encode_to_vec())
        .unwrap()
        .remove(0)
}

async fn apply_player(app: &QconnectApp<InMemoryWsTransport, TestSink>, event: QueueServerEvent) {
    app.apply_session_management_event(
        event.event_type.as_message_type(),
        &event.payload,
        &local_identity("local-uuid"),
    )
    .await;
}

async fn takeover(app: &QconnectApp<InMemoryWsTransport, TestSink>, shuffle: bool) {
    seed_local_and_active_peer(app).await;
    app.sync_handle().lock().await.session.active_renderer_id = Some(1);
    {
        let state = app.state_handle();
        let mut state = state.lock().await;
        state.queue = QConnectQueueState {
            version: QueueVersion::new(2, 2),
            queue_items: (0..3)
                .map(|qid| QueueItem {
                    queue_item_id: qid,
                    track_id: 100 + qid,
                    track_context_uuid: String::new(),
                })
                .collect(),
            shuffle_mode: shuffle,
            shuffle_order: shuffle.then(|| vec![0, 2, 1]),
            ..Default::default()
        };
    }
    app.apply_session_management_event(
        "MESSAGE_TYPE_SRVR_CTRL_ACTIVE_RENDERER_CHANGED",
        &json!({"active_renderer_id": 2}),
        &local_identity("local-uuid"),
    )
    .await;
}

async fn snapshot(app: &QconnectApp<InMemoryWsTransport, TestSink>) -> QConnectRendererState {
    let queue = app.queue_state_snapshot().await;
    let state = app.sync_handle();
    let state = state.lock().await;
    build_effective_renderer_snapshot(
        &queue,
        &QConnectRendererState::default(),
        state.session_renderer_states.get(&2),
        state.session_loop_mode,
    )
}

#[tokio::test]
async fn next_and_previous_after_paused_or_playing_takeover_with_zero_item() {
    for playing in [2, 3] {
        for shuffle in [false, true] {
            let (app, _sink, _transport, _rx) = build_connected_app().await;
            takeover(&app, shuffle).await;
            apply_player(&app, player_event(None, playing, Some(33_000))).await;
            let renderer = snapshot(&app).await;
            assert_eq!(renderer.current_track.as_ref().unwrap().queue_item_id, 0);
            let queue = app.queue_state_snapshot().await;
            for (direction, target) in [
                (
                    QconnectRemoteSkipDirection::Next,
                    if shuffle { 2 } else { 1 },
                ),
                (QconnectRemoteSkipDirection::Previous, 0),
            ] {
                let result =
                    resolve_controller_queue_item_from_snapshots(&queue, &renderer, direction);
                assert_eq!(result.target_queue_item_id, Some(target));
            }
            app.disconnect().await.unwrap();
        }
    }
}

#[tokio::test]
async fn returning_to_item_zero_and_position_zero_replaces_old_peer_cursor() {
    let (app, _sink, _transport, _rx) = build_connected_app().await;
    takeover(&app, false).await;
    apply_player(&app, player_event(Some(2), 2, Some(50_000))).await;
    apply_player(&app, player_event(None, 3, None)).await;
    let renderer = snapshot(&app).await;
    assert_eq!(renderer.current_track.unwrap().queue_item_id, 0);
    assert_eq!(renderer.current_position_ms, Some(0));
    assert_eq!(renderer.playing_state, Some(3));
    app.disconnect().await.unwrap();
}

#[tokio::test]
async fn takeover_controls_send_while_queue_read_is_pending() {
    let (app, _sink, transport, _rx) = build_connected_app().await;
    takeover(&app, true).await;
    apply_player(&app, player_event(None, 2, Some(33_000))).await;
    app.trigger_queue_state_resync().await;
    let read_uuid = app
        .state_handle()
        .lock()
        .await
        .pending
        .current()
        .unwrap()
        .uuid
        .clone();
    for payload in [
        json!({"playing_state": 3}),
        json!({"playing_state": 2}),
        json!({"current_position": 0}),
        json!({"playing_state": 2, "current_position": 0,
            "current_queue_item": {"id": 0, "queue_version": {"major": 2, "minor": 2}}}),
        json!({"playing_state": 2, "current_position": 0,
            "current_queue_item": {"id": 2, "queue_version": {"major": 2, "minor": 2}}}),
    ] {
        let command = app
            .build_queue_command(QueueCommandType::CtrlSrvrSetPlayerState, payload.clone())
            .await;
        let bytes = qconnect_protocol::encode_queue_command_batch(&command).unwrap();
        let wire = QConnectMessages::decode(bytes.as_slice()).unwrap();
        let sent = wire.messages[0]
            .ctrl_srvr_set_player_state
            .as_ref()
            .unwrap();
        assert_eq!(
            sent.playing_state,
            payload["playing_state"].as_i64().map(|v| v as i32)
        );
        assert_eq!(
            sent.current_position,
            payload["current_position"].as_i64().map(|v| v as i32)
        );
        assert_eq!(
            sent.current_queue_item.as_ref().and_then(|item| item.id),
            payload["current_queue_item"]["id"]
                .as_i64()
                .map(|v| v as i32)
        );
        app.send_queue_command(command).await.unwrap();
        assert_eq!(
            app.state_handle()
                .lock()
                .await
                .pending
                .current()
                .unwrap()
                .uuid,
            read_uuid
        );
    }
    assert_eq!(transport.sent_messages().await.len(), 6);
    app.disconnect().await.unwrap();
}

#[tokio::test]
async fn takeback_keeps_renderer_caches_separate_and_honors_no_track_sentinel() {
    let (app, _sink, _transport, _rx) = build_connected_app().await;
    takeover(&app, false).await;
    apply_player(&app, player_event(Some(2), 2, Some(10_000))).await;
    app.apply_session_management_event(
        "MESSAGE_TYPE_SRVR_CTRL_ACTIVE_RENDERER_CHANGED",
        &json!({"active_renderer_id": 1}),
        &local_identity("local-uuid"),
    )
    .await;
    apply_player(&app, player_event(None, 3, None)).await;
    {
        let state = app.sync_handle();
        let state = state.lock().await;
        assert_eq!(state.session.active_renderer_id, Some(1));
        assert_eq!(
            state.session_renderer_states[&2].current_queue_item_id,
            Some(0)
        );
        assert!(!crate::is_peer_renderer_active(&state.session));
    }
    apply_player(&app, player_event(Some(-1), 1, None)).await;
    assert_eq!(
        app.sync_handle().lock().await.session_renderer_states[&2].current_queue_item_id,
        None
    );
    app.disconnect().await.unwrap();
}

#[tokio::test]
async fn mute_and_unmute_do_not_strand_pending_but_still_respect_queue_writes() {
    let (app, _sink, transport, _rx) = build_connected_app().await;
    takeover(&app, false).await;
    for muted in [true, false, true] {
        let command = app
            .build_queue_command(
                QueueCommandType::CtrlSrvrMuteVolume,
                json!({"renderer_id": 2, "value": muted}),
            )
            .await;
        app.send_queue_command(command).await.unwrap();
        assert!(app.state_handle().lock().await.pending.current().is_none());
    }
    let write = app
        .build_queue_command(
            QueueCommandType::CtrlSrvrQueueLoadTracks,
            json!({"track_ids": [100, 101]}),
        )
        .await;
    let write_uuid = app.send_queue_command(write).await.unwrap();
    let mute = app
        .build_queue_command(
            QueueCommandType::CtrlSrvrMuteVolume,
            json!({"renderer_id": 2, "value": false}),
        )
        .await;
    assert!(app.send_queue_command(mute).await.is_err());
    assert_eq!(
        app.state_handle()
            .lock()
            .await
            .pending
            .current()
            .unwrap()
            .uuid,
        write_uuid
    );
    assert_eq!(transport.sent_messages().await.len(), 4);
    app.disconnect().await.unwrap();
}
