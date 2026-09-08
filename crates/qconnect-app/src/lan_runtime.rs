//! Shared physical lifecycle for the blocking QConnect LAN receiver adapters.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::{AuthorityCell, AuthorityStamp, QconnectEnableIntent};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const START_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// A listener outlives the one-shot enable epoch that created it, but never
/// the current enabled intent or the exact authority runtime it projects.
pub fn lan_callback_is_current(
    enable_intent: &QconnectEnableIntent,
    authority: &AuthorityCell,
    stamp: AuthorityStamp,
) -> bool {
    enable_intent.current_token().is_some() && authority.is_current(stamp)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LanRuntimeError {
    #[error("qconnect-lan-physical-teardown-unsafe")]
    PhysicalTeardownUnsafe,
    #[error("qconnect-lan-start-already-pending")]
    StartAlreadyPending,
    #[error("qconnect-lan-disabled")]
    Cancelled,
    #[error("qconnect-lan-bind-failed")]
    BindFailed,
    #[error("qconnect-lan-start-task-failed")]
    StartTaskFailed,
    #[error("qconnect-lan-shutdown-task-failed")]
    ShutdownTaskFailed,
    #[error("qconnect-lan-shutdown-timed-out")]
    ShutdownTimedOut,
    #[error("qconnect-lan-start-cleanup-timed-out")]
    StartCleanupTimedOut,
}

/// Owns the pending-start fence and the irreversible unsafe-teardown latch.
///
/// Qt and qbzd supply only their runtime constructor and blocking shutdown
/// primitive. Cancellation cleanup, worker joining, timeouts and fail-closed
/// latching are identical and remain tested in this shared implementation.
pub struct LanRuntimeLifecycle<R> {
    start_pending: Arc<AtomicBool>,
    teardown_unsafe: Arc<AtomicBool>,
    shutdown: Arc<dyn Fn(&mut R) + Send + Sync + 'static>,
}

impl<R> Clone for LanRuntimeLifecycle<R> {
    fn clone(&self) -> Self {
        Self {
            start_pending: Arc::clone(&self.start_pending),
            teardown_unsafe: Arc::clone(&self.teardown_unsafe),
            shutdown: Arc::clone(&self.shutdown),
        }
    }
}

impl<R> LanRuntimeLifecycle<R>
where
    R: Send + 'static,
{
    pub fn new(shutdown: impl Fn(&mut R) + Send + Sync + 'static) -> Self {
        Self {
            start_pending: Arc::new(AtomicBool::new(false)),
            teardown_unsafe: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(shutdown),
        }
    }

    pub fn start_pending(&self) -> bool {
        self.start_pending.load(Ordering::Acquire)
    }

    pub fn teardown_safe(&self) -> bool {
        !self.teardown_unsafe.load(Ordering::Acquire)
    }

    pub async fn start<E, S, C>(&self, start: S, cancellation: C) -> Result<R, LanRuntimeError>
    where
        E: Send + 'static,
        S: FnOnce() -> Result<R, E> + Send + 'static,
        C: Future<Output = ()>,
    {
        if !self.teardown_safe() {
            return Err(LanRuntimeError::PhysicalTeardownUnsafe);
        }
        if self.start_pending.swap(true, Ordering::AcqRel) {
            return Err(LanRuntimeError::StartAlreadyPending);
        }

        let mut start_task = tokio::task::spawn_blocking(start);
        let result = tokio::select! {
            biased;
            _ = cancellation => {
                // A running blocking task cannot be aborted. Its supervisor
                // retains ownership until any resulting listener is withdrawn.
                let lifecycle = self.clone();
                tokio::spawn(async move {
                    match start_task.await {
                        Ok(Ok(runtime)) => {
                            if let Err(error) = lifecycle.shutdown(runtime).await {
                                log::warn!(
                                    "[QConnect LAN] cancelled listener cleanup failed: {error}"
                                );
                            }
                        }
                        Ok(Err(_)) => {}
                        Err(error) => {
                            log::warn!(
                                "[QConnect LAN] cancelled listener start worker failed: {error}"
                            );
                            lifecycle.teardown_unsafe.store(true, Ordering::Release);
                        }
                    }
                    lifecycle.start_pending.store(false, Ordering::Release);
                });
                return Err(LanRuntimeError::Cancelled);
            }
            result = &mut start_task => result,
        };
        self.start_pending.store(false, Ordering::Release);

        match result {
            Ok(Ok(runtime)) => Ok(runtime),
            Ok(Err(_)) => Err(LanRuntimeError::BindFailed),
            Err(_) => {
                self.teardown_unsafe.store(true, Ordering::Release);
                Err(LanRuntimeError::StartTaskFailed)
            }
        }
    }

    /// Run physical teardown on the blocking pool. Any lost worker or timeout
    /// latches the lifecycle unsafe so logout/disable cannot claim success.
    pub async fn shutdown(&self, mut runtime: R) -> Result<(), LanRuntimeError> {
        let shutdown_fn = Arc::clone(&self.shutdown);
        let mut shutdown = tokio::task::spawn_blocking(move || shutdown_fn(&mut runtime));
        let result = match tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut shutdown).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(LanRuntimeError::ShutdownTaskFailed),
            Err(_) => {
                // The detached worker keeps owning the runtime and may finish
                // its idempotent shutdown, but the process can no longer prove
                // that the listener and mDNS publication are gone.
                drop(shutdown);
                Err(LanRuntimeError::ShutdownTimedOut)
            }
        };
        if result.is_err() {
            self.teardown_unsafe.store(true, Ordering::Release);
        }
        result
    }

    /// Wait for cancellation cleanup to take ownership and finish, then prove
    /// that no physical listener is left in an unknown state.
    pub async fn settle(&self) -> Result<(), LanRuntimeError> {
        if self.start_pending() {
            let settled = tokio::time::timeout(START_CLEANUP_TIMEOUT, async {
                while self.start_pending() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .is_ok();
            if !settled {
                return Err(LanRuntimeError::StartCleanupTimedOut);
            }
        }
        if !self.teardown_safe() {
            return Err(LanRuntimeError::PhysicalTeardownUnsafe);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::AuthorityOrigin;

    #[test]
    fn callback_survives_intent_refresh_but_not_disable_or_authority_change() {
        let intent = QconnectEnableIntent::new(true);
        let original = intent.current_token().unwrap();
        let authority = AuthorityCell::new();
        let stamp = authority.reserve(AuthorityOrigin::Owner);
        assert!(authority.install(stamp));
        assert!(lan_callback_is_current(&intent, &authority, stamp));

        let refreshed = intent.enable_new_intent();
        assert!(!intent.is_current(original));
        assert!(intent.is_current(refreshed));
        assert!(lan_callback_is_current(&intent, &authority, stamp));

        let disabled = intent.disable();
        assert!(!lan_callback_is_current(&intent, &authority, stamp));
        assert!(intent.enable_if_disabled(disabled).is_some());
        assert!(lan_callback_is_current(&intent, &authority, stamp));

        authority.clear();
        assert!(!lan_callback_is_current(&intent, &authority, stamp));
    }

    #[tokio::test]
    async fn successful_start_and_shutdown_share_one_physical_owner() {
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&shutdowns);
        let lifecycle = LanRuntimeLifecycle::new(move |_runtime: &mut u8| {
            observed.fetch_add(1, Ordering::AcqRel);
        });

        let runtime = lifecycle
            .start(|| Ok::<_, ()>(7), std::future::pending())
            .await
            .unwrap();
        assert_eq!(runtime, 7);
        assert!(!lifecycle.start_pending());
        lifecycle.shutdown(runtime).await.unwrap();
        assert_eq!(shutdowns.load(Ordering::Acquire), 1);
        assert!(lifecycle.teardown_safe());
    }

    #[tokio::test]
    async fn cancellation_supervisor_withdraws_late_listener_before_settling() {
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&shutdowns);
        let lifecycle = LanRuntimeLifecycle::new(move |_runtime: &mut u8| {
            observed.fetch_add(1, Ordering::AcqRel);
        });

        assert_eq!(
            lifecycle
                .start(
                    || {
                        std::thread::sleep(Duration::from_millis(25));
                        Ok::<_, ()>(1)
                    },
                    std::future::ready(()),
                )
                .await,
            Err(LanRuntimeError::Cancelled)
        );
        assert!(lifecycle.start_pending());
        lifecycle.settle().await.unwrap();
        assert_eq!(shutdowns.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn panicked_shutdown_latches_fail_closed() {
        let lifecycle = LanRuntimeLifecycle::new(|_runtime: &mut u8| panic!("shutdown failed"));
        assert_eq!(
            lifecycle.shutdown(1).await,
            Err(LanRuntimeError::ShutdownTaskFailed)
        );
        assert!(!lifecycle.teardown_safe());
        assert_eq!(
            lifecycle.settle().await,
            Err(LanRuntimeError::PhysicalTeardownUnsafe)
        );
    }
}
