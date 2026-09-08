use qconnect_core::{PendingActionSlot, QConnectQueueState, QConnectRendererState};

#[derive(Debug, Default)]
pub struct QconnectRuntimeState {
    pub queue: QConnectQueueState,
    pub renderer: QConnectRendererState,
    pub pending: PendingActionSlot,
    pub transport_connected: bool,
    pub concurrency_canceled_action_uuid: Option<String>,
}
