pub const EVENT_PENDING_ACTION_STARTED: &str = "pending_action_started";
pub const EVENT_PENDING_ACTION_COMPLETED: &str = "pending_action_completed";
pub const EVENT_PENDING_ACTION_TIMEOUT: &str = "pending_action_timeout";
pub const EVENT_PENDING_ACTION_CANCELED_BY_CONCURRENT_REMOTE_EVENT: &str =
    "pending_action_canceled_by_concurrent_remote_event";
pub const EVENT_QUEUE_EVENT_UUID_MATCH: &str = "queue_event_uuid_match";
pub const EVENT_QUEUE_EVENT_UUID_MISMATCH: &str = "queue_event_uuid_mismatch";
pub const EVENT_QUEUE_RESYNC_TRIGGERED: &str = "queue_resync_triggered";

pub fn queue_reducer_event_name(event_name: &str) -> String {
    format!("queue_reducer_applied_{event_name}")
}
