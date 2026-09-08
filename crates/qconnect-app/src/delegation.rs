//! Transactional authority switching for official QConnect LAN handoffs.
//!
//! The coordinator owns generations, cancellation and the commit gate. Hosts
//! own frontend-specific runtime construction. Network work always happens
//! before the commit gate; the gate covers only the final authority swap and
//! lifecycle invalidation, so a late candidate can never become authoritative.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::{stream::FuturesUnordered, FutureExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// A candidate whose shortest-lived delegated credential bounds its authority.
pub trait DelegationCandidate: Send + 'static {
    fn expires_at_unix_secs(&self) -> i64;
}

impl DelegationCandidate for qconnect_lan::HandoffCandidate {
    fn expires_at_unix_secs(&self) -> i64 {
        self.api_token()
            .expires_at()
            .min(self.qconnect_token().expires_at())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialOrigin {
    Owner,
    Delegated {
        generation: u64,
        expires_at_unix_secs: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationPhase {
    Disabled,
    ShuttingDown,
    OwnerReady,
    CandidateValidating { generation: u64 },
    CandidateValidated { generation: u64 },
    Activating { generation: u64 },
    DelegatedActive { generation: u64 },
    RestoringOwner { generation: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationErrorCode {
    CandidateExpired,
    CandidateCancelled,
    ValidationTimedOut,
    ApiRejected,
    QwsRejected,
    ActivationRejected,
    CommitRejected,
    OwnerRestoreFailed,
    GenerationExhausted,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationSnapshot {
    pub revision: u64,
    pub phase: DelegationPhase,
    pub credential_origin: CredentialOrigin,
    pub candidate_generation: Option<u64>,
    pub last_error: Option<DelegationErrorCode>,
}

#[derive(Debug, Clone, Copy)]
pub struct DelegationCoordinatorConfig {
    /// One deadline covers API validation, QWS preflight, local preparation and
    /// the targeted `SET_ACTIVE=true` activation proof.
    pub transaction_timeout: Duration,
    /// Cleanup is awaited but bounded so disable/logout cannot hang forever.
    pub cleanup_timeout: Duration,
    /// Delay between failed owner-restore attempts. The active delegated
    /// authority stays supervised until a restore succeeds or shutdown wins.
    pub restore_retry_backoff: Duration,
}

impl Default for DelegationCoordinatorConfig {
    fn default() -> Self {
        Self {
            transaction_timeout: Duration::from_secs(30),
            cleanup_timeout: Duration::from_secs(2),
            restore_retry_backoff: Duration::from_millis(250),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreReason {
    CredentialExpired,
    TransportFatal,
}

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum DelegationCoordinatorError {
    #[error("qconnect-delegation-disabled")]
    Disabled,
    #[error("qconnect-delegation-expired")]
    CandidateExpired,
    #[error("qconnect-delegation-generation-exhausted")]
    GenerationExhausted,
}

/// Cooperative cancellation passed to host preparation and activation.
///
/// The coordinator also selects on this signal itself, so correctness does not
/// rely on every host await polling it. Hosts may use it to stop nested work
/// promptly and must not detach unowned tasks.
#[derive(Clone)]
pub struct DelegationCancellation {
    receiver: watch::Receiver<bool>,
}

impl DelegationCancellation {
    #[cfg(test)]
    pub(crate) fn from_receiver(receiver: watch::Receiver<bool>) -> Self {
        Self { receiver }
    }

    pub fn is_cancelled(&self) -> bool {
        *self.receiver.borrow()
    }

    pub async fn cancelled(&mut self) {
        loop {
            if *self.receiver.borrow() {
                return;
            }
            if self.receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

/// A failed atomic commit returns ownership of the prepared authority so the
/// coordinator can explicitly shut it down and scrub it.
pub struct CommitRejected<T> {
    prepared: T,
    code: DelegationErrorCode,
}

impl<T> CommitRejected<T> {
    pub const fn new(prepared: T, code: DelegationErrorCode) -> Self {
        Self { prepared, code }
    }

    fn into_parts(self) -> (T, DelegationErrorCode) {
        (self.prepared, self.code)
    }
}

/// Frontend/runtime-specific seams used by the shared coordinator.
///
/// `prepare_*` and `activate_delegation` may do I/O. `commit_*` is synchronous
/// and runs while the coordinator's short commit gate is held. It MUST be an
/// atomic, non-blocking authority/stamp swap: an `Err` means no externally
/// visible mutation occurred. It must not perform I/O, wait for a task, acquire
/// an async mutex, or touch audio.
#[async_trait]
pub trait DelegationHost: Send + Sync + 'static {
    type Candidate: DelegationCandidate;
    type PreparedDelegation: Send + 'static;
    type PreparedOwner: Send + 'static;
    type RetiredAuthority: Send + 'static;

    async fn prepare_delegation(
        &self,
        generation: u64,
        candidate: Self::Candidate,
        cancellation: DelegationCancellation,
    ) -> Result<Self::PreparedDelegation, DelegationErrorCode>;

    /// Last I/O step before local commit: send the exact renderer join and wait
    /// for the targeted `SRVR_RNDR_SET_ACTIVE { active: true }` command.
    async fn activate_delegation(
        &self,
        generation: u64,
        prepared: &mut Self::PreparedDelegation,
        cancellation: DelegationCancellation,
    ) -> Result<(), DelegationErrorCode>;

    fn commit_delegation(
        &self,
        generation: u64,
        prepared: Self::PreparedDelegation,
    ) -> Result<Self::RetiredAuthority, CommitRejected<Self::PreparedDelegation>>;

    async fn discard_delegation(&self, generation: u64, prepared: Self::PreparedDelegation);

    async fn prepare_owner(
        &self,
        generation: u64,
        reason: RestoreReason,
        cancellation: DelegationCancellation,
    ) -> Result<Self::PreparedOwner, DelegationErrorCode>;

    fn commit_owner(
        &self,
        generation: u64,
        prepared: Self::PreparedOwner,
    ) -> Result<Self::RetiredAuthority, CommitRejected<Self::PreparedOwner>>;

    async fn discard_owner(&self, generation: u64, prepared: Self::PreparedOwner);

    async fn retire_authority(&self, retired: Self::RetiredAuthority);

    /// Stop whichever authority is installed. Must be idempotent.
    async fn shutdown_authority(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Candidate,
    Restore {
        expected_active: u64,
        reason: RestoreReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateStage {
    Validating,
    Validated,
    Activating,
}

struct PendingControl {
    generation: u64,
    kind: PendingKind,
    stage: CandidateStage,
    cancel: watch::Sender<bool>,
}

enum ActiveControl {
    Owner,
    Delegated {
        generation: u64,
        expires_at_unix_secs: i64,
        cancel: watch::Sender<bool>,
        restore_required: Option<RestoreReason>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoordinatorLifecycle {
    Disabled,
    Enabled,
    ShuttingDown,
}

struct CoordinatorState {
    lifecycle: CoordinatorLifecycle,
    revision: u64,
    next_generation: u64,
    pending: Option<PendingControl>,
    active: ActiveControl,
    last_error: Option<DelegationErrorCode>,
}

impl CoordinatorState {
    fn owner_ready() -> Self {
        Self {
            lifecycle: CoordinatorLifecycle::Enabled,
            revision: 0,
            next_generation: 0,
            pending: None,
            active: ActiveControl::Owner,
            last_error: None,
        }
    }

    fn disabled() -> Self {
        Self {
            lifecycle: CoordinatorLifecycle::Disabled,
            revision: 0,
            next_generation: 0,
            pending: None,
            active: ActiveControl::Owner,
            last_error: None,
        }
    }

    fn allocate_generation(&mut self) -> Result<u64, DelegationCoordinatorError> {
        let next = self
            .next_generation
            .checked_add(1)
            .ok_or(DelegationCoordinatorError::GenerationExhausted)?;
        self.next_generation = next;
        Ok(next)
    }

    fn credential_origin(&self) -> CredentialOrigin {
        match self.active {
            ActiveControl::Owner => CredentialOrigin::Owner,
            ActiveControl::Delegated {
                generation,
                expires_at_unix_secs,
                ..
            } => CredentialOrigin::Delegated {
                generation,
                expires_at_unix_secs,
            },
        }
    }

    fn snapshot(&self) -> DelegationSnapshot {
        let phase = if self.lifecycle == CoordinatorLifecycle::Disabled {
            DelegationPhase::Disabled
        } else if self.lifecycle == CoordinatorLifecycle::ShuttingDown {
            DelegationPhase::ShuttingDown
        } else if let Some(pending) = self.pending.as_ref() {
            match pending.kind {
                PendingKind::Restore { .. } => DelegationPhase::RestoringOwner {
                    generation: pending.generation,
                },
                PendingKind::Candidate => match pending.stage {
                    CandidateStage::Validating => DelegationPhase::CandidateValidating {
                        generation: pending.generation,
                    },
                    CandidateStage::Validated => DelegationPhase::CandidateValidated {
                        generation: pending.generation,
                    },
                    CandidateStage::Activating => DelegationPhase::Activating {
                        generation: pending.generation,
                    },
                },
            }
        } else {
            match self.active {
                ActiveControl::Owner => DelegationPhase::OwnerReady,
                ActiveControl::Delegated { generation, .. } => {
                    DelegationPhase::DelegatedActive { generation }
                }
            }
        };
        DelegationSnapshot {
            revision: self.revision,
            phase,
            credential_origin: self.credential_origin(),
            candidate_generation: self.pending.as_ref().and_then(|pending| {
                matches!(pending.kind, PendingKind::Candidate).then_some(pending.generation)
            }),
            last_error: self.last_error,
        }
    }

    fn publish(&mut self, sender: &watch::Sender<DelegationSnapshot>) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("delegation snapshot revision exhausted");
        sender.send_replace(self.snapshot());
    }
}

struct CoordinatorInner<H: DelegationHost> {
    host: Arc<H>,
    config: DelegationCoordinatorConfig,
    state: Mutex<CoordinatorState>,
    /// Serializes generation invalidation against the final authority swap.
    /// It never covers candidate network I/O or retirement.
    commit_gate: Mutex<()>,
    /// Serializes the complete shutdown cleanup and makes repeated concurrent
    /// shutdown calls idempotent.
    shutdown_gate: Mutex<()>,
    tasks: std::sync::Mutex<HashMap<u64, JoinHandle<()>>>,
    snapshot_tx: watch::Sender<DelegationSnapshot>,
}

pub struct DelegationCoordinator<H: DelegationHost> {
    inner: Arc<CoordinatorInner<H>>,
}

impl<H: DelegationHost> Clone for DelegationCoordinator<H> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<H: DelegationHost> DelegationCoordinator<H> {
    pub fn owner_ready(host: Arc<H>, config: DelegationCoordinatorConfig) -> Self {
        Self::new(host, config, CoordinatorState::owner_ready())
    }

    pub fn disabled(host: Arc<H>, config: DelegationCoordinatorConfig) -> Self {
        Self::new(host, config, CoordinatorState::disabled())
    }

    fn new(host: Arc<H>, config: DelegationCoordinatorConfig, state: CoordinatorState) -> Self {
        let initial = state.snapshot();
        let (snapshot_tx, _) = watch::channel(initial);
        Self {
            inner: Arc::new(CoordinatorInner {
                host,
                config,
                state: Mutex::new(state),
                commit_gate: Mutex::new(()),
                shutdown_gate: Mutex::new(()),
                tasks: std::sync::Mutex::new(HashMap::new()),
                snapshot_tx,
            }),
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<DelegationSnapshot> {
        self.inner.snapshot_tx.subscribe()
    }

    pub fn snapshot(&self) -> DelegationSnapshot {
        *self.inner.snapshot_tx.borrow()
    }

    /// Declare that the already-prepared owner authority is ready. Intended for
    /// initial global-QConnect startup after owner authentication.
    pub async fn declare_owner_ready(&self) -> bool {
        self.declare_owner_ready_if(|| true).await
    }

    /// Declare owner readiness only while an external lifecycle intent remains
    /// current. The predicate runs inside the coordinator commit gate and must
    /// be synchronous and bounded. This closes disable-vs-connect races without
    /// making teardown wait behind owner network preparation.
    pub async fn declare_owner_ready_if(&self, is_current: impl FnOnce() -> bool) -> bool {
        let _gate = self.inner.commit_gate.lock().await;
        let mut state = self.inner.state.lock().await;
        if !is_current()
            || state.lifecycle != CoordinatorLifecycle::Disabled
            || state.pending.is_some()
            || !matches!(state.active, ActiveControl::Owner)
        {
            return false;
        }
        state.lifecycle = CoordinatorLifecycle::Enabled;
        state.last_error = None;
        state.publish(&self.inner.snapshot_tx);
        true
    }

    /// Admit a validated LAN candidate. A pending candidate/restore is
    /// cancelled, but an already-active delegation remains authoritative until
    /// this generation is fully prepared and committed.
    pub async fn admit(&self, candidate: H::Candidate) -> Result<u64, DelegationCoordinatorError> {
        let expires_at = candidate.expires_at_unix_secs();
        if expires_at <= unix_now_secs() {
            return Err(DelegationCoordinatorError::CandidateExpired);
        }

        let gate = self.inner.commit_gate.lock().await;
        let (generation, cancellation) = {
            let mut state = self.inner.state.lock().await;
            if state.lifecycle != CoordinatorLifecycle::Enabled {
                return Err(DelegationCoordinatorError::Disabled);
            }
            let generation = match state.allocate_generation() {
                Ok(generation) => generation,
                Err(error) => {
                    disable_for_generation_exhaustion(&mut state);
                    state.publish(&self.inner.snapshot_tx);
                    return Err(error);
                }
            };
            if let Some(pending) = state.pending.take() {
                pending.cancel.send_replace(true);
            }
            let (cancel, receiver) = watch::channel(false);
            state.pending = Some(PendingControl {
                generation,
                kind: PendingKind::Candidate,
                stage: CandidateStage::Validating,
                cancel,
            });
            state.last_error = None;
            state.publish(&self.inner.snapshot_tx);
            (generation, DelegationCancellation { receiver })
        };

        self.inner
            .spawn_candidate(generation, expires_at, candidate, cancellation);
        drop(gate);
        Ok(generation)
    }

    /// Restore the owner only if `expected_active_generation` is still the
    /// installed delegation. A late failure from an older runtime cannot tear
    /// down its replacement.
    pub async fn restore_owner_if_active(
        &self,
        expected_active_generation: u64,
        reason: RestoreReason,
    ) -> Result<bool, DelegationCoordinatorError> {
        self.inner
            .start_restore(expected_active_generation, reason)
            .await
    }

    /// Global disable/logout/shutdown. Admission is invalidated under the same
    /// gate as commit, then every owned transaction is cancelled and joined
    /// before the installed authority is stopped.
    pub async fn shutdown(&self) {
        let _shutdown = self.inner.shutdown_gate.lock().await;
        {
            let _gate = self.inner.commit_gate.lock().await;
            let mut state = self.inner.state.lock().await;
            if state.lifecycle == CoordinatorLifecycle::Disabled {
                return;
            }
            state.lifecycle = CoordinatorLifecycle::ShuttingDown;
            if let Some(pending) = state.pending.take() {
                pending.cancel.send_replace(true);
            }
            if let ActiveControl::Delegated { cancel, .. } = &state.active {
                cancel.send_replace(true);
            }
            state.last_error = None;
            state.publish(&self.inner.snapshot_tx);
        }

        let tasks = {
            let mut tasks = recover_std_lock(&self.inner.tasks);
            tasks.drain().map(|(_, task)| task).collect::<Vec<_>>()
        };
        await_all_or_abort(tasks, self.inner.config.cleanup_timeout).await;
        let shutdown_timed_out = tokio::time::timeout(
            self.inner.config.cleanup_timeout,
            self.inner.host.shutdown_authority(),
        )
        .await
        .is_err();

        let _gate = self.inner.commit_gate.lock().await;
        let mut state = self.inner.state.lock().await;
        state.lifecycle = CoordinatorLifecycle::Disabled;
        state.pending = None;
        state.active = ActiveControl::Owner;
        state.last_error = shutdown_timed_out.then_some(DelegationErrorCode::Internal);
        state.publish(&self.inner.snapshot_tx);
    }
}

impl<H: DelegationHost> CoordinatorInner<H> {
    fn spawn_candidate(
        self: &Arc<Self>,
        generation: u64,
        expires_at: i64,
        candidate: H::Candidate,
        cancellation: DelegationCancellation,
    ) {
        let runner = Arc::clone(self);
        let supervisor = Arc::clone(self);
        let task = tokio::spawn(async move {
            if AssertUnwindSafe(runner.run_candidate(
                generation,
                expires_at,
                candidate,
                cancellation,
            ))
            .catch_unwind()
            .await
            .is_err()
            {
                supervisor
                    .fail_pending(generation, DelegationErrorCode::Internal)
                    .await;
            }
        });
        register_task(self, generation, task);
    }

    fn spawn_restore(
        self: &Arc<Self>,
        generation: u64,
        expected_active: u64,
        reason: RestoreReason,
        cancellation: DelegationCancellation,
    ) {
        let runner = Arc::clone(self);
        let supervisor = Arc::clone(self);
        let task = tokio::spawn(async move {
            if AssertUnwindSafe(runner.run_restore(
                generation,
                expected_active,
                reason,
                cancellation,
            ))
            .catch_unwind()
            .await
            .is_err()
            {
                supervisor
                    .fail_pending(generation, DelegationErrorCode::Internal)
                    .await;
            }
        });
        register_task(self, generation, task);
    }

    async fn run_candidate(
        self: Arc<Self>,
        generation: u64,
        expires_at: i64,
        candidate: H::Candidate,
        cancellation: DelegationCancellation,
    ) {
        let deadline = Instant::now() + self.config.transaction_timeout;
        let prepared = match bounded_step(
            deadline,
            cancellation.clone(),
            self.host
                .prepare_delegation(generation, candidate, cancellation.clone()),
        )
        .await
        {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(code)) => {
                self.fail_pending(generation, code).await;
                return;
            }
            Err(StepStop::Cancelled) => {
                self.fail_pending(generation, DelegationErrorCode::CandidateCancelled)
                    .await;
                return;
            }
            Err(StepStop::TimedOut) => {
                self.fail_pending(generation, DelegationErrorCode::ValidationTimedOut)
                    .await;
                return;
            }
        };

        if !self
            .set_candidate_stage(generation, CandidateStage::Validated)
            .await
            || expires_at <= unix_now_secs()
        {
            self.discard_delegation(generation, prepared).await;
            self.fail_pending(generation, DelegationErrorCode::CandidateExpired)
                .await;
            return;
        }

        let mut prepared = prepared;
        if !self
            .set_candidate_stage(generation, CandidateStage::Activating)
            .await
        {
            self.discard_delegation(generation, prepared).await;
            return;
        }
        match bounded_step(
            deadline,
            cancellation.clone(),
            self.host
                .activate_delegation(generation, &mut prepared, cancellation.clone()),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(code)) => {
                self.discard_delegation(generation, prepared).await;
                self.fail_pending(generation, code).await;
                return;
            }
            Err(StepStop::Cancelled) => {
                self.discard_delegation(generation, prepared).await;
                self.fail_pending(generation, DelegationErrorCode::CandidateCancelled)
                    .await;
                return;
            }
            Err(StepStop::TimedOut) => {
                self.discard_delegation(generation, prepared).await;
                self.fail_pending(generation, DelegationErrorCode::ValidationTimedOut)
                    .await;
                return;
            }
        }

        let gate = self.commit_gate.lock().await;
        let mut state = self.state.lock().await;
        let expired_before_commit = expires_at <= unix_now_secs();
        let candidate_matches = state.lifecycle == CoordinatorLifecycle::Enabled
            && state.pending.as_ref().is_some_and(|pending| {
                pending.generation == generation && matches!(pending.kind, PendingKind::Candidate)
            });
        if cancellation.is_cancelled() || expired_before_commit || !candidate_matches {
            let code = if expired_before_commit {
                DelegationErrorCode::CandidateExpired
            } else {
                DelegationErrorCode::CandidateCancelled
            };
            drop(state);
            drop(gate);
            self.discard_delegation(generation, prepared).await;
            self.fail_pending(generation, code).await;
            return;
        }

        let retired = match self.host.commit_delegation(generation, prepared) {
            Ok(retired) => retired,
            Err(rejected) => {
                let (prepared, code) = rejected.into_parts();
                drop(state);
                drop(gate);
                self.discard_delegation(generation, prepared).await;
                self.fail_pending(generation, code).await;
                return;
            }
        };

        let pending = state
            .pending
            .take()
            .expect("candidate pending invariant held across synchronous commit");
        if let ActiveControl::Delegated { cancel, .. } = &state.active {
            cancel.send_replace(true);
        }
        let active_cancellation = pending.cancel.clone();
        let expired_after_commit = expires_at <= unix_now_secs();
        state.active = ActiveControl::Delegated {
            generation,
            expires_at_unix_secs: expires_at,
            cancel: pending.cancel,
            restore_required: expired_after_commit.then_some(RestoreReason::CredentialExpired),
        };
        state.last_error = None;
        let immediate_restore = prepare_required_restore(&mut state);
        state.publish(&self.snapshot_tx);
        drop(state);
        if let Some((restore_generation, expected_active, reason, cancellation)) = immediate_restore
        {
            self.spawn_restore(restore_generation, expected_active, reason, cancellation);
        }
        drop(gate);

        if expired_after_commit {
            self.retire_authority(retired).await;
            return;
        }

        let monitor = async {
            let mut active_cancel = DelegationCancellation {
                receiver: active_cancellation.subscribe(),
            };
            let remaining_secs = expires_at.saturating_sub(unix_now_secs());
            let until_expiry = if remaining_secs <= 0 {
                Duration::ZERO
            } else {
                Duration::from_secs(remaining_secs as u64)
            };
            tokio::select! {
                _ = active_cancel.cancelled() => {}
                _ = tokio::time::sleep(until_expiry) => {
                    let _ = self
                        .start_restore(generation, RestoreReason::CredentialExpired)
                        .await;
                }
            }
        };
        tokio::join!(self.retire_authority(retired), monitor);
    }

    async fn start_restore(
        self: &Arc<Self>,
        expected_active: u64,
        reason: RestoreReason,
    ) -> Result<bool, DelegationCoordinatorError> {
        let gate = self.commit_gate.lock().await;
        let start = {
            let mut state = self.state.lock().await;
            if state.lifecycle != CoordinatorLifecycle::Enabled {
                return Err(DelegationCoordinatorError::Disabled);
            }
            let active_cancel = match &mut state.active {
                ActiveControl::Delegated {
                    generation: active_generation,
                    cancel,
                    restore_required,
                    ..
                } if *active_generation == expected_active => {
                    *restore_required = Some(merge_restore_reason(*restore_required, reason));
                    cancel.clone()
                }
                _ => return Ok(false),
            };

            if let Some(pending) = state.pending.as_ref() {
                match pending.kind {
                    PendingKind::Candidate => {
                        state.publish(&self.snapshot_tx);
                        return Ok(true);
                    }
                    PendingKind::Restore {
                        expected_active: pending_active,
                        ..
                    } if pending_active == expected_active => {
                        state.publish(&self.snapshot_tx);
                        return Ok(true);
                    }
                    PendingKind::Restore { .. } => {}
                }
            }

            let generation = match state.allocate_generation() {
                Ok(generation) => generation,
                Err(error) => {
                    disable_for_generation_exhaustion(&mut state);
                    state.publish(&self.snapshot_tx);
                    return Err(error);
                }
            };
            if let Some(pending) = state.pending.take() {
                pending.cancel.send_replace(true);
            }
            active_cancel.send_replace(true);
            let (cancel, receiver) = watch::channel(false);
            state.pending = Some(PendingControl {
                generation,
                kind: PendingKind::Restore {
                    expected_active,
                    reason,
                },
                stage: CandidateStage::Validating,
                cancel,
            });
            state.last_error = None;
            state.publish(&self.snapshot_tx);
            Some((generation, DelegationCancellation { receiver }))
        };

        if let Some((generation, cancellation)) = start {
            self.spawn_restore(generation, expected_active, reason, cancellation);
        }
        drop(gate);
        Ok(true)
    }

    async fn run_restore(
        self: Arc<Self>,
        generation: u64,
        expected_active: u64,
        reason: RestoreReason,
        cancellation: DelegationCancellation,
    ) {
        loop {
            let deadline = Instant::now() + self.config.transaction_timeout;
            let prepared = match bounded_step(
                deadline,
                cancellation.clone(),
                self.host
                    .prepare_owner(generation, reason, cancellation.clone()),
            )
            .await
            {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(_)) | Err(StepStop::TimedOut) => {
                    if !self
                        .record_restore_failure_and_wait(
                            generation,
                            expected_active,
                            cancellation.clone(),
                        )
                        .await
                    {
                        return;
                    }
                    continue;
                }
                Err(StepStop::Cancelled) => return,
            };

            let gate = self.commit_gate.lock().await;
            let mut state = self.state.lock().await;
            if cancellation.is_cancelled()
                || state.lifecycle != CoordinatorLifecycle::Enabled
                || !pending_matches_restore_state(&state, generation, expected_active)
            {
                drop(state);
                drop(gate);
                self.discard_owner(generation, prepared).await;
                return;
            }
            let retired = match self.host.commit_owner(generation, prepared) {
                Ok(retired) => retired,
                Err(rejected) => {
                    let (prepared, _) = rejected.into_parts();
                    drop(state);
                    drop(gate);
                    self.discard_owner(generation, prepared).await;
                    if !self
                        .record_restore_failure_and_wait(
                            generation,
                            expected_active,
                            cancellation.clone(),
                        )
                        .await
                    {
                        return;
                    }
                    continue;
                }
            };
            state.pending = None;
            state.active = ActiveControl::Owner;
            state.last_error = None;
            state.publish(&self.snapshot_tx);
            drop(state);
            drop(gate);
            self.retire_authority(retired).await;
            return;
        }
    }

    async fn set_candidate_stage(&self, generation: u64, stage: CandidateStage) -> bool {
        let mut state = self.state.lock().await;
        if state.lifecycle != CoordinatorLifecycle::Enabled {
            return false;
        }
        let Some(pending) = state.pending.as_mut() else {
            return false;
        };
        if pending.generation != generation || !matches!(pending.kind, PendingKind::Candidate) {
            return false;
        }
        pending.stage = stage;
        state.publish(&self.snapshot_tx);
        true
    }

    async fn fail_pending(self: &Arc<Self>, generation: u64, code: DelegationErrorCode) {
        let gate = self.commit_gate.lock().await;
        let restore = {
            let mut state = self.state.lock().await;
            let Some(pending) = state.pending.as_ref() else {
                return;
            };
            if pending.generation != generation {
                return;
            }
            state.pending = None;
            state.last_error = Some(code);
            let restore = prepare_required_restore(&mut state);
            state.publish(&self.snapshot_tx);
            restore
        };
        if let Some((restore_generation, expected_active, reason, cancellation)) = restore {
            self.spawn_restore(restore_generation, expected_active, reason, cancellation);
        }
        drop(gate);
    }

    async fn record_restore_failure_and_wait(
        &self,
        generation: u64,
        expected_active: u64,
        mut cancellation: DelegationCancellation,
    ) -> bool {
        {
            let mut state = self.state.lock().await;
            if state.lifecycle != CoordinatorLifecycle::Enabled
                || !pending_matches_restore_state(&state, generation, expected_active)
            {
                return false;
            }
            state.last_error = Some(DelegationErrorCode::OwnerRestoreFailed);
            state.publish(&self.snapshot_tx);
        }

        let backoff = self
            .config
            .restore_retry_backoff
            .max(Duration::from_millis(10));
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = cancellation.cancelled() => return false,
        }
        let state = self.state.lock().await;
        state.lifecycle == CoordinatorLifecycle::Enabled
            && pending_matches_restore_state(&state, generation, expected_active)
    }

    async fn discard_delegation(&self, generation: u64, prepared: H::PreparedDelegation) {
        let _ = tokio::time::timeout(
            self.config.cleanup_timeout,
            self.host.discard_delegation(generation, prepared),
        )
        .await;
    }

    async fn discard_owner(&self, generation: u64, prepared: H::PreparedOwner) {
        let _ = tokio::time::timeout(
            self.config.cleanup_timeout,
            self.host.discard_owner(generation, prepared),
        )
        .await;
    }

    async fn retire_authority(&self, retired: H::RetiredAuthority) {
        let _ = tokio::time::timeout(
            self.config.cleanup_timeout,
            self.host.retire_authority(retired),
        )
        .await;
    }
}

type RestoreStart = (u64, u64, RestoreReason, DelegationCancellation);

fn prepare_required_restore(state: &mut CoordinatorState) -> Option<RestoreStart> {
    if state.lifecycle != CoordinatorLifecycle::Enabled || state.pending.is_some() {
        return None;
    }
    let (expected_active, reason, active_cancel) = match &state.active {
        ActiveControl::Delegated {
            generation,
            cancel,
            restore_required: Some(reason),
            ..
        } => (*generation, *reason, cancel.clone()),
        ActiveControl::Owner | ActiveControl::Delegated { .. } => return None,
    };
    let generation = match state.allocate_generation() {
        Ok(generation) => generation,
        Err(_) => {
            disable_for_generation_exhaustion(state);
            return None;
        }
    };
    active_cancel.send_replace(true);
    let (cancel, receiver) = watch::channel(false);
    state.pending = Some(PendingControl {
        generation,
        kind: PendingKind::Restore {
            expected_active,
            reason,
        },
        stage: CandidateStage::Validating,
        cancel,
    });
    Some((
        generation,
        expected_active,
        reason,
        DelegationCancellation { receiver },
    ))
}

fn pending_matches_restore_state(
    state: &CoordinatorState,
    generation: u64,
    expected_active: u64,
) -> bool {
    matches!(
        state.active,
        ActiveControl::Delegated { generation, .. } if generation == expected_active
    ) && state.pending.as_ref().is_some_and(|pending| {
        pending.generation == generation
            && matches!(
                pending.kind,
                PendingKind::Restore {
                    expected_active: active,
                    ..
                } if active == expected_active
            )
    })
}

const fn merge_restore_reason(
    current: Option<RestoreReason>,
    requested: RestoreReason,
) -> RestoreReason {
    match (current, requested) {
        (Some(RestoreReason::TransportFatal), _) | (_, RestoreReason::TransportFatal) => {
            RestoreReason::TransportFatal
        }
        _ => RestoreReason::CredentialExpired,
    }
}

fn disable_for_generation_exhaustion(state: &mut CoordinatorState) {
    state.lifecycle = CoordinatorLifecycle::Disabled;
    if let Some(pending) = state.pending.take() {
        pending.cancel.send_replace(true);
    }
    if let ActiveControl::Delegated { cancel, .. } = &state.active {
        cancel.send_replace(true);
    }
    state.last_error = Some(DelegationErrorCode::GenerationExhausted);
}

fn register_task<H: DelegationHost>(
    inner: &Arc<CoordinatorInner<H>>,
    generation: u64,
    task: JoinHandle<()>,
) {
    let mut tasks = recover_std_lock(&inner.tasks);
    tasks.retain(|_, task| !task.is_finished());
    tasks.insert(generation, task);
}

fn recover_std_lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Give every candidate/restore transaction one shared graceful-cleanup
/// budget, then abort the remainder together. A hostile burst must not turn a
/// nominal two-second shutdown budget into two seconds per generation.
async fn await_all_or_abort(tasks: Vec<JoinHandle<()>>, timeout: Duration) {
    let mut tasks = tasks.into_iter().collect::<FuturesUnordered<_>>();
    let timed_out = tokio::time::timeout(timeout, async { while tasks.next().await.is_some() {} })
        .await
        .is_err();
    if !timed_out {
        return;
    }

    for task in tasks.iter() {
        task.abort();
    }
    // Aborted Tokio tasks normally join on the next poll. Keep even this tail
    // bounded: cancellation-safe host cleanup and authority stamps own the
    // late-resource safety if a task refuses to yield promptly.
    let _ = tokio::time::timeout(Duration::from_millis(250), async {
        while tasks.next().await.is_some() {}
    })
    .await;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepStop {
    Cancelled,
    TimedOut,
}

async fn bounded_step<T, F>(
    deadline: Instant,
    mut cancellation: DelegationCancellation,
    future: F,
) -> Result<T, StepStop>
where
    F: Future<Output = T>,
{
    tokio::pin!(future);
    let sleep = tokio::time::sleep_until(deadline);
    tokio::pin!(sleep);
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(StepStop::Cancelled),
        _ = &mut sleep => Err(StepStop::TimedOut),
        value = &mut future => Ok(value),
    }
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Mutex as StdMutex;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn candidate_cleanup_uses_one_shared_shutdown_budget() {
        let tasks = (0..3)
            .map(|_| tokio::spawn(std::future::pending::<()>()))
            .collect();
        let cleanup = tokio::spawn(await_all_or_abort(tasks, Duration::from_secs(2)));

        tokio::task::yield_now().await;
        assert!(!cleanup.is_finished());
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::task::yield_now().await;

        assert!(
            cleanup.is_finished(),
            "cleanup multiplied its timeout by the number of candidate tasks"
        );
        cleanup.await.expect("cleanup task panicked");
    }

    #[derive(Clone, Copy)]
    struct FakeCandidate {
        id: u64,
        expires_at: i64,
        prepare_delay: Duration,
        fail: Option<DelegationErrorCode>,
    }

    impl DelegationCandidate for FakeCandidate {
        fn expires_at_unix_secs(&self) -> i64 {
            self.expires_at
        }
    }

    #[derive(Default)]
    struct FakeHost {
        authority: StdMutex<Option<u64>>,
        commits: StdMutex<Vec<u64>>,
        discards: StdMutex<Vec<u64>>,
        shutdowns: StdMutex<u32>,
        owner_prepare_attempts: AtomicUsize,
        owner_prepare_failures: AtomicUsize,
        owner_commit_attempts: AtomicUsize,
        owner_commit_failures: AtomicUsize,
        owner_discards: AtomicUsize,
    }

    #[async_trait]
    impl DelegationHost for FakeHost {
        type Candidate = FakeCandidate;
        type PreparedDelegation = u64;
        type PreparedOwner = ();
        type RetiredAuthority = Option<u64>;

        async fn prepare_delegation(
            &self,
            _generation: u64,
            candidate: Self::Candidate,
            _cancellation: DelegationCancellation,
        ) -> Result<Self::PreparedDelegation, DelegationErrorCode> {
            tokio::time::sleep(candidate.prepare_delay).await;
            if let Some(error) = candidate.fail {
                return Err(error);
            }
            Ok(candidate.id)
        }

        async fn activate_delegation(
            &self,
            _generation: u64,
            prepared: &mut Self::PreparedDelegation,
            _cancellation: DelegationCancellation,
        ) -> Result<(), DelegationErrorCode> {
            let _ = prepared;
            tokio::time::sleep(candidate_activation_delay(*prepared)).await;
            Ok(())
        }

        fn commit_delegation(
            &self,
            _generation: u64,
            prepared: Self::PreparedDelegation,
        ) -> Result<Self::RetiredAuthority, CommitRejected<Self::PreparedDelegation>> {
            if prepared == 97 {
                return Err(CommitRejected::new(
                    prepared,
                    DelegationErrorCode::CommitRejected,
                ));
            }
            let previous = self
                .authority
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .replace(prepared);
            self.commits
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(prepared);
            Ok(previous)
        }

        async fn discard_delegation(&self, _generation: u64, prepared: Self::PreparedDelegation) {
            self.discards
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(prepared);
        }

        async fn prepare_owner(
            &self,
            _generation: u64,
            _reason: RestoreReason,
            _cancellation: DelegationCancellation,
        ) -> Result<Self::PreparedOwner, DelegationErrorCode> {
            self.owner_prepare_attempts
                .fetch_add(1, AtomicOrdering::SeqCst);
            if take_scripted_failure(&self.owner_prepare_failures) {
                return Err(DelegationErrorCode::OwnerRestoreFailed);
            }
            Ok(())
        }

        fn commit_owner(
            &self,
            _generation: u64,
            prepared: Self::PreparedOwner,
        ) -> Result<Self::RetiredAuthority, CommitRejected<Self::PreparedOwner>> {
            self.owner_commit_attempts
                .fetch_add(1, AtomicOrdering::SeqCst);
            if take_scripted_failure(&self.owner_commit_failures) {
                return Err(CommitRejected::new(
                    prepared,
                    DelegationErrorCode::OwnerRestoreFailed,
                ));
            }
            let previous = self
                .authority
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            Ok(previous)
        }

        async fn discard_owner(&self, _generation: u64, _prepared: Self::PreparedOwner) {
            self.owner_discards.fetch_add(1, AtomicOrdering::SeqCst);
        }

        async fn retire_authority(&self, _retired: Self::RetiredAuthority) {}

        async fn shutdown_authority(&self) {
            self.authority
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            *self
                .shutdowns
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
        }
    }

    fn candidate(id: u64) -> FakeCandidate {
        FakeCandidate {
            id,
            expires_at: unix_now_secs() + 3_600,
            prepare_delay: Duration::ZERO,
            fail: None,
        }
    }

    fn candidate_activation_delay(id: u64) -> Duration {
        if id == 99 {
            Duration::from_secs(60)
        } else {
            Duration::ZERO
        }
    }

    fn take_scripted_failure(counter: &AtomicUsize) -> bool {
        counter
            .fetch_update(
                AtomicOrdering::SeqCst,
                AtomicOrdering::SeqCst,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
    }

    fn coordinator(host: Arc<FakeHost>) -> DelegationCoordinator<FakeHost> {
        DelegationCoordinator::owner_ready(
            host,
            DelegationCoordinatorConfig {
                transaction_timeout: Duration::from_secs(2),
                cleanup_timeout: Duration::from_millis(100),
                restore_retry_backoff: Duration::from_millis(20),
            },
        )
    }

    async fn wait_for(
        receiver: &mut watch::Receiver<DelegationSnapshot>,
        predicate: impl Fn(DelegationSnapshot) -> bool,
    ) -> DelegationSnapshot {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = *receiver.borrow_and_update();
                if predicate(snapshot) {
                    return snapshot;
                }
                receiver.changed().await.unwrap();
            }
        })
        .await
        .expect("coordinator state transition timed out")
    }

    #[tokio::test]
    async fn latest_candidate_wins_without_late_commit() {
        let host = Arc::new(FakeHost::default());
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();
        let mut first = candidate(1);
        first.prepare_delay = Duration::from_millis(250);
        let first_generation = coordinator.admit(first).await.unwrap();
        let second_generation = coordinator.admit(candidate(2)).await.unwrap();

        let snapshot = wait_for(&mut states, |snapshot| {
            matches!(
                snapshot.phase,
                DelegationPhase::DelegatedActive { generation }
                    if generation == second_generation
            )
        })
        .await;
        assert!(matches!(
            snapshot.credential_origin,
            CredentialOrigin::Delegated { generation, .. }
                if generation == second_generation
        ));
        assert_ne!(first_generation, second_generation);
        assert_eq!(
            host.commits
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[2]
        );
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn failed_replacement_preserves_active_authority() {
        let host = Arc::new(FakeHost::default());
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();
        let first_generation = coordinator.admit(candidate(1)).await.unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(
                snapshot.phase,
                DelegationPhase::DelegatedActive { generation }
                    if generation == first_generation
            )
        })
        .await;

        let mut rejected = candidate(2);
        rejected.fail = Some(DelegationErrorCode::ApiRejected);
        coordinator.admit(rejected).await.unwrap();
        let snapshot = wait_for(&mut states, |snapshot| {
            snapshot.last_error == Some(DelegationErrorCode::ApiRejected)
                && snapshot.candidate_generation.is_none()
        })
        .await;
        assert!(matches!(
            snapshot.credential_origin,
            CredentialOrigin::Delegated { generation, .. }
                if generation == first_generation
        ));
        assert_eq!(
            *host
                .authority
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some(1)
        );
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_during_activation_prevents_commit() {
        let host = Arc::new(FakeHost::default());
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();
        coordinator.admit(candidate(99)).await.unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(snapshot.phase, DelegationPhase::Activating { .. })
        })
        .await;

        coordinator.shutdown().await;
        assert_eq!(coordinator.snapshot().phase, DelegationPhase::Disabled);
        assert!(host
            .commits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        assert_eq!(
            *host
                .shutdowns
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            1
        );
    }

    #[tokio::test]
    async fn explicit_restore_is_generation_guarded() {
        let host = Arc::new(FakeHost::default());
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();
        let generation = coordinator.admit(candidate(7)).await.unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(
                snapshot.phase,
                DelegationPhase::DelegatedActive { generation: active }
                    if active == generation
            )
        })
        .await;

        assert!(!coordinator
            .restore_owner_if_active(generation + 1, RestoreReason::TransportFatal)
            .await
            .unwrap());
        assert!(coordinator
            .restore_owner_if_active(generation, RestoreReason::TransportFatal)
            .await
            .unwrap());
        let snapshot = wait_for(&mut states, |snapshot| {
            snapshot.phase == DelegationPhase::OwnerReady
        })
        .await;
        assert_eq!(snapshot.credential_origin, CredentialOrigin::Owner);
        assert_eq!(
            *host
                .authority
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            None
        );
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn commit_rejection_discards_candidate_without_swapping_owner() {
        let host = Arc::new(FakeHost::default());
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();

        coordinator.admit(candidate(97)).await.unwrap();
        let snapshot = wait_for(&mut states, |snapshot| {
            snapshot.last_error == Some(DelegationErrorCode::CommitRejected)
        })
        .await;

        assert_eq!(snapshot.phase, DelegationPhase::OwnerReady);
        assert_eq!(snapshot.credential_origin, CredentialOrigin::Owner);
        assert_eq!(
            host.discards
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[97]
        );
        assert!(host
            .commits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        coordinator.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn delegated_credential_expiry_restores_owner_automatically() {
        let host = Arc::new(FakeHost::default());
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();
        let mut expiring = candidate(8);
        expiring.expires_at = unix_now_secs() + 1;
        let generation = coordinator.admit(expiring).await.unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(
                snapshot.phase,
                DelegationPhase::DelegatedActive { generation: active }
                    if active == generation
            )
        })
        .await;

        tokio::time::advance(Duration::from_secs(2)).await;
        let snapshot = wait_for(&mut states, |snapshot| {
            snapshot.phase == DelegationPhase::OwnerReady
        })
        .await;

        assert_eq!(snapshot.credential_origin, CredentialOrigin::Owner);
        assert_eq!(host.owner_prepare_attempts.load(AtomicOrdering::SeqCst), 1);
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn failed_owner_prepare_retries_until_restore_succeeds() {
        let host = Arc::new(FakeHost::default());
        host.owner_prepare_failures.store(1, AtomicOrdering::SeqCst);
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();
        let generation = coordinator.admit(candidate(9)).await.unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(snapshot.phase, DelegationPhase::DelegatedActive { .. })
        })
        .await;

        coordinator
            .restore_owner_if_active(generation, RestoreReason::TransportFatal)
            .await
            .unwrap();
        wait_for(&mut states, |snapshot| {
            snapshot.phase == DelegationPhase::OwnerReady
        })
        .await;

        assert_eq!(host.owner_prepare_attempts.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(host.owner_commit_attempts.load(AtomicOrdering::SeqCst), 1);
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn failed_owner_commit_discards_and_retries() {
        let host = Arc::new(FakeHost::default());
        host.owner_commit_failures.store(1, AtomicOrdering::SeqCst);
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();
        let generation = coordinator.admit(candidate(10)).await.unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(snapshot.phase, DelegationPhase::DelegatedActive { .. })
        })
        .await;

        coordinator
            .restore_owner_if_active(generation, RestoreReason::TransportFatal)
            .await
            .unwrap();
        wait_for(&mut states, |snapshot| {
            snapshot.phase == DelegationPhase::OwnerReady
        })
        .await;

        assert_eq!(host.owner_commit_attempts.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(host.owner_discards.load(AtomicOrdering::SeqCst), 1);
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn failed_candidate_after_cancelling_restore_resumes_restore() {
        let host = Arc::new(FakeHost::default());
        host.owner_prepare_failures
            .store(100, AtomicOrdering::SeqCst);
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();
        let generation = coordinator.admit(candidate(11)).await.unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(snapshot.phase, DelegationPhase::DelegatedActive { .. })
        })
        .await;
        coordinator
            .restore_owner_if_active(generation, RestoreReason::TransportFatal)
            .await
            .unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(snapshot.phase, DelegationPhase::RestoringOwner { .. })
        })
        .await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while host.owner_prepare_attempts.load(AtomicOrdering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first restore attempt did not start");

        let mut rejected = candidate(12);
        rejected.prepare_delay = Duration::from_millis(50);
        rejected.fail = Some(DelegationErrorCode::ApiRejected);
        coordinator.admit(rejected).await.unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(snapshot.phase, DelegationPhase::CandidateValidating { .. })
        })
        .await;
        host.owner_prepare_failures.store(0, AtomicOrdering::SeqCst);

        let snapshot = wait_for(&mut states, |snapshot| {
            snapshot.phase == DelegationPhase::OwnerReady
        })
        .await;
        assert_eq!(snapshot.credential_origin, CredentialOrigin::Owner);
        assert!(host.owner_prepare_attempts.load(AtomicOrdering::SeqCst) >= 2);
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_shutdown_is_idempotent_and_cannot_be_reopened_midflight() {
        let host = Arc::new(FakeHost::default());
        let coordinator = coordinator(Arc::clone(&host));
        let first = coordinator.clone();
        let second = coordinator.clone();

        tokio::join!(first.shutdown(), second.shutdown());

        assert_eq!(coordinator.snapshot().phase, DelegationPhase::Disabled);
        assert_eq!(
            *host
                .shutdowns
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            1
        );
    }

    #[tokio::test]
    async fn declare_owner_ready_is_compare_and_set() {
        let host = Arc::new(FakeHost::default());
        let coordinator = DelegationCoordinator::disabled(
            Arc::clone(&host),
            DelegationCoordinatorConfig::default(),
        );

        assert!(coordinator.declare_owner_ready().await);
        assert!(!coordinator.declare_owner_ready().await);
        let mut states = coordinator.subscribe();
        coordinator.admit(candidate(13)).await.unwrap();
        wait_for(&mut states, |snapshot| {
            matches!(snapshot.phase, DelegationPhase::DelegatedActive { .. })
        })
        .await;
        assert!(!coordinator.declare_owner_ready().await);
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn declare_owner_ready_revalidates_external_intent_inside_commit_gate() {
        let host = Arc::new(FakeHost::default());
        let coordinator = DelegationCoordinator::disabled(
            Arc::clone(&host),
            DelegationCoordinatorConfig::default(),
        );

        assert!(!coordinator.declare_owner_ready_if(|| false).await);
        assert_eq!(coordinator.snapshot().phase, DelegationPhase::Disabled);
        assert!(coordinator.declare_owner_ready_if(|| true).await);
        assert_eq!(coordinator.snapshot().phase, DelegationPhase::OwnerReady);
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn published_snapshot_revisions_are_strictly_monotonic() {
        let host = Arc::new(FakeHost::default());
        let coordinator = coordinator(Arc::clone(&host));
        let mut states = coordinator.subscribe();
        let mut previous = states.borrow().revision;
        let mut delayed = candidate(14);
        delayed.prepare_delay = Duration::from_millis(20);
        coordinator.admit(delayed).await.unwrap();

        loop {
            states.changed().await.unwrap();
            let snapshot = *states.borrow_and_update();
            assert!(snapshot.revision > previous);
            previous = snapshot.revision;
            if matches!(snapshot.phase, DelegationPhase::DelegatedActive { .. }) {
                break;
            }
        }
        coordinator.shutdown().await;
        assert!(coordinator.snapshot().revision > previous);
    }
}
