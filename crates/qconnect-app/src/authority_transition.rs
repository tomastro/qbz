//! Shared release ordering for QConnect authority transitions.

use std::sync::Arc;

use tokio::sync::{oneshot, Mutex, OwnedMutexGuard};

use crate::{AuthorityCell, AuthorityStamp, DelegationErrorCode};

/// RAII fence that suspends owner actions until the transition releases it.
pub struct OwnerActionFence {
    authority: Arc<AuthorityCell>,
}

impl OwnerActionFence {
    pub fn acquire(authority: Arc<AuthorityCell>) -> Self {
        authority.suspend_owner_actions();
        Self { authority }
    }

    /// Close admission, perform adapter-specific cancellation, then wait for
    /// all action permits admitted before the fence to drain.
    pub async fn acquire_drained(
        authority: Arc<AuthorityCell>,
        after_fence: impl FnOnce(),
    ) -> Self {
        let fence = Self::acquire(authority);
        after_fence();
        fence.authority.wait_for_actions_drained().await;
        fence
    }
}

impl Drop for OwnerActionFence {
    fn drop(&mut self) {
        self.authority.resume_owner_actions();
    }
}

/// Acquire the host transition lane before closing ordinary action admission,
/// then revalidate the exact authority on both sides of the drain. This order
/// is shared because a stale waiter must not briefly fence its replacement.
pub async fn acquire_transition_guard_and_fence(
    transition_gate: Arc<Mutex<()>>,
    authority: Arc<AuthorityCell>,
    expected: AuthorityStamp,
    after_fence: impl FnOnce(),
) -> Result<(OwnedMutexGuard<()>, OwnerActionFence), DelegationErrorCode> {
    let guard = transition_gate.lock_owned().await;
    if !authority.is_current(expected) {
        return Err(DelegationErrorCode::CandidateCancelled);
    }
    let fence = OwnerActionFence::acquire_drained(Arc::clone(&authority), after_fence).await;
    if !authority.is_current(expected) {
        return Err(DelegationErrorCode::CandidateCancelled);
    }
    Ok((guard, fence))
}

/// Cancellation-safe final release edge for an installed authority.
///
/// Admission always opens before the prepared loop wakes, and the transition
/// lane unlocks last. `Drop` preserves the same order after cancellation.
pub struct DeferredActivationRelease {
    start: Option<oneshot::Sender<()>>,
    transition_fence: Option<OwnerActionFence>,
    transition_guard: Option<OwnedMutexGuard<()>>,
}

impl DeferredActivationRelease {
    pub fn new(
        start: Option<oneshot::Sender<()>>,
        transition_fence: Option<OwnerActionFence>,
        transition_guard: Option<OwnedMutexGuard<()>>,
    ) -> Self {
        Self {
            start,
            transition_fence,
            transition_guard,
        }
    }

    pub fn wake_runtime(&mut self) {
        self.transition_fence.take();
        if let Some(start) = self.start.take() {
            let _ = start.send(());
        }
    }

    pub fn unlock_transition(&mut self) {
        self.transition_guard.take();
    }

    pub fn release(&mut self) {
        self.wake_runtime();
        self.unlock_transition();
    }
}

impl Drop for DeferredActivationRelease {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::AuthorityOrigin;

    #[tokio::test]
    async fn fence_closes_before_adapter_cancellation_and_waits_for_live_actions() {
        let authority = Arc::new(AuthorityCell::new());
        let permit = Arc::clone(&authority).try_owner_action_permit().unwrap();
        let cancellation_saw_closed = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&cancellation_saw_closed);
        let task_authority = Arc::clone(&authority);
        let fence_task = tokio::spawn(async move {
            OwnerActionFence::acquire_drained(Arc::clone(&task_authority), || {
                observed.store(!task_authority.owner_actions_allowed(), Ordering::Release);
            })
            .await
        });

        tokio::task::yield_now().await;
        assert!(cancellation_saw_closed.load(Ordering::Acquire));
        assert!(!fence_task.is_finished());
        drop(permit);
        let fence = fence_task.await.unwrap();
        assert!(!authority.owner_actions_allowed());
        drop(fence);
        assert!(authority.owner_actions_allowed());
    }

    #[tokio::test]
    async fn runtime_wakes_only_after_owner_admission_reopens() {
        let authority = Arc::new(AuthorityCell::new());
        let fence = OwnerActionFence::acquire(Arc::clone(&authority));
        let (start, started) = oneshot::channel();
        let mut release = DeferredActivationRelease::new(Some(start), Some(fence), None);

        release.wake_runtime();
        started.await.unwrap();
        assert!(authority.owner_actions_allowed());
    }

    #[tokio::test]
    async fn transition_gate_precedes_fence_and_stale_waiter_never_closes_admission() {
        let authority = Arc::new(AuthorityCell::new());
        let stamp = authority.reserve(AuthorityOrigin::Owner);
        assert!(authority.install(stamp));
        let gate = Arc::new(Mutex::new(()));
        let held_gate = Arc::clone(&gate).lock_owned().await;
        let task_gate = Arc::clone(&gate);
        let task_authority = Arc::clone(&authority);
        let task = tokio::spawn(async move {
            acquire_transition_guard_and_fence(task_gate, task_authority, stamp, || {}).await
        });

        tokio::task::yield_now().await;
        assert!(authority.owner_actions_allowed());
        assert!(!task.is_finished());

        drop(held_gate);
        let (guard, fence) = task.await.unwrap().unwrap();
        assert!(!authority.owner_actions_allowed());
        drop(fence);
        drop(guard);
        assert!(authority.owner_actions_allowed());

        let stale = stamp;
        let replacement = authority.reserve(AuthorityOrigin::Owner);
        assert!(authority.install(replacement));
        let callback_ran = Arc::new(AtomicBool::new(false));
        let callback_observer = Arc::clone(&callback_ran);
        assert!(matches!(
            acquire_transition_guard_and_fence(gate, Arc::clone(&authority), stale, move || {
                callback_observer.store(true, Ordering::Release)
            },)
            .await,
            Err(DelegationErrorCode::CandidateCancelled)
        ));
        assert!(!callback_ran.load(Ordering::Acquire));
        assert!(authority.owner_actions_allowed());
    }
}
