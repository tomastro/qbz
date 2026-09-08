//! Forbidden (HTTP 403) circuit breaker.
//!
//! Issue #637: after a Qobuz outage + forced re-login, a user's account can be
//! transiently rejected (entitlement not yet restored, a wedged session, etc.).
//! Our prefetch scheduler re-drives `get_stream_url` / CMAF `session/start`
//! with no backoff, so a handful of legitimate 403s escalated into a sustained
//! ~2-3 req/s storm — which trips Qobuz's edge/WAF and turns the transient 403
//! into a persistent per-IP block (the "error decoding response body" lines in
//! the report are that HTML/empty WAF body hitting a `.json()` call).
//!
//! This breaker converts "hammer until IP-banned" into "back off and recover":
//! once a small number of 403s land in quick succession it opens for an
//! exponential cooldown, during which the hot streaming/favorites paths
//! short-circuit WITHOUT touching the network. After the cooldown a single
//! probe is allowed through; a success closes the breaker, another 403 re-opens
//! it immediately with a longer cooldown.
//!
//! Only genuine 403s feed it — 5xx/429/404/transport errors have their own
//! handling and must not open it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Consecutive 403s (across the streaming/favorites paths) that open the breaker.
const OPEN_THRESHOLD: u32 = 3;
/// First cooldown once the breaker opens.
const BASE_COOLDOWN: Duration = Duration::from_secs(30);
/// Cooldown ceiling — doubling stops here.
const MAX_COOLDOWN: Duration = Duration::from_secs(120);

struct Inner {
    /// Consecutive 403s not yet cleared by a success. Not reset when the breaker
    /// opens, so the post-cooldown probe re-opens on a single further 403.
    consecutive: u32,
    /// When set and still in the future, the breaker is open until this instant.
    open_until: Option<Instant>,
    /// Cooldown applied on the next open (grows exponentially, capped).
    next_cooldown: Duration,
}

/// A shared, cheaply-clonable 403 circuit breaker. Uses a plain `Mutex` (no
/// `.await` held) so it is trivially callable from any async path.
pub struct ForbiddenBreaker {
    inner: Mutex<Inner>,
}

impl Default for ForbiddenBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl ForbiddenBreaker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                consecutive: 0,
                open_until: None,
                next_cooldown: BASE_COOLDOWN,
            }),
        }
    }

    /// If the breaker is open, returns the remaining cooldown so the caller can
    /// short-circuit (and log how long it is backing off). Returns `None` when
    /// closed OR when the cooldown has just elapsed — in the latter case the
    /// open state is cleared so exactly the next call is a half-open probe.
    pub fn blocked_for(&self) -> Option<Duration> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(until) = g.open_until {
            let now = Instant::now();
            if now < until {
                return Some(until - now);
            }
            // Cooldown elapsed: half-open. Let the next call probe the network.
            g.open_until = None;
        }
        None
    }

    /// Record a successful authenticated response: fully resets the breaker.
    pub fn record_success(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.consecutive = 0;
        g.open_until = None;
        g.next_cooldown = BASE_COOLDOWN;
    }

    /// Record a 403. Opens (or re-opens) the breaker once the threshold is hit.
    /// Returns the cooldown it opened with, if it opened on this call — for
    /// logging.
    pub fn record_forbidden(&self) -> Option<Duration> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.consecutive = g.consecutive.saturating_add(1);
        if g.consecutive >= OPEN_THRESHOLD {
            let cooldown = g.next_cooldown;
            g.open_until = Some(Instant::now() + cooldown);
            g.next_cooldown = (g.next_cooldown * 2).min(MAX_COOLDOWN);
            Some(cooldown)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_closed_below_threshold() {
        let b = ForbiddenBreaker::new();
        assert!(b.record_forbidden().is_none());
        assert!(b.record_forbidden().is_none());
        assert!(b.blocked_for().is_none(), "2 < threshold, still closed");
    }

    #[test]
    fn opens_at_threshold_and_blocks() {
        let b = ForbiddenBreaker::new();
        b.record_forbidden();
        b.record_forbidden();
        let opened = b.record_forbidden();
        assert_eq!(opened, Some(BASE_COOLDOWN), "3rd 403 opens with base cooldown");
        let remaining = b.blocked_for().expect("breaker is open");
        assert!(remaining <= BASE_COOLDOWN && remaining > Duration::ZERO);
    }

    #[test]
    fn success_resets_everything() {
        let b = ForbiddenBreaker::new();
        b.record_forbidden();
        b.record_forbidden();
        b.record_success();
        // Counter cleared: it takes another full threshold to open again.
        assert!(b.record_forbidden().is_none());
        assert!(b.record_forbidden().is_none());
        assert!(b.blocked_for().is_none());
    }

    #[test]
    fn cooldown_grows_exponentially_and_caps() {
        let b = ForbiddenBreaker::new();
        // First open: BASE.
        b.record_forbidden();
        b.record_forbidden();
        assert_eq!(b.record_forbidden(), Some(BASE_COOLDOWN));
        // Consecutive is not reset on open, so a single further 403 re-opens
        // with the doubled cooldown.
        assert_eq!(b.record_forbidden(), Some(BASE_COOLDOWN * 2));
        assert_eq!(b.record_forbidden(), Some((BASE_COOLDOWN * 4).min(MAX_COOLDOWN)));
        // Keep re-opening; cooldown must never exceed the cap.
        for _ in 0..8 {
            let c = b.record_forbidden().expect("re-opens each time past threshold");
            assert!(c <= MAX_COOLDOWN);
        }
        assert_eq!(b.record_forbidden(), Some(MAX_COOLDOWN), "cooldown pinned at cap");
    }
}
