//! Process-wide offline gate — the single Qobuz choke point.
//!
//! Offline MODE (induced from Settings or detected loss of connectivity)
//! means ZERO Qobuz services: while the gate is closed, every HTTP request
//! the client would issue fails fast with [`crate::error::ApiError::OfflineMode`]
//! instead of timing out against the network. The owner of the flag is the
//! shared offline-mode engine in `qbz-app`; frontends never set it directly.
//!
//! The flag is process-global on purpose: there is exactly one Qobuz client
//! per process, the gate must hold across re-logins, and a per-instance flag
//! would silently reset when the client is rebuilt.

use std::sync::atomic::{AtomicBool, Ordering};

static OFFLINE: AtomicBool = AtomicBool::new(false);

/// Close (true) or open (false) the gate. Called by the offline-mode engine
/// on every mode transition.
pub fn set_offline(offline: bool) {
    let was = OFFLINE.swap(offline, Ordering::Relaxed);
    if was != offline {
        log::info!(
            "[OfflineGate] Qobuz API gate {}",
            if offline { "CLOSED (offline mode)" } else { "OPEN" }
        );
    }
}

/// Whether the gate is currently closed (offline mode active).
pub fn is_offline() -> bool {
    OFFLINE.load(Ordering::Relaxed)
}

/// Serializes tests that flip the process-global gate (tests run in
/// parallel). Shared crate-wide so client.rs tests and the tests below
/// take the same lock.
#[cfg(test)]
pub(crate) fn test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

/// Drop guard that reopens the gate, so a panicking test cannot leak a
/// closed gate into other tests in this process.
#[cfg(test)]
pub(crate) struct TestGateReset;

#[cfg(test)]
impl Drop for TestGateReset {
    fn drop(&mut self) {
        set_offline(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_toggles() {
        let _lock = test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _reset = TestGateReset;
        set_offline(false);
        assert!(!is_offline());
        set_offline(true);
        assert!(is_offline());
        set_offline(false);
        assert!(!is_offline());
    }
}
