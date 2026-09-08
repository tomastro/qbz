//! Runtime Session Contract
//!
//! Implements the UI-agnostic runtime lifecycle as per ADR_RUNTIME_SESSION_CONTRACT.md
//!
//! Key concepts:
//! - Single bootstrap entrypoint
//! - Canonical state machine (Uninitialized -> Ready)
//! - Typed errors (no string matching)
//! - Command gating in backend
//! - Lifecycle events

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Canonical runtime states
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "data")]
pub enum RuntimeState {
    /// Initial state - nothing initialized
    Uninitialized,
    /// Client initialized but no authentication
    InitializedNoAuth,
    /// Authenticated but per-user session not activated (transitional)
    AuthenticatedNoUserSession { user_id: u64 },
    /// Fully ready - all systems operational
    Ready { user_id: u64 },
    /// Degraded state - something is broken
    Degraded { reason: DegradedReason },
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::Uninitialized
    }
}

/// Reasons for degraded state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", content = "message")]
pub enum DegradedReason {
    /// Bundle token extraction failed
    BundleExtractionFailed(String),
    /// CoreBridge initialization failed
    CoreBridgeInitFailed(String),
    /// Network connectivity issues
    NetworkError(String),
    /// Database/storage issues
    StorageError(String),
    /// Session activation failed (per-user stores not initialized)
    SessionActivationFailed(String),
}

/// Full runtime status returned by runtime_get_status and runtime_bootstrap
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    /// Current state
    pub state: RuntimeState,
    /// User ID if authenticated (None if not logged in)
    pub user_id: Option<u64>,
    /// Whether the API client is initialized (bundle tokens extracted)
    pub client_initialized: bool,
    /// Whether legacy auth is active
    pub legacy_auth: bool,
    /// Whether CoreBridge/V2 auth is active
    pub corebridge_auth: bool,
    /// Whether per-user session is activated
    pub session_activated: bool,
    /// Degraded reason if state is Degraded
    pub degraded_reason: Option<DegradedReason>,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            state: RuntimeState::Uninitialized,
            user_id: None,
            client_initialized: false,
            legacy_auth: false,
            corebridge_auth: false,
            session_activated: false,
            degraded_reason: None,
        }
    }
}

/// Typed runtime errors - no string matching in clients
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "code", content = "details")]
pub enum RuntimeError {
    /// Runtime not initialized - call runtime_bootstrap first
    RuntimeNotInitialized,
    /// Authentication required for this operation
    AuthRequired,
    /// Per-user session not activated - call activate_user_session
    UserSessionNotActivated,
    /// CoreBridge/V2 auth missing - V2 commands won't work
    CoreBridgeAuthMissing,
    /// Runtime is in degraded state
    RuntimeDegraded(DegradedReason),
    /// Invalid user ID (e.g., 0)
    InvalidUserId,
    /// Bootstrap already in progress
    BootstrapInProgress,
    /// V2 CoreBridge authentication failed
    V2AuthFailed(String),
    /// V2 CoreBridge not initialized
    V2NotInitialized,
    /// Manual offline mode is on and the requested track is not available
    /// in any local cache. Emitted instead of silently streaming from the
    /// network (see issue #279).
    TrackNotAvailableOffline,
    /// Internal error
    Internal(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuntimeNotInitialized => write!(f, "Runtime not initialized"),
            Self::AuthRequired => write!(f, "Authentication required"),
            Self::UserSessionNotActivated => write!(f, "User session not activated"),
            Self::CoreBridgeAuthMissing => write!(f, "CoreBridge authentication missing"),
            Self::RuntimeDegraded(reason) => write!(f, "Runtime degraded: {:?}", reason),
            Self::InvalidUserId => write!(f, "Invalid user ID"),
            Self::BootstrapInProgress => write!(f, "Bootstrap already in progress"),
            Self::V2AuthFailed(msg) => write!(f, "V2 authentication failed: {}", msg),
            Self::V2NotInitialized => write!(f, "V2 CoreBridge not initialized"),
            Self::TrackNotAvailableOffline => {
                write!(f, "Track not available in offline cache while in offline mode")
            }
            Self::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for RuntimeError {}

/// Command prerequisites - what each command requires
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRequirement {
    /// No requirements (public endpoints)
    None,
    /// Requires client to be initialized (bundle tokens)
    RequiresClientInit,
    /// Requires authentication (logged in)
    RequiresAuth,
    /// Requires per-user session to be activated
    RequiresUserSession,
    /// Requires CoreBridge/V2 auth (for V2 commands)
    RequiresCoreBridgeAuth,
}

/// Runtime state manager - thread-safe, holds canonical state
pub struct RuntimeManager {
    state: Arc<RwLock<RuntimeStatus>>,
    bootstrap_in_progress: Arc<RwLock<bool>>,
    /// Tracks which Mixtape/Collection the current queue was built from.
    /// Set by v2_enqueue_collection (replace mode), cleared by any
    /// non-Mixtape queue replacement (set_queue / clear_queue).
    /// Append-style ops (add/add_next/bulk) preserve this value.
    /// Persistence is in-memory only; session_queue_state table will
    /// add the source_collection_id column when it lands.
    queue_source_collection_id: RwLock<Option<String>>,
}

impl RuntimeManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(RuntimeStatus::default())),
            bootstrap_in_progress: Arc::new(RwLock::new(false)),
            queue_source_collection_id: RwLock::new(None),
        }
    }

    /// Set (or clear) which collection the current queue was built from.
    pub async fn set_queue_source_collection(&self, id: Option<String>) {
        *self.queue_source_collection_id.write().await = id;
    }

    /// Return the collection ID that seeded the current queue, if any.
    pub async fn get_queue_source_collection(&self) -> Option<String> {
        self.queue_source_collection_id.read().await.clone()
    }

    /// Get current runtime status
    pub async fn get_status(&self) -> RuntimeStatus {
        self.state.read().await.clone()
    }

    /// Update runtime state
    pub async fn set_state(&self, new_state: RuntimeState) {
        let mut status = self.state.write().await;
        status.state = new_state.clone();

        // Update derived fields based on state
        match &new_state {
            RuntimeState::Uninitialized => {
                status.client_initialized = false;
                status.legacy_auth = false;
                status.corebridge_auth = false;
                status.session_activated = false;
                status.user_id = None;
                status.degraded_reason = None;
            }
            RuntimeState::InitializedNoAuth => {
                status.client_initialized = true;
                status.legacy_auth = false;
                status.corebridge_auth = false;
                status.session_activated = false;
                status.user_id = None;
                status.degraded_reason = None;
            }
            RuntimeState::AuthenticatedNoUserSession { user_id } => {
                status.client_initialized = true;
                status.legacy_auth = true;
                status.session_activated = false;
                status.user_id = Some(*user_id);
                status.degraded_reason = None;
            }
            RuntimeState::Ready { user_id } => {
                status.client_initialized = true;
                status.legacy_auth = true;
                status.corebridge_auth = true;
                status.session_activated = true;
                status.user_id = Some(*user_id);
                status.degraded_reason = None;
            }
            RuntimeState::Degraded { reason } => {
                status.degraded_reason = Some(reason.clone());
            }
        }

        log::info!("[Runtime] State changed to: {:?}", new_state);
    }

    /// Mark client as initialized
    pub async fn set_client_initialized(&self, initialized: bool) {
        let mut status = self.state.write().await;
        status.client_initialized = initialized;
        if initialized && status.state == RuntimeState::Uninitialized {
            status.state = RuntimeState::InitializedNoAuth;
        }
    }

    /// Mark legacy auth status
    pub async fn set_legacy_auth(&self, auth: bool, user_id: Option<u64>) {
        let mut status = self.state.write().await;
        status.legacy_auth = auth;
        if auth {
            if let Some(uid) = user_id {
                status.user_id = Some(uid);
                if !status.session_activated {
                    status.state = RuntimeState::AuthenticatedNoUserSession { user_id: uid };
                }
            }
        } else {
            status.user_id = None;
            status.corebridge_auth = false;
            status.session_activated = false;
            status.state = RuntimeState::InitializedNoAuth;
        }
    }

    /// Mark CoreBridge auth status
    pub async fn set_corebridge_auth(&self, auth: bool) {
        let mut status = self.state.write().await;
        status.corebridge_auth = auth;
    }

    /// Mark session as activated
    pub async fn set_session_activated(&self, activated: bool, user_id: u64) {
        let mut status = self.state.write().await;
        status.session_activated = activated;
        if activated && status.legacy_auth && status.corebridge_auth {
            status.state = RuntimeState::Ready { user_id };
            status.user_id = Some(user_id);
        }
    }

    /// Check if bootstrap is in progress
    pub async fn is_bootstrap_in_progress(&self) -> bool {
        *self.bootstrap_in_progress.read().await
    }

    /// Set bootstrap in progress flag
    pub async fn set_bootstrap_in_progress(&self, in_progress: bool) {
        *self.bootstrap_in_progress.write().await = in_progress;
    }

    /// Validate command requirements against current state
    pub async fn check_requirements(&self, req: CommandRequirement) -> Result<(), RuntimeError> {
        let status = self.state.read().await;

        match req {
            CommandRequirement::None => Ok(()),
            CommandRequirement::RequiresClientInit => {
                if !status.client_initialized {
                    Err(RuntimeError::RuntimeNotInitialized)
                } else {
                    Ok(())
                }
            }
            CommandRequirement::RequiresAuth => {
                if !status.client_initialized {
                    Err(RuntimeError::RuntimeNotInitialized)
                } else if !status.legacy_auth {
                    Err(RuntimeError::AuthRequired)
                } else {
                    Ok(())
                }
            }
            CommandRequirement::RequiresUserSession => {
                // RequiresUserSession only checks session_activated, not legacy_auth.
                // This allows both:
                // - Online sessions: session_activated=true via activate_session()
                // - Offline sessions: session_activated=true via activate_offline_session()
                //
                // Commands needing Qobuz API should use RequiresAuth or RequiresCoreBridgeAuth.
                if !status.session_activated {
                    Err(RuntimeError::UserSessionNotActivated)
                } else {
                    Ok(())
                }
            }
            CommandRequirement::RequiresCoreBridgeAuth => {
                if !status.client_initialized {
                    Err(RuntimeError::RuntimeNotInitialized)
                } else if !status.legacy_auth {
                    Err(RuntimeError::AuthRequired)
                } else if !status.session_activated {
                    Err(RuntimeError::UserSessionNotActivated)
                } else if !status.corebridge_auth {
                    Err(RuntimeError::CoreBridgeAuthMissing)
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Check if in degraded state
    pub async fn is_degraded(&self) -> bool {
        matches!(self.state.read().await.state, RuntimeState::Degraded { .. })
    }

    /// Set degraded state
    pub async fn set_degraded(&self, reason: DegradedReason) {
        self.set_state(RuntimeState::Degraded { reason }).await;
    }
}

impl Default for RuntimeManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Events emitted during runtime lifecycle
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum RuntimeEvent {
    /// A live (cold) bundle-token extraction has started — no cache was
    /// available, so we must download Qobuz's ~7 MB bundle before the UI can
    /// proceed. The frontend shows a "connecting to Qobuz" state. Only emitted
    /// on a cold start (first run or after a cache wipe); warm starts skip it.
    BundleFetchStarted,
    /// Runtime initialized (client ready)
    RuntimeInitialized,
    /// Authentication state changed
    AuthChanged {
        logged_in: bool,
        user_id: Option<u64>,
    },
    /// Per-user session activated
    UserSessionActivated { user_id: u64 },
    /// Per-user session deactivated
    UserSessionDeactivated,
    /// CoreBridge auth failed
    CoreBridgeAuthFailed { error: String },
    /// Runtime entered degraded state
    RuntimeDegraded { reason: DegradedReason },
    /// Runtime fully ready
    RuntimeReady { user_id: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mixtape_context_set_clear() {
        let mgr = RuntimeManager::new();

        // Initially None
        assert_eq!(mgr.get_queue_source_collection().await, None);

        // Set Some
        mgr.set_queue_source_collection(Some("col-abc".to_string())).await;
        assert_eq!(
            mgr.get_queue_source_collection().await,
            Some("col-abc".to_string())
        );

        // Clear to None
        mgr.set_queue_source_collection(None).await;
        assert_eq!(mgr.get_queue_source_collection().await, None);
    }

    #[tokio::test]
    async fn default_state_is_uninitialized() {
        let mgr = RuntimeManager::new();
        let status = mgr.get_status().await;

        assert_eq!(status.state, RuntimeState::Uninitialized);
        assert!(!status.client_initialized);
        assert!(!status.legacy_auth);
        assert!(!status.corebridge_auth);
        assert!(!status.session_activated);
        assert_eq!(status.user_id, None);
    }

    #[tokio::test]
    async fn client_initialized_transitions_to_initialized_no_auth() {
        let mgr = RuntimeManager::new();

        mgr.set_client_initialized(true).await;
        let status = mgr.get_status().await;

        assert_eq!(status.state, RuntimeState::InitializedNoAuth);
        assert!(status.client_initialized);
        assert!(!status.legacy_auth);
        assert_eq!(status.user_id, None);
    }

    #[tokio::test]
    async fn legacy_auth_transitions_to_authenticated_no_user_session() {
        let mgr = RuntimeManager::new();

        mgr.set_client_initialized(true).await;
        mgr.set_legacy_auth(true, Some(42)).await;
        let status = mgr.get_status().await;

        assert_eq!(
            status.state,
            RuntimeState::AuthenticatedNoUserSession { user_id: 42 }
        );
        assert!(status.client_initialized);
        assert!(status.legacy_auth);
        assert!(!status.session_activated);
        assert_eq!(status.user_id, Some(42));
    }

    #[tokio::test]
    async fn corebridge_auth_and_session_activation_reaches_ready() {
        let mgr = RuntimeManager::new();

        mgr.set_client_initialized(true).await;
        mgr.set_legacy_auth(true, Some(42)).await;
        mgr.set_corebridge_auth(true).await;
        mgr.set_session_activated(true, 42).await;
        let status = mgr.get_status().await;

        assert_eq!(status.state, RuntimeState::Ready { user_id: 42 });
        assert!(status.client_initialized);
        assert!(status.legacy_auth);
        assert!(status.corebridge_auth);
        assert!(status.session_activated);
        assert_eq!(status.user_id, Some(42));
    }

    #[tokio::test]
    async fn requires_user_session_accepts_offline_session_without_legacy_auth() {
        let mgr = RuntimeManager::new();

        mgr.set_session_activated(true, 0).await;

        assert!(mgr
            .check_requirements(CommandRequirement::RequiresUserSession)
            .await
            .is_ok());
        assert!(matches!(
            mgr.check_requirements(CommandRequirement::RequiresAuth).await,
            Err(RuntimeError::RuntimeNotInitialized)
        ));
    }
}
