//! WHICH disc the open ephemeral session came from.
//!
//! The session itself deliberately knows nothing about discs — `adopt_tracks`
//! takes a label and a track list, and that is the whole point: a disc is not
//! a second kind of session. But two features need to name the medium rather
//! than the session:
//!
//!  * the metadata button, which has to write its correction under a key that
//!    survives the eject (`qbz_disc::store`);
//!  * the rip wizard, which reads that same key for its defaults.
//!
//! So the identity lives HERE, beside the session rather than inside it: one
//! process-global slot, set when a disc opens and cleared when the session
//! closes. Nothing persists — the STORE is what persists; this only remembers
//! which row is in the drive right now.

use std::sync::Mutex;

/// What kind of medium is open. The two are not interchangeable: a CD has a
/// MusicBrainz DiscID and can be ripped, a SACD image has neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscKind {
    Cd,
    Sacd,
}

impl DiscKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DiscKind::Cd => "cd",
            DiscKind::Sacd => "sacd",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscIdentity {
    /// TOC geometry hash — the key into `qbz_disc::store`. Available before
    /// anything has been looked up, and identical for two pressings that share
    /// a table of contents (which is exactly why remembering the user's choice
    /// under it is worth doing).
    pub fingerprint: String,
    /// MusicBrainz DiscID. `None` for a SACD image, which names itself.
    pub disc_id: Option<String>,
    pub kind: DiscKind,
}

static CURRENT: Mutex<Option<DiscIdentity>> = Mutex::new(None);

pub fn set(identity: DiscIdentity) {
    if let Ok(mut slot) = CURRENT.lock() {
        *slot = Some(identity);
    }
    publish(true);
}

pub fn current() -> Option<DiscIdentity> {
    CURRENT.lock().ok()?.clone()
}

/// The session closed, or a folder replaced the disc. Called from the SAME
/// place that tears the session down, so the two cannot disagree about whether
/// a disc is open.
pub fn clear() {
    if let Ok(mut slot) = CURRENT.lock() {
        *slot = None;
    }
    publish(false);
}

/// Tell the UI whether what is open is a DISC.
///
/// The pane needs that question answered for BOTH media, and neither existing
/// flag does it: `local_ephemeral_is_cd` is narrower on purpose (only a
/// physical CD can be ripped) and there is nothing at all for a SACD image.
/// Without it the "correct the details" button appeared on an opened FOLDER,
/// where there is no disc to correct and nothing to write a correction under.
fn publish(is_disc: bool) {
    crate::local_bridge::ui(move |mut b| b.as_mut().set_local_session_is_disc(is_disc));
}
