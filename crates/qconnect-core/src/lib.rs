//! qconnect-core
//!
//! UI-agnostic queue domain primitives for Qobuz Connect.

pub mod admission;
pub mod pending;
pub mod queue;
pub mod reducer;
pub mod renderer;
pub mod telemetry;

pub use admission::{
    evaluate_remote_queue_admission, resolve_handoff_intent, validate_track_origins_for_admission,
    AdmissionDecision, HandoffIntent, TrackOrigin,
};
pub use pending::{PendingActionError, PendingActionSlot, PendingCorrelation, PendingQueueAction};
pub use queue::{QConnectQueueState, QueueEvent, QueueItem, QueueVersion};
pub use reducer::{apply_event, build_shuffle_order, ReducerOutcome};
pub use renderer::{apply_renderer_command, QConnectRendererState, RendererCommand};
