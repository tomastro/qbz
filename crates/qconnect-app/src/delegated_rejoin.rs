//! Shared reconnect watchdog for an installed delegated QConnect runtime.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use qconnect_transport_ws::TransportEvent;

use crate::{AuthorityCell, AuthorityOrigin, AuthorityStamp};

const DELEGATED_REJOIN_SESSION_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegatedRuntimeEventDirective {
    Forward,
    Reconnecting,
    Rejoin,
    Connected,
    RestoreOwner,
}

/// Minimal shared reconnect reducer. Adapters execute their concrete lifecycle
/// projection and report I/O, then forward non-fatal events to their app.
#[derive(Debug, Default)]
pub struct DelegatedRuntimeEventState {
    disconnected: bool,
}

impl DelegatedRuntimeEventState {
    pub fn observe(&mut self, event: &TransportEvent) -> DelegatedRuntimeEventDirective {
        match event {
            TransportEvent::Disconnected => {
                self.disconnected = true;
                DelegatedRuntimeEventDirective::Reconnecting
            }
            TransportEvent::Subscribed if self.disconnected => {
                DelegatedRuntimeEventDirective::Rejoin
            }
            TransportEvent::SessionEstablished => {
                self.disconnected = false;
                DelegatedRuntimeEventDirective::Connected
            }
            TransportEvent::CloudError { .. }
            | TransportEvent::MaxReconnectAttemptsExceeded { .. } => {
                DelegatedRuntimeEventDirective::RestoreOwner
            }
            _ => DelegatedRuntimeEventDirective::Forward,
        }
    }
}

#[derive(Default)]
struct RejoinWatchdogGeneration(AtomicU64);

impl RejoinWatchdogGeneration {
    fn advance(&self) -> u64 {
        self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    fn claim(&self, expected: u64) -> bool {
        self.0
            .compare_exchange(
                expected,
                expected.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

/// Restores owner authority if a delegated runtime fails to re-establish its
/// session within the official receiver budget. Re-arm, cancellation, runtime
/// replacement and drop all invalidate the prior deadline before aborting it.
pub struct DelegatedRejoinWatchdog {
    generation: Arc<RejoinWatchdogGeneration>,
    task: Option<tokio::task::JoinHandle<()>>,
    timeout: Duration,
}

impl DelegatedRejoinWatchdog {
    pub fn new() -> Self {
        Self::with_timeout(DELEGATED_REJOIN_SESSION_TIMEOUT)
    }

    fn with_timeout(timeout: Duration) -> Self {
        Self {
            generation: Arc::new(RejoinWatchdogGeneration::default()),
            task: None,
            timeout,
        }
    }

    pub fn arm<F, Fut>(
        &mut self,
        authority: Arc<AuthorityCell>,
        stamp: AuthorityStamp,
        delegation_generation: u64,
        restore: F,
    ) where
        F: FnOnce(u64) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        // Advance before aborting so even a task already waking at the
        // deadline loses its compare-exchange against this newer rejoin.
        let watchdog_generation = self.generation.advance();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let generation = Arc::clone(&self.generation);
        let timeout = self.timeout;
        self.task = Some(tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            if !generation.claim(watchdog_generation)
                || stamp.origin()
                    != (AuthorityOrigin::Delegated {
                        generation: delegation_generation,
                    })
                || !authority.is_current(stamp)
            {
                return;
            }
            restore(delegation_generation).await;
        }));
    }

    pub fn cancel(&mut self) {
        // Invalidate first: abort is cooperative and may race a waking task.
        self.generation.advance();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Default for DelegatedRejoinWatchdog {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DelegatedRejoinWatchdog {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn runtime_event_state_rejoins_only_after_disconnect_and_fails_closed_on_fatal() {
        let mut state = DelegatedRuntimeEventState::default();
        assert_eq!(
            state.observe(&TransportEvent::Subscribed),
            DelegatedRuntimeEventDirective::Forward
        );
        assert_eq!(
            state.observe(&TransportEvent::Disconnected),
            DelegatedRuntimeEventDirective::Reconnecting
        );
        assert_eq!(
            state.observe(&TransportEvent::Subscribed),
            DelegatedRuntimeEventDirective::Rejoin
        );
        assert_eq!(
            state.observe(&TransportEvent::SessionEstablished),
            DelegatedRuntimeEventDirective::Connected
        );
        assert_eq!(
            state.observe(&TransportEvent::Subscribed),
            DelegatedRuntimeEventDirective::Forward
        );
        assert_eq!(
            state.observe(&TransportEvent::MaxReconnectAttemptsExceeded {
                attempts: 3,
                last_reason: "sanitized".to_string(),
            }),
            DelegatedRuntimeEventDirective::RestoreOwner
        );
    }

    #[test]
    fn generation_is_claimed_once_and_replacement_invalidates_prior_arm() {
        let generation = RejoinWatchdogGeneration::default();
        let first = generation.advance();
        assert!(generation.claim(first));
        assert!(!generation.claim(first));

        let cancelled = generation.advance();
        generation.advance();
        let replacement = generation.advance();
        assert!(!generation.claim(cancelled));
        assert!(generation.claim(replacement));
    }

    #[tokio::test(start_paused = true)]
    async fn fires_once_only_for_the_exact_installed_delegation() {
        let authority = Arc::new(AuthorityCell::new());
        let stamp = authority.reserve(AuthorityOrigin::Delegated { generation: 7 });
        assert!(authority.install(stamp));
        let restores = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&restores);
        let mut watchdog = DelegatedRejoinWatchdog::with_timeout(Duration::from_secs(1));
        watchdog.arm(Arc::clone(&authority), stamp, 7, move |_| async move {
            observed.fetch_add(1, Ordering::AcqRel);
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(restores.load(Ordering::Acquire), 1);

        let stale_observed = Arc::clone(&restores);
        watchdog.arm(Arc::clone(&authority), stamp, 7, move |_| async move {
            stale_observed.fetch_add(1, Ordering::AcqRel);
        });
        let replacement = authority.reserve(AuthorityOrigin::Owner);
        assert!(authority.install(replacement));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(restores.load(Ordering::Acquire), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_invalidates_a_ready_deadline_before_aborting() {
        let authority = Arc::new(AuthorityCell::new());
        let stamp = authority.reserve(AuthorityOrigin::Delegated { generation: 9 });
        assert!(authority.install(stamp));
        let restores = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&restores);
        let mut watchdog = DelegatedRejoinWatchdog::with_timeout(Duration::from_secs(1));
        watchdog.arm(authority, stamp, 9, move |_| async move {
            observed.fetch_add(1, Ordering::AcqRel);
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        watchdog.cancel();
        tokio::task::yield_now().await;
        assert_eq!(restores.load(Ordering::Acquire), 0);
    }
}
