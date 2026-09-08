//! Shared QWS preflight state machine used by every QBZ runtime adapter.

use std::collections::VecDeque;

use qconnect_protocol::RendererCommandType;
use qconnect_transport_ws::TransportEvent;
use tokio::sync::broadcast;
use zeroize::Zeroizing;

use crate::{DelegationCancellation, DelegationErrorCode};

const PREFLIGHT_BUFFER_EVENTS: usize = 256;
const PREFLIGHT_BUFFER_BYTES: usize = 256 * 1024;

/// Owns the pre-commit receiver and the bounded replay buffer accumulated
/// while a candidate proves QWS readiness and session acceptance.
pub struct DelegationPreflight {
    receiver: broadcast::Receiver<TransportEvent>,
    buffered: VecDeque<TransportEvent>,
    buffered_bytes: usize,
    confirmed_owner_session_id: Option<Zeroizing<String>>,
}

impl DelegationPreflight {
    pub fn new(receiver: broadcast::Receiver<TransportEvent>) -> Self {
        Self {
            receiver,
            buffered: VecDeque::new(),
            buffered_bytes: 0,
            confirmed_owner_session_id: None,
        }
    }

    pub fn confirmed_owner_session_id(&self) -> Option<&str> {
        self.confirmed_owner_session_id
            .as_deref()
            .map(String::as_str)
    }

    /// Transfer the live receiver and replay-worthy events into the installed
    /// runtime after the authority commit succeeds.
    pub fn into_session_events(
        self,
    ) -> (
        broadcast::Receiver<TransportEvent>,
        VecDeque<TransportEvent>,
    ) {
        (self.receiver, self.buffered)
    }

    pub async fn wait_for_qws_ready(
        &mut self,
        cancellation: DelegationCancellation,
    ) -> Result<(), DelegationErrorCode> {
        let mut authenticated = false;
        let mut subscribed = false;
        while !(authenticated && subscribed) {
            let event = self.next_event(cancellation.clone()).await?;
            match &event {
                TransportEvent::Authenticated => authenticated = true,
                TransportEvent::Subscribed => subscribed = true,
                TransportEvent::Disconnected
                | TransportEvent::CloudError { .. }
                | TransportEvent::MaxReconnectAttemptsExceeded { .. } => {
                    return Err(DelegationErrorCode::QwsRejected)
                }
                _ => {}
            }
            self.retain(event)?;
        }
        Ok(())
    }

    pub async fn wait_for_activation(
        &mut self,
        cancellation: DelegationCancellation,
    ) -> Result<(), DelegationErrorCode> {
        loop {
            let event = self.next_event(cancellation.clone()).await?;
            let accepted = matches!(
                &event,
                TransportEvent::InboundRendererServerCommand(command)
                    if command.command_type == RendererCommandType::SrvrRndrSetActive
                        && command.payload.get("active").and_then(serde_json::Value::as_bool)
                            == Some(true)
            );
            if matches!(
                &event,
                TransportEvent::Disconnected
                    | TransportEvent::CloudError { .. }
                    | TransportEvent::MaxReconnectAttemptsExceeded { .. }
            ) {
                return Err(DelegationErrorCode::ActivationRejected);
            }
            self.retain(event)?;
            if accepted {
                return Ok(());
            }
        }
    }

    /// Owner preparation completes only after the cloud emits both the
    /// controller session id and the establishment proof used by live loops.
    pub async fn wait_for_owner_session(
        &mut self,
        cancellation: DelegationCancellation,
    ) -> Result<(), DelegationErrorCode> {
        let mut established = false;
        loop {
            let event = self.next_event(cancellation.clone()).await?;
            if let Some(session_id) = owner_session_id_from_event(&event).map(str::to_string) {
                self.confirmed_owner_session_id = Some(Zeroizing::new(session_id));
            }
            established |= matches!(&event, TransportEvent::SessionEstablished);
            if matches!(
                &event,
                TransportEvent::Disconnected
                    | TransportEvent::CloudError { .. }
                    | TransportEvent::MaxReconnectAttemptsExceeded { .. }
            ) {
                return Err(DelegationErrorCode::OwnerRestoreFailed);
            }
            self.retain(event)?;
            if established && self.confirmed_owner_session_id.is_some() {
                return Ok(());
            }
        }
    }

    async fn next_event(
        &mut self,
        mut cancellation: DelegationCancellation,
    ) -> Result<TransportEvent, DelegationErrorCode> {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(DelegationErrorCode::CandidateCancelled),
            event = self.receiver.recv() => match event {
                Ok(event) => Ok(event),
                Err(broadcast::error::RecvError::Lagged(_)) => Err(DelegationErrorCode::Internal),
                Err(broadcast::error::RecvError::Closed) => Err(DelegationErrorCode::QwsRejected),
            }
        }
    }

    fn retain(&mut self, event: TransportEvent) -> Result<(), DelegationErrorCode> {
        if !should_retain(&event) {
            return Ok(());
        }
        let bytes = retained_event_bytes(&event);
        if self.buffered.len() >= PREFLIGHT_BUFFER_EVENTS
            || self.buffered_bytes.saturating_add(bytes) > PREFLIGHT_BUFFER_BYTES
        {
            return Err(DelegationErrorCode::Internal);
        }
        self.buffered_bytes = self.buffered_bytes.saturating_add(bytes);
        self.buffered.push_back(event);
        Ok(())
    }
}

fn retained_event_bytes(event: &TransportEvent) -> usize {
    match event {
        TransportEvent::InboundPayloadBytes { payload, .. } => payload.len(),
        _ => 0,
    }
}

fn should_retain(event: &TransportEvent) -> bool {
    matches!(
        event,
        TransportEvent::Connected
            | TransportEvent::Disconnected
            | TransportEvent::SessionEstablished
            | TransportEvent::InboundPayloadBytes { .. }
            | TransportEvent::InboundQueueServerEvent(_)
            | TransportEvent::InboundRendererServerCommand(_)
            | TransportEvent::InboundReceived(_)
    )
}

fn owner_session_id_from_event(event: &TransportEvent) -> Option<&str> {
    match event {
        TransportEvent::InboundQueueServerEvent(event)
            if event.message_type() == "MESSAGE_TYPE_SRVR_CTRL_SESSION_STATE" =>
        {
            event
                .payload
                .get("session_uuid")
                .and_then(serde_json::Value::as_str)
                .filter(|session_id| !session_id.is_empty())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use qconnect_protocol::{
        QueueEventType, QueueServerEvent, RendererCommandType, RendererServerCommand,
    };
    use tokio::sync::watch;

    use super::*;

    fn cancellation(cancelled: bool) -> (watch::Sender<bool>, DelegationCancellation) {
        let (sender, receiver) = watch::channel(cancelled);
        (sender, DelegationCancellation::from_receiver(receiver))
    }

    #[tokio::test]
    async fn readiness_accepts_either_order_and_replays_only_runtime_events() {
        let (sender, receiver) = broadcast::channel(8);
        let mut preflight = DelegationPreflight::new(receiver);
        sender.send(TransportEvent::Subscribed).unwrap();
        sender.send(TransportEvent::Connected).unwrap();
        sender.send(TransportEvent::Authenticated).unwrap();

        let (_cancel_guard, cancellation) = cancellation(false);
        preflight.wait_for_qws_ready(cancellation).await.unwrap();
        let (_, buffered) = preflight.into_session_events();
        assert!(matches!(buffered.front(), Some(TransportEvent::Connected)));
        assert_eq!(buffered.len(), 1);
    }

    #[tokio::test]
    async fn activation_requires_active_true_and_retains_commands_in_order() {
        let (sender, receiver) = broadcast::channel(8);
        let mut preflight = DelegationPreflight::new(receiver);
        for active in [false, true] {
            sender
                .send(TransportEvent::InboundRendererServerCommand(
                    RendererServerCommand {
                        command_type: RendererCommandType::SrvrRndrSetActive,
                        payload: serde_json::json!({ "active": active }),
                    },
                ))
                .unwrap();
        }

        let (_cancel_guard, cancellation) = cancellation(false);
        preflight.wait_for_activation(cancellation).await.unwrap();
        let (_, buffered) = preflight.into_session_events();
        assert_eq!(buffered.len(), 2);
    }

    #[tokio::test]
    async fn owner_requires_session_state_and_establishment_in_any_order() {
        let (sender, receiver) = broadcast::channel(8);
        let mut preflight = DelegationPreflight::new(receiver);
        sender.send(TransportEvent::SessionEstablished).unwrap();
        sender
            .send(TransportEvent::InboundQueueServerEvent(QueueServerEvent {
                event_type: QueueEventType::SrvrCtrlSessionState,
                action_uuid: None,
                queue_version: None,
                payload: serde_json::json!({ "session_uuid": "owner-session" }),
            }))
            .unwrap();

        let (_cancel_guard, cancellation) = cancellation(false);
        preflight
            .wait_for_owner_session(cancellation)
            .await
            .unwrap();
        assert_eq!(
            preflight.confirmed_owner_session_id(),
            Some("owner-session")
        );
    }

    #[tokio::test]
    async fn cancellation_wins_without_waiting_for_transport() {
        let (_sender, receiver) = broadcast::channel(1);
        let mut preflight = DelegationPreflight::new(receiver);
        let (_cancel_guard, cancellation) = cancellation(true);
        assert_eq!(
            preflight.wait_for_qws_ready(cancellation).await,
            Err(DelegationErrorCode::CandidateCancelled)
        );
    }

    #[tokio::test]
    async fn cancellation_wins_when_a_transport_event_is_already_ready() {
        let (sender, receiver) = broadcast::channel(1);
        let mut preflight = DelegationPreflight::new(receiver);
        sender.send(TransportEvent::Authenticated).unwrap();
        let (_cancel_guard, cancellation) = cancellation(true);

        assert_eq!(
            preflight.wait_for_qws_ready(cancellation).await,
            Err(DelegationErrorCode::CandidateCancelled)
        );
    }

    #[test]
    fn replay_buffer_is_bounded_by_count_and_payload_bytes() {
        let (_sender, receiver) = broadcast::channel(1);
        let mut preflight = DelegationPreflight::new(receiver);
        for _ in 0..PREFLIGHT_BUFFER_EVENTS {
            preflight.retain(TransportEvent::Connected).unwrap();
        }
        assert_eq!(
            preflight.retain(TransportEvent::Connected),
            Err(DelegationErrorCode::Internal)
        );

        let (_sender, receiver) = broadcast::channel(1);
        let mut preflight = DelegationPreflight::new(receiver);
        assert_eq!(
            preflight.retain(TransportEvent::InboundPayloadBytes {
                cloud_message_type: 6,
                payload: vec![0; PREFLIGHT_BUFFER_BYTES + 1],
            }),
            Err(DelegationErrorCode::Internal)
        );
    }
}
