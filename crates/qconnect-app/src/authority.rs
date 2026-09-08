//! Exact authority stamps for QConnect runtimes.
//!
//! A stamp is reserved while a replacement runtime is being prepared, but it
//! does not become authoritative until [`AuthorityCell::install`] succeeds.
//! Long-lived tasks capture that exact stamp and re-check it immediately before
//! publishing or mutating shared state. This prevents a retired owner or guest
//! runtime from acting on a replacement runtime after an asynchronous callback
//! completes late.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::Notify;

/// One process-local enable epoch for the global QConnect lifecycle.
///
/// A token is intentionally opaque: adapters may carry it across network
/// awaits, but every authority/runtime/LAN publication must revalidate it at a
/// short synchronous commit boundary. `disable()` advances the epoch before it
/// starts any asynchronous teardown, so work that finishes late cannot publish
/// a replacement runtime or resurrect LAN discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QconnectEnableToken(u64);

/// Exact disabled intent produced by one teardown boundary. Automatic restore
/// may re-enable only while this remains the latest intent; a later manual
/// enable or repeated disable invalidates the token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QconnectDisabledToken(u64);

#[derive(Debug, Clone, Copy)]
struct QconnectEnableState {
    enabled: bool,
    epoch: u64,
}

/// Synchronous latest-intent gate for enable/disable versus long-running
/// connect work.
///
/// The mutex is never held across an await. Callers may use
/// [`commit_if_current`](Self::commit_if_current) only for a bounded in-memory
/// install (authority stamp, runtime slot or LAN slot). This lets disable win
/// immediately without waiting behind DNS, HTTP, QWS or API I/O.
#[derive(Debug)]
pub struct QconnectEnableIntent {
    state: Mutex<QconnectEnableState>,
    changed: Notify,
}

impl QconnectEnableIntent {
    pub fn new(enabled: bool) -> Self {
        Self {
            state: Mutex::new(QconnectEnableState { enabled, epoch: 0 }),
            changed: Notify::new(),
        }
    }

    /// Express enabled intent and return its stable epoch. Repeated enable
    /// requests are idempotent and therefore do not invalidate an in-flight
    /// connect for the same user intent.
    pub fn enable(&self) -> QconnectEnableToken {
        let mut state = self.lock_enable_state();
        if !state.enabled {
            state.epoch = next_enable_epoch(state.epoch);
            state.enabled = true;
        }
        QconnectEnableToken(state.epoch)
    }

    /// Express a distinct enabled intent even when QConnect was already
    /// logically enabled. Interactive connect entrypoints use this so a fresh
    /// user click supersedes an automatic restore that has not installed a
    /// runtime yet; retry loops should retain the token returned by `enable()`
    /// or this method instead of minting one per attempt.
    pub fn enable_new_intent(&self) -> QconnectEnableToken {
        let mut state = self.lock_enable_state();
        state.epoch = next_enable_epoch(state.epoch);
        state.enabled = true;
        self.changed.notify_waiters();
        QconnectEnableToken(state.epoch)
    }

    /// Invalidate every token issued before this disable boundary. The epoch is
    /// advanced even for an idempotent repeated disable, so a later-invoked
    /// shutdown always wins over older work.
    pub fn disable(&self) -> QconnectDisabledToken {
        let mut state = self.lock_enable_state();
        state.epoch = next_enable_epoch(state.epoch);
        state.enabled = false;
        // Notify while the state mutex is still held. `cancelled()` registers
        // its waiter before inspecting this same state, which closes both
        // possible missed-wake windows around the epoch check.
        self.changed.notify_waiters();
        QconnectDisabledToken(state.epoch)
    }

    /// Atomically disable one exact enabled intent. This is the inverse of
    /// [`enable_if_disabled`](Self::enable_if_disabled): a renderer handoff
    /// may replace a consumed restore token without disabling a newer manual
    /// enable that won the race.
    pub fn disable_if_current(&self, token: QconnectEnableToken) -> Option<QconnectDisabledToken> {
        let mut state = self.lock_enable_state();
        if !state.enabled || state.epoch != token.0 {
            return None;
        }
        state.epoch = next_enable_epoch(state.epoch);
        state.enabled = false;
        self.changed.notify_waiters();
        Some(QconnectDisabledToken(state.epoch))
    }

    /// Atomically enable only from one exact disabled boundary. On success the
    /// epoch advances, transferring restore ownership to the returned enabled
    /// token. A stale automatic restore can never overwrite newer user intent.
    pub fn enable_if_disabled(&self, token: QconnectDisabledToken) -> Option<QconnectEnableToken> {
        let mut state = self.lock_enable_state();
        if state.enabled || state.epoch != token.0 {
            return None;
        }
        state.epoch = next_enable_epoch(state.epoch);
        state.enabled = true;
        Some(QconnectEnableToken(state.epoch))
    }

    pub fn is_disabled_current(&self, token: QconnectDisabledToken) -> bool {
        let state = self.lock_enable_state();
        !state.enabled && state.epoch == token.0
    }

    pub fn current_token(&self) -> Option<QconnectEnableToken> {
        let state = self.lock_enable_state();
        state.enabled.then_some(QconnectEnableToken(state.epoch))
    }

    pub fn is_current(&self, token: QconnectEnableToken) -> bool {
        let state = self.lock_enable_state();
        state.enabled && state.epoch == token.0
    }

    /// Wait until `token` no longer represents enabled intent.
    ///
    /// Registration is deliberately enabled before the state check. If a
    /// disable wins first, the state check returns immediately; if it wins
    /// after registration, the notification is retained for the await.
    pub async fn cancelled(&self, token: QconnectEnableToken) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_current(token) {
                return;
            }
            notified.await;
        }
    }

    /// Run one short, non-async publication only if `token` still represents
    /// the latest enabled intent. Disable is serialized against the closure, so
    /// either the publication wins first and teardown observes it, or disable
    /// wins first and the publication is rejected.
    pub fn commit_if_current<R>(
        &self,
        token: QconnectEnableToken,
        commit: impl FnOnce() -> R,
    ) -> Option<R> {
        let state = self.lock_enable_state();
        if !state.enabled || state.epoch != token.0 {
            return None;
        }
        Some(commit())
    }

    /// Publish a short disabled-state side effect only while no newer enable
    /// has won. This is the inverse commit boundary used by status mirrors:
    /// either the disabled publication completes first and a later enable
    /// overwrites it, or the newer enable rejects the stale publication.
    pub fn commit_if_disabled<R>(&self, commit: impl FnOnce() -> R) -> Option<R> {
        let state = self.lock_enable_state();
        if state.enabled {
            return None;
        }
        Some(commit())
    }

    fn lock_enable_state(&self) -> MutexGuard<'_, QconnectEnableState> {
        self.state.lock().unwrap_or_else(|poisoned| {
            log::error!("[QConnect] enable-intent gate recovered from a poisoned lock");
            let guard = poisoned.into_inner();
            self.state.clear_poison();
            guard
        })
    }
}

fn next_enable_epoch(epoch: u64) -> u64 {
    epoch
        .checked_add(1)
        .expect("QConnect enable-intent epoch space exhausted")
}

/// Credential authority represented by a runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuthorityOrigin {
    /// The daemon's authenticated account.
    Owner,
    /// A controller-delegated identity, scoped to one coordinator generation.
    Delegated { generation: u64 },
}

impl AuthorityOrigin {
    pub const fn delegated_generation(self) -> Option<u64> {
        match self {
            Self::Owner => None,
            Self::Delegated { generation } => Some(generation),
        }
    }
}

/// Unforgeable-in-module identity for one prepared/installed runtime.
///
/// The epoch distinguishes repeated owner runtimes as well as delegated
/// generations. Checking only `Owner` would let an old owner's reconnect task
/// mutate a freshly restored owner runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AuthorityStamp {
    epoch: u64,
    origin: AuthorityOrigin,
}

impl AuthorityStamp {
    pub const fn epoch(self) -> u64 {
        self.epoch
    }

    pub const fn origin(self) -> AuthorityOrigin {
        self.origin
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct AuthorityState {
    current: Option<AuthorityStamp>,
    /// Monotonic fence for installs. A clear advances this even while no runtime
    /// is active, invalidating candidates reserved before the clear.
    installed_through_epoch: u64,
}

/// Exact owner-side observation of one authority generation.
///
/// The fields stay private deliberately: callers may carry this token across
/// queues/awaits, but only [`AuthorityCell`] can mint or validate it. Comparing
/// only `current` is insufficient because an off-state can travel through
/// `None -> delegated -> None` and look identical again. The captured
/// high-water mark makes every successful install/clear part of the identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct OwnerAuthorityToken {
    current: Option<AuthorityStamp>,
    installed_through_epoch: u64,
}

impl fmt::Debug for OwnerAuthorityToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OwnerAuthorityToken(..)")
    }
}

/// Synchronous, short-held authority cell shared by callback/task boundaries.
///
/// No guard is exposed, so callers cannot accidentally hold this mutex across
/// an `.await`. Poisoning is recovered deliberately: a panic in diagnostics or
/// teardown must not disable the stale-callback safety boundary.
pub struct AuthorityCell {
    next_epoch: AtomicU64,
    state: Mutex<AuthorityState>,
    owner_action_fences: AtomicU64,
    active_actions: AtomicU64,
    actions_drained: Notify,
    owner_actions_resumed: Notify,
}

impl Default for AuthorityCell {
    fn default() -> Self {
        Self {
            // Epoch zero is reserved as the initial high-water mark.
            next_epoch: AtomicU64::new(1),
            state: Mutex::new(AuthorityState::default()),
            owner_action_fences: AtomicU64::new(0),
            active_actions: AtomicU64::new(0),
            actions_drained: Notify::new(),
            owner_actions_resumed: Notify::new(),
        }
    }
}

/// RAII admission for one authority-sensitive action.
///
/// Handoffs first close admission and then wait for every live permit to drop
/// before capturing or restoring the queue. This turns the authority checks
/// from a racy check-before-await into a real transaction boundary.
pub struct AuthorityActionPermit {
    authority: Arc<AuthorityCell>,
}

impl fmt::Debug for AuthorityActionPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorityActionPermit(..)")
    }
}

impl Drop for AuthorityActionPermit {
    fn drop(&mut self) {
        let previous = self.authority.active_actions.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "authority action permit count underflow");
        if previous == 1 {
            self.authority.actions_drained.notify_waiters();
        }
    }
}

/// Atomic classification of one owner-side observation attempt.
///
/// `Delegated` is an actual guest authority snapshot. `Fenced` means a
/// lifecycle transition is in progress (or raced the observation) and callers
/// must retry without treating it as a guest handoff.
#[derive(Debug)]
pub enum OwnerAuthorityObservation {
    Owner {
        token: OwnerAuthorityToken,
        permit: AuthorityActionPermit,
    },
    Delegated,
    Fenced,
}

/// Exact re-admission result for work already stamped by an owner observation.
#[derive(Debug)]
pub enum ExactOwnerAuthorityObservation {
    Admitted(AuthorityActionPermit),
    Stale,
    Fenced,
}

impl fmt::Debug for AuthorityCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock_state();
        f.debug_struct("AuthorityCell")
            .field("current", &state.current)
            .field("installed_through_epoch", &state.installed_through_epoch)
            .finish_non_exhaustive()
    }
}

impl AuthorityCell {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve a unique epoch without making it active.
    pub fn reserve(&self, origin: AuthorityOrigin) -> AuthorityStamp {
        AuthorityStamp {
            epoch: self.reserve_epoch(),
            origin,
        }
    }

    /// Install `stamp` when it is newer than every prior install/clear fence.
    ///
    /// Returning `false` is the expected outcome for a late candidate whose
    /// reservation lost a latest-wins race. The current authority is untouched.
    pub fn install(&self, stamp: AuthorityStamp) -> bool {
        let mut state = self.lock_state();
        if stamp.epoch <= state.installed_through_epoch {
            return false;
        }
        state.installed_through_epoch = stamp.epoch;
        state.current = Some(stamp);
        true
    }

    /// Clear every current authority and fence out all stamps reserved before
    /// this call. Intended for global disable/logout/shutdown.
    pub fn clear(&self) -> Option<AuthorityStamp> {
        let fence = self.reserve_epoch();
        let mut state = self.lock_state();
        state.installed_through_epoch = state.installed_through_epoch.max(fence);
        state.current.take()
    }

    /// Clear only if `expected` is still current, while fencing candidates that
    /// were already reserved. A retired runtime therefore cannot clear its
    /// replacement during late teardown.
    pub fn clear_if_current(&self, expected: AuthorityStamp) -> bool {
        let fence = self.reserve_epoch();
        let mut state = self.lock_state();
        if state.current != Some(expected) {
            return false;
        }
        state.installed_through_epoch = state.installed_through_epoch.max(fence);
        state.current = None;
        true
    }

    pub fn current(&self) -> Option<AuthorityStamp> {
        self.lock_state().current
    }

    pub fn is_current(&self, stamp: AuthorityStamp) -> bool {
        self.current() == Some(stamp)
    }

    /// Owner-only daemon side effects remain enabled while QConnect is off or
    /// an owner runtime is installed, and are fenced for every guest epoch.
    pub fn owner_actions_allowed(&self) -> bool {
        self.owner_action_fences.load(Ordering::Acquire) == 0
            && !matches!(
                self.current().map(AuthorityStamp::origin),
                Some(AuthorityOrigin::Delegated { .. })
            )
    }

    /// Admit a daemon-local owner action and keep it visible to an authority
    /// handoff until the returned permit is dropped. Local actions are valid
    /// while QConnect is off or an owner runtime is current, never for a guest.
    pub fn try_owner_action_permit(self: &Arc<Self>) -> Option<AuthorityActionPermit> {
        self.try_owner_action_permit_observed()
            .map(|(_, permit)| permit)
    }

    /// Admit owner work and capture the exact authority generation under which
    /// its input will be read. The permit must remain live until that read and
    /// every directly governed mutation complete. The token may then stamp a
    /// queued continuation for exact re-admission at its eventual consumer.
    pub fn try_owner_action_permit_observed(
        self: &Arc<Self>,
    ) -> Option<(OwnerAuthorityToken, AuthorityActionPermit)> {
        match self.observe_owner_authority() {
            OwnerAuthorityObservation::Owner { token, permit } => Some((token, permit)),
            OwnerAuthorityObservation::Delegated | OwnerAuthorityObservation::Fenced => None,
        }
    }

    /// Atomically distinguish a real delegated observation from a transient
    /// lifecycle fence. Consumers that publish authority edges must skip a
    /// `Fenced` observation: treating every failed owner admission as delegated
    /// would fabricate a handoff while a guest candidate is merely activating
    /// and may still fail.
    pub fn observe_owner_authority(self: &Arc<Self>) -> OwnerAuthorityObservation {
        if self.owner_action_fences.load(Ordering::Acquire) != 0 {
            return OwnerAuthorityObservation::Fenced;
        }
        let observed_state = *self.lock_state();
        let Some(observed) = owner_token_for(observed_state) else {
            // This was a real delegated snapshot. A fence that starts after the
            // read cannot make the guest observation unsafe: callers will stamp
            // it delegated and never perform an owner-only effect.
            return OwnerAuthorityObservation::Delegated;
        };

        self.active_actions
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .expect("QConnect authority action permit count exhausted");

        // Close both races: fence acquisition and an authority install/clear
        // between the observation and the active-action increment.
        let fenced = self.owner_action_fences.load(Ordering::Acquire) != 0;
        let current_state = *self.lock_state();
        if !fenced && current_state == observed_state {
            OwnerAuthorityObservation::Owner {
                token: observed,
                permit: AuthorityActionPermit {
                    authority: Arc::clone(self),
                },
            }
        } else {
            self.release_rejected_action();
            if fenced {
                OwnerAuthorityObservation::Fenced
            } else if owner_token_for(current_state).is_none() {
                OwnerAuthorityObservation::Delegated
            } else {
                // An owner/clear generation changed without leaving a stable
                // delegated snapshot at this commit boundary. Retry instead of
                // publishing either authority from a torn observation.
                OwnerAuthorityObservation::Fenced
            }
        }
    }

    /// Re-admit a queued owner continuation only if the complete authority
    /// observation that produced it is still current. This rejects stale work
    /// even when the visible origin cycles back to owner or `None`.
    pub fn try_owner_action_permit_exact(
        self: &Arc<Self>,
        expected: OwnerAuthorityToken,
    ) -> Option<AuthorityActionPermit> {
        match self.observe_exact_owner_authority(expected) {
            ExactOwnerAuthorityObservation::Admitted(permit) => Some(permit),
            ExactOwnerAuthorityObservation::Stale | ExactOwnerAuthorityObservation::Fenced => None,
        }
    }

    /// Re-admit an exact owner token while preserving the distinction between
    /// a permanently stale generation and a transient lifecycle fence.
    pub fn observe_exact_owner_authority(
        self: &Arc<Self>,
        expected: OwnerAuthorityToken,
    ) -> ExactOwnerAuthorityObservation {
        if self.owner_action_fences.load(Ordering::Acquire) != 0 {
            return ExactOwnerAuthorityObservation::Fenced;
        }
        if owner_token_for(*self.lock_state()) != Some(expected) {
            return ExactOwnerAuthorityObservation::Stale;
        }

        self.active_actions
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .expect("QConnect authority action permit count exhausted");

        let fenced = self.owner_action_fences.load(Ordering::Acquire) != 0;
        let current_matches = owner_token_for(*self.lock_state()) == Some(expected);
        if !fenced && current_matches {
            ExactOwnerAuthorityObservation::Admitted(AuthorityActionPermit {
                authority: Arc::clone(self),
            })
        } else {
            self.release_rejected_action();
            if fenced {
                ExactOwnerAuthorityObservation::Fenced
            } else {
                ExactOwnerAuthorityObservation::Stale
            }
        }
    }

    /// Preserve one queued owner event across a transient lifecycle fence, then
    /// either re-admit it under the same exact token or reject it as stale once
    /// the transition outcome is known.
    pub async fn wait_for_exact_owner_action_permit(
        self: &Arc<Self>,
        expected: OwnerAuthorityToken,
    ) -> Option<AuthorityActionPermit> {
        loop {
            match self.observe_exact_owner_authority(expected) {
                ExactOwnerAuthorityObservation::Admitted(permit) => return Some(permit),
                ExactOwnerAuthorityObservation::Stale => return None,
                ExactOwnerAuthorityObservation::Fenced => {
                    self.wait_for_owner_actions_resumed().await;
                }
            }
        }
    }

    /// Admit an origin-agnostic transport action such as pause, seek or volume.
    ///
    /// Those controls are valid for both the signed-in owner and the currently
    /// delegated renderer, but they must still participate in the handoff
    /// fence. Otherwise a local seek admitted just before guest -> owner restore
    /// could land on the restored owner's stream after the authority swap.
    pub fn try_transport_action_permit(self: &Arc<Self>) -> Option<AuthorityActionPermit> {
        self.try_action_permit_matching_state(|_| true)
    }

    /// Admit work for one exact renderer runtime. A reserved, retired, or
    /// replaced stamp cannot obtain a permit, and an in-progress handoff closes
    /// admission before waiting for already admitted work to drain.
    pub fn try_runtime_action_permit(
        self: &Arc<Self>,
        expected: AuthorityStamp,
    ) -> Option<AuthorityActionPermit> {
        self.try_action_permit_matching_state(|state| state.current == Some(expected))
    }

    fn try_action_permit_matching_state<F>(
        self: &Arc<Self>,
        matches_authority: F,
    ) -> Option<AuthorityActionPermit>
    where
        F: Fn(&AuthorityState) -> bool,
    {
        if self.owner_action_fences.load(Ordering::Acquire) != 0 {
            return None;
        }
        if !matches_authority(&self.lock_state()) {
            return None;
        }

        self.active_actions
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .expect("QConnect authority action permit count exhausted");

        // Close the acquisition-vs-fence race. If a transition began after the
        // first check, this increment is either observed by its drain or rolled
        // back here before any caller-visible action can start.
        if self.owner_action_fences.load(Ordering::Acquire) == 0
            && matches_authority(&self.lock_state())
        {
            Some(AuthorityActionPermit {
                authority: Arc::clone(self),
            })
        } else {
            self.release_rejected_action();
            None
        }
    }

    fn release_rejected_action(&self) {
        let previous = self.active_actions.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "authority action permit count underflow");
        if previous == 1 {
            self.actions_drained.notify_waiters();
        }
    }

    /// Fence daemon owner-only background work across a guest -> owner restore
    /// or global teardown, while the queue snapshot is still being replaced.
    pub fn suspend_owner_actions(&self) {
        self.owner_action_fences
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .expect("QConnect owner-action fence count exhausted");
    }

    pub fn resume_owner_actions(&self) {
        match self
            .owner_action_fences
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            }) {
            Ok(1) => self.owner_actions_resumed.notify_waiters(),
            Ok(_) => {}
            Err(_) => log::error!("[QConnect] unbalanced owner-action fence release"),
        }
    }

    /// Wait until the outermost lifecycle fence has been released. The notify
    /// future is registered before the atomic check so a release cannot be
    /// missed between observation and suspension.
    pub async fn wait_for_owner_actions_resumed(&self) {
        loop {
            let notified = self.owner_actions_resumed.notified();
            if self.owner_action_fences.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Wait until every action admitted before the fence has completed.
    /// Callers must hold at least one owner-action fence for the whole wait.
    pub async fn wait_for_actions_drained(&self) {
        loop {
            let notified = self.actions_drained.notified();
            if self.active_actions.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    fn reserve_epoch(&self) -> u64 {
        self.next_epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                epoch.checked_add(1)
            })
            .expect("QConnect authority epoch space exhausted")
    }

    fn lock_state(&self) -> MutexGuard<'_, AuthorityState> {
        self.state.lock().unwrap_or_else(|poisoned| {
            log::error!("[QConnect] authority cell recovered from a poisoned lock");
            let guard = poisoned.into_inner();
            self.state.clear_poison();
            guard
        })
    }
}

fn owner_token_for(state: AuthorityState) -> Option<OwnerAuthorityToken> {
    if matches!(
        state.current.map(AuthorityStamp::origin),
        Some(AuthorityOrigin::Delegated { .. })
    ) {
        return None;
    }
    Some(OwnerAuthorityToken {
        current: state.current,
        installed_through_epoch: state.installed_through_epoch,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn disable_invalidates_inflight_enable_token() {
        let intent = QconnectEnableIntent::new(false);
        let first = intent.enable();
        assert_eq!(intent.enable(), first, "repeated enable must be idempotent");
        assert!(intent.is_current(first));

        intent.disable();
        assert!(!intent.is_current(first));
        assert!(intent
            .commit_if_current(first, || panic!("stale connect publication ran"))
            .is_none());

        let replacement = intent.enable();
        assert_ne!(replacement, first);
        assert!(intent.is_current(replacement));
        assert!(!intent.is_current(first));
    }

    #[tokio::test]
    async fn cancellation_observes_disable_that_precedes_the_wait() {
        let intent = QconnectEnableIntent::new(true);
        let token = intent.current_token().expect("initial enabled token");
        intent.disable();

        tokio::time::timeout(Duration::from_secs(1), intent.cancelled(token))
            .await
            .expect("pre-existing cancellation was missed");
    }

    #[tokio::test]
    async fn cancellation_wakes_a_registered_waiter() {
        let intent = Arc::new(QconnectEnableIntent::new(true));
        let token = intent.current_token().expect("initial enabled token");
        let waiter_intent = Arc::clone(&intent);
        let waiter = tokio::spawn(async move { waiter_intent.cancelled(token).await });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        intent.disable();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("registered cancellation waiter timed out")
            .expect("registered cancellation waiter panicked");
    }

    #[tokio::test]
    async fn exact_disable_wakes_its_enabled_waiter() {
        let intent = Arc::new(QconnectEnableIntent::new(true));
        let token = intent.current_token().expect("initial enabled token");
        let waiter_intent = Arc::clone(&intent);
        let waiter = tokio::spawn(async move { waiter_intent.cancelled(token).await });
        tokio::task::yield_now().await;

        let disabled = intent
            .disable_if_current(token)
            .expect("exact enabled intent should disable");
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("exact-disable cancellation waiter timed out")
            .expect("exact-disable cancellation waiter panicked");
        assert!(intent.is_disabled_current(disabled));
    }

    #[test]
    fn disabled_publication_is_rejected_after_a_new_enable() {
        let intent = QconnectEnableIntent::new(true);
        intent.disable();
        let replacement = intent.enable();

        assert!(intent
            .commit_if_disabled(|| panic!("stale disabled publication ran"))
            .is_none());
        assert!(intent.is_current(replacement));
    }

    #[test]
    fn later_manual_disable_invalidates_automatic_restore() {
        let intent = QconnectEnableIntent::new(true);
        let cast_suspend = intent.disable();
        let manual_disable = intent.disable();

        assert!(!intent.is_disabled_current(cast_suspend));
        assert!(intent.enable_if_disabled(cast_suspend).is_none());
        assert!(intent.is_disabled_current(manual_disable));
    }

    #[test]
    fn exact_disabled_intent_restores_once() {
        let intent = QconnectEnableIntent::new(true);
        let disabled = intent.disable();
        let restored = intent
            .enable_if_disabled(disabled)
            .expect("exact disabled intent should restore");

        assert!(intent.is_current(restored));
        assert!(intent.enable_if_disabled(disabled).is_none());
    }

    #[test]
    fn stale_enabled_intent_cannot_disable_its_replacement() {
        let intent = QconnectEnableIntent::new(true);
        let stale = intent.current_token().expect("initial enabled intent");
        let disabled = intent.disable();
        let replacement = intent
            .enable_if_disabled(disabled)
            .expect("replacement enabled intent");

        assert!(intent.disable_if_current(stale).is_none());
        assert!(intent.is_current(replacement));
    }

    #[test]
    fn manual_enable_supersedes_cast_disable_of_consumed_restore() {
        let intent = QconnectEnableIntent::new(true);
        let suspended = intent.disable();
        let automatic_restore = intent
            .enable_if_disabled(suspended)
            .expect("automatic restore should consume its exact token");
        let manual_connect = intent.enable_new_intent();

        assert_ne!(manual_connect, automatic_restore);
        assert!(intent.disable_if_current(automatic_restore).is_none());
        assert!(intent.is_current(manual_connect));
    }

    #[tokio::test]
    async fn new_enabled_intent_wakes_the_superseded_waiter() {
        let intent = Arc::new(QconnectEnableIntent::new(true));
        let stale = intent.current_token().expect("initial enabled intent");
        let waiter_intent = Arc::clone(&intent);
        let waiter = tokio::spawn(async move { waiter_intent.cancelled(stale).await });
        tokio::task::yield_now().await;

        let replacement = intent.enable_new_intent();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("superseded enabled waiter timed out")
            .expect("superseded enabled waiter panicked");
        assert!(intent.is_current(replacement));
    }

    #[test]
    fn consumed_restore_can_be_replaced_by_exact_cast_disable() {
        let intent = QconnectEnableIntent::new(true);
        let first_disabled = intent.disable();
        let restored = intent
            .enable_if_disabled(first_disabled)
            .expect("exact restore should enable");
        let replacement_disabled = intent
            .disable_if_current(restored)
            .expect("exact restored intent should disable");

        assert_ne!(replacement_disabled, first_disabled);
        assert!(intent.is_disabled_current(replacement_disabled));
        assert!(intent.enable_if_disabled(first_disabled).is_none());
        assert!(intent.enable_if_disabled(replacement_disabled).is_some());
    }

    #[test]
    fn manual_disable_after_cast_replacement_invalidates_restore() {
        let intent = QconnectEnableIntent::new(true);
        let first_disabled = intent.disable();
        let restored = intent
            .enable_if_disabled(first_disabled)
            .expect("exact restore should enable");
        let cast_disabled = intent
            .disable_if_current(restored)
            .expect("Cast should replace the consumed restore token");
        let manual_disabled = intent.disable();

        assert!(!intent.is_disabled_current(cast_disabled));
        assert!(intent.enable_if_disabled(cast_disabled).is_none());
        assert!(intent.is_disabled_current(manual_disabled));
    }

    #[test]
    fn disable_serializes_after_an_already_committing_publication() {
        let intent = Arc::new(QconnectEnableIntent::new(true));
        let token = intent.current_token().expect("initial enabled token");
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let published = Arc::new(AtomicBool::new(false));

        let worker_intent = Arc::clone(&intent);
        let worker_entered = Arc::clone(&entered);
        let worker_release = Arc::clone(&release);
        let worker_published = Arc::clone(&published);
        let worker = std::thread::spawn(move || {
            worker_intent
                .commit_if_current(token, || {
                    worker_entered.wait();
                    worker_release.wait();
                    worker_published.store(true, AtomicOrdering::Release);
                })
                .expect("current publication was rejected");
        });

        entered.wait();
        let disable_intent = Arc::clone(&intent);
        let disable = std::thread::spawn(move || disable_intent.disable());
        assert!(
            !disable.is_finished(),
            "disable bypassed the commit boundary"
        );
        release.wait();
        worker.join().expect("publication thread panicked");
        disable.join().expect("disable thread panicked");

        assert!(published.load(AtomicOrdering::Acquire));
        assert!(intent.current_token().is_none());
        assert!(!intent.is_current(token));
    }

    #[test]
    fn reserve_does_not_activate_a_runtime() {
        let cell = AuthorityCell::new();
        let stamp = cell.reserve(AuthorityOrigin::Owner);

        assert_eq!(stamp.epoch(), 1);
        assert_eq!(stamp.origin(), AuthorityOrigin::Owner);
        assert_eq!(cell.current(), None);
        assert!(!cell.is_current(stamp));
    }

    #[test]
    fn install_is_exact_and_monotonic() {
        let cell = AuthorityCell::new();
        let old = cell.reserve(AuthorityOrigin::Owner);
        let guest = cell.reserve(AuthorityOrigin::Delegated { generation: 7 });

        assert!(cell.install(guest));
        assert!(cell.is_current(guest));
        assert_eq!(guest.origin().delegated_generation(), Some(7));
        assert!(!cell.install(old));
        assert!(cell.is_current(guest));
    }

    #[test]
    fn clear_fences_previously_reserved_candidates() {
        let cell = AuthorityCell::new();
        let owner = cell.reserve(AuthorityOrigin::Owner);
        let pending = cell.reserve(AuthorityOrigin::Delegated { generation: 1 });
        assert!(cell.install(owner));

        assert_eq!(cell.clear(), Some(owner));
        assert!(!cell.install(pending));
        assert_eq!(cell.current(), None);

        let replacement = cell.reserve(AuthorityOrigin::Owner);
        assert!(cell.install(replacement));
        assert!(cell.is_current(replacement));
    }

    #[test]
    fn retired_runtime_cannot_clear_its_replacement() {
        let cell = AuthorityCell::new();
        let old = cell.reserve(AuthorityOrigin::Owner);
        let replacement = cell.reserve(AuthorityOrigin::Owner);
        assert!(cell.install(old));
        assert!(cell.install(replacement));

        assert!(!cell.clear_if_current(old));
        assert!(cell.is_current(replacement));
    }

    #[test]
    fn owner_actions_are_fenced_only_for_delegated_authority() {
        let cell = AuthorityCell::new();
        assert!(cell.owner_actions_allowed());
        let owner = cell.reserve(AuthorityOrigin::Owner);
        assert!(cell.install(owner));
        assert!(cell.owner_actions_allowed());
        let guest = cell.reserve(AuthorityOrigin::Delegated { generation: 3 });
        assert!(cell.install(guest));
        assert!(!cell.owner_actions_allowed());
        cell.suspend_owner_actions();
        assert!(!cell.owner_actions_allowed());
        cell.clear();
        assert!(!cell.owner_actions_allowed());
        cell.suspend_owner_actions();
        cell.resume_owner_actions();
        assert!(!cell.owner_actions_allowed());
        cell.resume_owner_actions();
        assert!(cell.owner_actions_allowed());
    }

    #[test]
    fn permits_are_exact_to_the_installed_authority() {
        let cell = Arc::new(AuthorityCell::new());
        assert!(cell.try_owner_action_permit().is_some());

        let owner = cell.reserve(AuthorityOrigin::Owner);
        assert!(cell.install(owner));
        assert!(cell.try_owner_action_permit().is_some());
        assert!(cell.try_runtime_action_permit(owner).is_some());

        let guest = cell.reserve(AuthorityOrigin::Delegated { generation: 9 });
        assert!(cell.install(guest));
        assert!(cell.try_owner_action_permit().is_none());
        assert!(cell.try_transport_action_permit().is_some());
        assert!(cell.try_runtime_action_permit(owner).is_none());
        assert!(cell.try_runtime_action_permit(guest).is_some());
    }

    #[test]
    fn owner_observation_distinguishes_transition_fence_from_delegated() {
        let cell = Arc::new(AuthorityCell::new());
        match cell.observe_owner_authority() {
            OwnerAuthorityObservation::Owner { permit, .. } => drop(permit),
            other => panic!("initial owner observation was {other:?}"),
        }

        cell.suspend_owner_actions();
        assert!(matches!(
            cell.observe_owner_authority(),
            OwnerAuthorityObservation::Fenced
        ));
        cell.resume_owner_actions();

        let guest = cell.reserve(AuthorityOrigin::Delegated { generation: 12 });
        assert!(cell.install(guest));
        assert!(matches!(
            cell.observe_owner_authority(),
            OwnerAuthorityObservation::Delegated
        ));
    }

    #[test]
    fn exact_observation_distinguishes_transition_fence_from_stale_token() {
        let cell = Arc::new(AuthorityCell::new());
        let (token, permit) = cell
            .try_owner_action_permit_observed()
            .expect("initial owner observation");
        drop(permit);

        cell.suspend_owner_actions();
        assert!(matches!(
            cell.observe_exact_owner_authority(token),
            ExactOwnerAuthorityObservation::Fenced
        ));
        cell.resume_owner_actions();
        match cell.observe_exact_owner_authority(token) {
            ExactOwnerAuthorityObservation::Admitted(permit) => drop(permit),
            other => panic!("same owner token was not re-admitted: {other:?}"),
        }

        let guest = cell.reserve(AuthorityOrigin::Delegated { generation: 13 });
        assert!(cell.install(guest));
        assert!(matches!(
            cell.observe_exact_owner_authority(token),
            ExactOwnerAuthorityObservation::Stale
        ));
    }

    #[tokio::test]
    async fn queued_exact_admission_waits_for_fence_outcome() {
        let cell = Arc::new(AuthorityCell::new());
        let (token, permit) = cell
            .try_owner_action_permit_observed()
            .expect("initial owner observation");
        drop(permit);
        cell.suspend_owner_actions();
        let waiter_cell = Arc::clone(&cell);
        let waiter =
            tokio::spawn(
                async move { waiter_cell.wait_for_exact_owner_action_permit(token).await },
            );
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        cell.resume_owner_actions();
        let permit = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("exact admission waiter timed out")
            .expect("exact admission waiter panicked")
            .expect("same token became stale across a fence-only transition");
        drop(permit);
    }

    #[test]
    fn exact_owner_token_rejects_none_guest_none_cycle() {
        let cell = Arc::new(AuthorityCell::new());
        let (initial, permit) = cell
            .try_owner_action_permit_observed()
            .expect("initial off-state owner admission");
        drop(permit);

        let guest = cell.reserve(AuthorityOrigin::Delegated { generation: 17 });
        assert!(cell.install(guest));
        assert_eq!(cell.clear(), Some(guest));
        assert_eq!(cell.current(), None);

        assert!(cell.try_owner_action_permit_exact(initial).is_none());
        let (replacement, _) = cell
            .try_owner_action_permit_observed()
            .expect("fresh off-state owner admission");
        assert_ne!(replacement, initial);
        assert!(cell.try_owner_action_permit_exact(replacement).is_some());
    }

    #[test]
    fn exact_owner_token_rejects_clear_while_current_stays_none() {
        let cell = Arc::new(AuthorityCell::new());
        let (before_clear, permit) = cell
            .try_owner_action_permit_observed()
            .expect("initial off-state owner admission");
        drop(permit);

        assert_eq!(cell.clear(), None);
        assert_eq!(cell.current(), None);
        assert!(cell.try_owner_action_permit_exact(before_clear).is_none());

        let (after_clear, _) = cell
            .try_owner_action_permit_observed()
            .expect("post-clear off-state owner admission");
        assert_ne!(after_clear, before_clear);
    }

    #[test]
    fn exact_owner_token_rejects_owner_guest_owner_cycle() {
        let cell = Arc::new(AuthorityCell::new());
        let owner = cell.reserve(AuthorityOrigin::Owner);
        assert!(cell.install(owner));
        let (old_owner, permit) = cell
            .try_owner_action_permit_observed()
            .expect("installed owner admission");
        drop(permit);

        let guest = cell.reserve(AuthorityOrigin::Delegated { generation: 18 });
        assert!(cell.install(guest));
        let restored = cell.reserve(AuthorityOrigin::Owner);
        assert!(cell.install(restored));

        assert!(cell.try_owner_action_permit_exact(old_owner).is_none());
        let (new_owner, _) = cell
            .try_owner_action_permit_observed()
            .expect("restored owner admission");
        assert_ne!(new_owner, old_owner);
        assert!(cell.try_owner_action_permit_exact(new_owner).is_some());
    }

    #[test]
    fn transport_actions_allow_guest_but_not_a_transition_fence() {
        let cell = Arc::new(AuthorityCell::new());
        let guest = cell.reserve(AuthorityOrigin::Delegated { generation: 2 });
        assert!(cell.install(guest));
        assert!(cell.try_transport_action_permit().is_some());

        cell.suspend_owner_actions();
        assert!(cell.try_transport_action_permit().is_none());
        cell.resume_owner_actions();
        assert!(cell.try_transport_action_permit().is_some());
    }

    #[tokio::test]
    async fn fence_closes_admission_and_waits_for_live_permits() {
        let cell = Arc::new(AuthorityCell::new());
        let owner = cell.reserve(AuthorityOrigin::Owner);
        assert!(cell.install(owner));
        let permit = cell
            .try_owner_action_permit()
            .expect("owner action admitted before fence");

        cell.suspend_owner_actions();
        assert!(cell.try_owner_action_permit().is_none());
        assert!(cell.try_runtime_action_permit(owner).is_none());

        let waiter_cell = Arc::clone(&cell);
        let waiter = tokio::spawn(async move {
            waiter_cell.wait_for_actions_drained().await;
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(permit);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("permit drain timed out")
            .expect("permit drain task panicked");
        cell.resume_owner_actions();
        assert!(cell.try_owner_action_permit().is_some());
    }

    #[tokio::test]
    async fn fence_drains_active_delegated_runtime_permits() {
        let cell = Arc::new(AuthorityCell::new());
        let guest = cell.reserve(AuthorityOrigin::Delegated { generation: 4 });
        assert!(cell.install(guest));
        let permit = cell
            .try_runtime_action_permit(guest)
            .expect("delegated runtime action admitted before fence");

        cell.suspend_owner_actions();
        assert!(cell.try_runtime_action_permit(guest).is_none());

        let waiter_cell = Arc::clone(&cell);
        let waiter = tokio::spawn(async move {
            waiter_cell.wait_for_actions_drained().await;
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(permit);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("delegated permit drain timed out")
            .expect("delegated permit drain task panicked");
        cell.resume_owner_actions();
        assert!(cell.try_runtime_action_permit(guest).is_some());
    }

    #[test]
    fn poison_is_recovered() {
        let cell = Arc::new(AuthorityCell::new());
        let stamp = cell.reserve(AuthorityOrigin::Owner);
        let panic_cell = Arc::clone(&cell);
        let _ = std::thread::spawn(move || {
            let _guard = panic_cell.state.lock().expect("initial lock");
            panic!("poison authority cell for recovery test");
        })
        .join();

        assert!(cell.install(stamp));
        assert!(!cell.state.is_poisoned());
        assert!(cell.is_current(stamp));
    }
}
