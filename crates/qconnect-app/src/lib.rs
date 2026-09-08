//! qconnect-app
//!
//! Application adapter that composes qconnect core + protocol + transport.

mod app;
mod authority;
mod authority_transition;
mod delegated_rejoin;
mod delegation;
mod delegation_preflight;
mod error;
mod events;
mod feature_flags;
mod lan_projection;
mod lan_runtime;
mod playback_conflict;
pub mod queue_resolution;
pub mod renderer;
mod renderer_engine;
mod reporting;
pub mod session;
pub mod startup;
mod state;
mod sync_state;

pub use app::{
    queue_payload_track_preview, LocalPlaybackConflictChoice, QconnectApp, SessionApplyOutcome,
    SessionLoopHost, SessionStateTakeoverInput,
};
pub use authority::{
    AuthorityActionPermit, AuthorityCell, AuthorityOrigin, AuthorityStamp,
    ExactOwnerAuthorityObservation, OwnerAuthorityObservation, OwnerAuthorityToken,
    QconnectDisabledToken, QconnectEnableIntent, QconnectEnableToken,
};
pub use authority_transition::{
    acquire_transition_guard_and_fence, DeferredActivationRelease, OwnerActionFence,
};
pub use delegated_rejoin::{
    DelegatedRejoinWatchdog, DelegatedRuntimeEventDirective, DelegatedRuntimeEventState,
};
pub use delegation::{
    CommitRejected, CredentialOrigin, DelegationCancellation, DelegationCandidate,
    DelegationCoordinator, DelegationCoordinatorConfig, DelegationCoordinatorError,
    DelegationErrorCode, DelegationHost, DelegationPhase, DelegationSnapshot, RestoreReason,
};
pub use delegation_preflight::DelegationPreflight;
pub use error::{QconnectAppError, QconnectOwnerFailure};
pub use events::{NoOpEventSink, QconnectAppEvent, QconnectEventSink};
pub use feature_flags::{
    QBZ_QCONNECT_PANEL_SWITCH, QBZ_QCONNECT_QUEUE_MODEL, QBZ_QCONNECT_STRICT_DOMAIN_ISOLATION,
    QBZ_QCONNECT_TRANSPORT,
};
pub use lan_projection::LanProjectionSlot;
pub use lan_runtime::{lan_callback_is_current, LanRuntimeError, LanRuntimeLifecycle};
pub use playback_conflict::QconnectPlaybackConflictPolicy;
pub use qconnect_core::{
    evaluate_remote_queue_admission, resolve_handoff_intent, validate_track_origins_for_admission,
    AdmissionDecision, HandoffIntent, QConnectQueueState, QConnectRendererState, QueueVersion,
    RendererCommand, TrackOrigin,
};
pub use qconnect_protocol::{
    QueueCommandType, RendererBufferState, RendererReport, RendererReportType,
};
pub use renderer::{qconnect_queue_track_is_resolvable, qconnect_source_is_resolvable};
pub use renderer_engine::QconnectRendererEngine;
pub use reporting::{
    build_renderer_playback_report, qconnect_report_track_id, renderer_buffer_state,
    renderer_playing_state, RendererPlaybackSnapshot,
};
pub use session::{
    build_effective_renderer_snapshot, build_session_renderer_snapshot, compute_connection_state,
    deferred_join_reason, find_unique_renderer_id, is_local_renderer_active,
    is_peer_renderer_active, max_audio_quality_from_quality, normalize_active_renderer_id,
    qconnect_millis_from_secs, quality_from_max_audio_quality, queue_item_snapshot_for_cursor,
    refresh_local_renderer_id, renderer_allows_remote_volume, should_arm_renderer_watchdog,
    should_reask_queue_state, ConnectionDecision, LocalIdentity, QconnectFileAudioQualitySnapshot,
    QconnectLifecycleState, QconnectRendererInfo, QconnectSessionRendererState,
    QconnectSessionState, RendererStatus, ServerActiveState,
    JOIN_SESSION_REASON_CONTROLLER_REQUEST, JOIN_SESSION_REASON_RECONNECTION,
    QCONNECT_RENDERER_LOST_TIMEOUT_MS,
};
pub use startup::{compute_effective_startup, QconnectStartupMode};
pub use state::QconnectRuntimeState;
pub use sync_state::{
    active_peer_renderer_is_playing, arm_local_queue_takeover, cache_renderer_snapshot,
    clear_local_queue_takeover, confirm_local_playback_state_asserted,
    confirm_local_queue_takeover_action, ensure_session_renderer_state,
    local_playback_should_yield_to_active_peer, local_queue_takeover_needs_retry,
    reject_local_queue_takeover_action, remote_renderer_commands_are_fenced,
    set_local_playback_conflict_pending, should_materialize_remote_queue,
    sync_session_renderer_active_flags, QconnectRemoteSyncState,
};
