use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::HandoffCandidate;

#[derive(Default)]
struct AdmissionSlot {
    candidate: Option<HandoffCandidate>,
    closed: bool,
}

struct AdmissionShared {
    slot: Mutex<AdmissionSlot>,
    changed: Condvar,
}

#[derive(Clone)]
pub struct AdmissionSender {
    shared: Arc<AdmissionShared>,
}

pub struct AdmissionInbox {
    shared: Arc<AdmissionShared>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    Closed,
}

/// One pending candidate, latest-wins. Replacement drops and zeroizes the old
/// candidate before returning; memory use never grows with POST bursts.
pub fn admission_channel() -> (AdmissionSender, AdmissionInbox) {
    let shared = Arc::new(AdmissionShared {
        slot: Mutex::new(AdmissionSlot::default()),
        changed: Condvar::new(),
    });
    (
        AdmissionSender {
            shared: Arc::clone(&shared),
        },
        AdmissionInbox { shared },
    )
}

impl AdmissionSender {
    pub fn submit(&self, candidate: HandoffCandidate) -> Result<(), SubmitError> {
        let mut slot = self
            .shared
            .slot
            .lock()
            .expect("LAN admission lock poisoned");
        if slot.closed {
            return Err(SubmitError::Closed);
        }
        slot.candidate = Some(candidate);
        self.shared.changed.notify_one();
        Ok(())
    }

    pub fn close(&self) {
        let mut slot = self
            .shared
            .slot
            .lock()
            .expect("LAN admission lock poisoned");
        slot.closed = true;
        slot.candidate = None;
        self.shared.changed.notify_all();
    }
}

impl AdmissionInbox {
    pub fn try_take(&self) -> Option<HandoffCandidate> {
        self.shared
            .slot
            .lock()
            .expect("LAN admission lock poisoned")
            .candidate
            .take()
    }

    pub fn take_timeout(&self, timeout: Duration) -> Option<HandoffCandidate> {
        let slot = self
            .shared
            .slot
            .lock()
            .expect("LAN admission lock poisoned");
        let mut slot = self
            .shared
            .changed
            .wait_timeout_while(slot, timeout, |slot| {
                slot.candidate.is_none() && !slot.closed
            })
            .expect("LAN admission lock poisoned")
            .0;
        slot.candidate.take()
    }

    pub fn is_closed(&self) -> bool {
        self.shared
            .slot
            .lock()
            .expect("LAN admission lock poisoned")
            .closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LanJwtToken;

    fn candidate(session: &str) -> HandoffCandidate {
        HandoffCandidate::new(
            session.to_string(),
            LanJwtToken::new("https://api.example".into(), 999, "api".into()),
            LanJwtToken::new("wss://qws.example".into(), 999, "qws".into()),
            true,
        )
    }

    #[test]
    fn pending_slot_is_latest_wins_and_bounded() {
        let (sender, inbox) = admission_channel();
        sender.submit(candidate("first")).unwrap();
        sender.submit(candidate("second")).unwrap();

        assert_eq!(inbox.try_take().unwrap().session_id(), "second");
        assert!(inbox.try_take().is_none());
    }

    #[test]
    fn close_drops_pending_and_rejects_late_submit() {
        let (sender, inbox) = admission_channel();
        sender.submit(candidate("pending")).unwrap();
        sender.close();

        assert!(inbox.is_closed());
        assert!(inbox.try_take().is_none());
        assert_eq!(sender.submit(candidate("late")), Err(SubmitError::Closed));
    }
}
