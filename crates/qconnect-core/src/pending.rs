use crate::queue::QueueVersion;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingQueueAction {
    pub uuid: String,
    pub queue_version_ref: QueueVersion,
    pub emit_result_event: bool,
    pub is_ask_for_state_action: bool,
    #[serde(default)]
    pub is_transport_control_action: bool,
    #[serde(default)]
    pub is_set_loop_mode_action: bool,
    #[serde(default)]
    pub is_set_volume_action: bool,
    #[serde(default)]
    pub is_set_active_renderer_action: bool,
    #[serde(default)]
    pub is_queue_load_tracks_action: bool,
    #[serde(default)]
    pub expected_active_renderer_id: Option<i32>,
    pub concurrency_error: bool,
    pub sent_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingCorrelation {
    NoPending,
    EventWithoutActionUuid,
    Matched,
    Concurrent,
}

#[derive(Debug, Error)]
pub enum PendingActionError {
    #[error("pending queue action already active: {0}")]
    AlreadyPending(String),
}

#[derive(Debug, Default)]
pub struct PendingActionSlot {
    current: Option<PendingQueueAction>,
}

impl PendingActionSlot {
    pub fn start(&mut self, action: PendingQueueAction) -> Result<(), PendingActionError> {
        if let Some(existing) = &self.current {
            return Err(PendingActionError::AlreadyPending(existing.uuid.clone()));
        }
        self.current = Some(action);
        Ok(())
    }

    pub fn current(&self) -> Option<&PendingQueueAction> {
        self.current.as_ref()
    }

    pub fn current_mut(&mut self) -> Option<&mut PendingQueueAction> {
        self.current.as_mut()
    }

    pub fn correlate(&self, event_action_uuid: Option<&str>) -> PendingCorrelation {
        let Some(pending) = &self.current else {
            return PendingCorrelation::NoPending;
        };
        let Some(event_uuid) = event_action_uuid else {
            return PendingCorrelation::EventWithoutActionUuid;
        };

        if pending.uuid == event_uuid {
            PendingCorrelation::Matched
        } else {
            PendingCorrelation::Concurrent
        }
    }

    pub fn mark_concurrency_error(&mut self) -> bool {
        if let Some(current) = &mut self.current {
            current.concurrency_error = true;
            return true;
        }
        false
    }

    pub fn clear(&mut self) -> Option<PendingQueueAction> {
        self.current.take()
    }
}
