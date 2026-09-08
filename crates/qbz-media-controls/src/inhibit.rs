//! Linux sleep/idle inhibitor via `org.freedesktop.login1` (#522).
//!
//! While playback is Playing, QBZ holds a logind inhibitor lock
//! (`Inhibit("sleep:idle", ..., "block")`) so the system does not suspend or
//! go idle mid-track. The lock is fd-based: logind releases it the moment the
//! returned file descriptor is closed, so pause/stop (or process exit, even a
//! crash) releases it automatically — no explicit "uninhibit" call exists or
//! is needed.
//!
//! Uses the zbus re-exported by `mpris-server` (same graph-wide zbus the rest
//! of this crate rides) with a raw `call_method` — no proxy macro, matching
//! the crate's dependency-light style. login1 lives on the **system** bus,
//! unlike MPRIS (session bus), so this keeps its own lazy connection.

use mpris_server::zbus::{self, zvariant::OwnedFd};
use std::time::{Duration, Instant};

const LOGIN1_DEST: &str = "org.freedesktop.login1";
const LOGIN1_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_IFACE: &str = "org.freedesktop.login1.Manager";

/// After a transient `Inhibit` failure, wait this long before the next try.
/// `set_playing(true)` arrives with every playback-state update (~2 s), so
/// without a backoff a headless box logged the same warning every 2 s (369
/// lines in 11 min on a Pi daemon, 2026-08-29).
const RETRY_BACKOFF: Duration = Duration::from_secs(300);

/// Why the last `Inhibit` attempt did not produce a lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Denial {
    /// A permanent policy answer for this process (polkit wants an
    /// interactive auth we can never give — a daemon with no logind
    /// session). Never retried.
    Policy,
    /// Anything else (bus hiccup, logind restarting). Retried after
    /// `RETRY_BACKOFF`.
    Transient,
}

/// Pure retry decision, so the backoff can be unit-tested without a bus.
fn should_attempt(last: Option<(Denial, Instant)>, now: Instant) -> bool {
    match last {
        None => true,
        Some((Denial::Policy, _)) => false,
        Some((Denial::Transient, at)) => now.duration_since(at) >= RETRY_BACKOFF,
    }
}

fn classify(error: &zbus::Error) -> Denial {
    let text = error.to_string();
    if text.contains("InteractiveAuthorizationRequired") || text.contains("AccessDenied") {
        Denial::Policy
    } else {
        Denial::Transient
    }
}

/// Holds the inhibitor fd while playing. Owned by the MPRIS update loop
/// (single-threaded); acquire/release are driven by playback-state updates.
pub struct SleepInhibitor {
    /// Lazy system-bus connection; `None` until first acquire (or if the
    /// system bus is unreachable — then we retry on the next transition).
    conn: Option<zbus::Connection>,
    /// The inhibitor lock. `Some` while Playing; dropping it closes the fd,
    /// which is how logind releases the lock.
    fd: Option<OwnedFd>,
    /// Last failed attempt and why — gates the next attempt (see
    /// `should_attempt`).
    last_failure: Option<(Denial, Instant)>,
}

impl SleepInhibitor {
    pub fn new() -> Self {
        Self {
            conn: None,
            fd: None,
            last_failure: None,
        }
    }

    /// Drive the inhibitor from a playback transition: acquire on Playing,
    /// release on Paused/Stopped. Idempotent — repeated Playing updates keep
    /// the existing lock. Failures are logged and non-fatal (playback must
    /// never depend on logind being present, e.g. non-systemd distros).
    pub async fn set_playing(&mut self, playing: bool) {
        if playing {
            self.acquire().await;
        } else {
            self.release();
        }
    }

    async fn acquire(&mut self) {
        if self.fd.is_some() {
            return;
        }
        if !should_attempt(self.last_failure, Instant::now()) {
            return;
        }
        let conn = match &self.conn {
            Some(c) => c.clone(),
            None => match zbus::Connection::system().await {
                Ok(c) => {
                    self.conn = Some(c.clone());
                    c
                }
                Err(e) => {
                    log::warn!(
                        "[inhibit] system bus unavailable, cannot inhibit sleep (retry in {}s): {e}",
                        RETRY_BACKOFF.as_secs()
                    );
                    self.last_failure = Some((Denial::Transient, Instant::now()));
                    return;
                }
            },
        };
        let reply = conn
            .call_method(
                Some(LOGIN1_DEST),
                LOGIN1_PATH,
                Some(LOGIN1_IFACE),
                "Inhibit",
                &("sleep:idle", "QBZ", "Music playback in progress", "block"),
            )
            .await;
        match reply {
            Ok(msg) => match msg.body().deserialize::<OwnedFd>() {
                Ok(fd) => {
                    log::info!("[inhibit] acquired login1 sleep:idle inhibitor");
                    self.fd = Some(fd);
                    self.last_failure = None;
                }
                Err(e) => {
                    log::warn!("[inhibit] bad Inhibit reply (expected fd): {e}");
                    self.last_failure = Some((Denial::Transient, Instant::now()));
                }
            },
            Err(e) => {
                let denial = classify(&e);
                match denial {
                    Denial::Policy => log::info!(
                        "[inhibit] login1 refuses a sleep:idle inhibitor for this process ({e}); \
                         giving up — playback is unaffected"
                    ),
                    Denial::Transient => log::warn!(
                        "[inhibit] login1 Inhibit call failed (retry in {}s): {e}",
                        RETRY_BACKOFF.as_secs()
                    ),
                }
                self.last_failure = Some((denial, Instant::now()));
            }
        }
    }

    fn release(&mut self) {
        if self.fd.take().is_some() {
            // Dropping the OwnedFd closes it; logind releases the lock.
            log::info!("[inhibit] released login1 sleep:idle inhibitor");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_attempt_is_always_made() {
        assert!(should_attempt(None, Instant::now()));
    }

    #[test]
    fn policy_denial_is_never_retried() {
        let at = Instant::now();
        assert!(!should_attempt(Some((Denial::Policy, at)), at));
        assert!(!should_attempt(Some((Denial::Policy, at)), at + RETRY_BACKOFF * 10));
    }

    #[test]
    fn transient_failure_waits_out_the_backoff() {
        let at = Instant::now();
        assert!(!should_attempt(Some((Denial::Transient, at)), at));
        assert!(!should_attempt(Some((Denial::Transient, at)), at + RETRY_BACKOFF / 2));
        assert!(should_attempt(Some((Denial::Transient, at)), at + RETRY_BACKOFF));
    }
}
