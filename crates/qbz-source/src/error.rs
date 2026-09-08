//! The one error vocabulary for the seam.
//!
//! Note the field is `by`, never `source`: `thiserror` (workspace pins `= "2"`,
//! `crates/Cargo.toml:75`) treats a variant field NAMED `source` as the
//! `std::error::Error::source()` chain and requires it to implement `Error`.
//! [`crate::SourceId`] is a `&'static str` newtype and does not, so a field
//! called `source` here would not compile.

use crate::id::{ItemKind, RawRef, SourceId};

/// Everything that can go wrong between "a caller has a string" and "the
/// frontend has tracks, metadata, artwork or a playback ticket".
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// No source recognised the reference at all.
    #[error("no source claims {0:?}")]
    Unclaimed(RawRef),

    /// A bare id that MORE THAN ONE source could legitimately own, with no
    /// source word to break the tie (survey §6.3: a numeric id can be a Qobuz
    /// track id, `local_tracks.id = 2954`, OR a Plex rating key `44440`).
    ///
    /// The registry REFUSES to guess. This is deliberately loud: it turns the
    /// producer-side gaps (`HomeCard` has no `source` field — survey IC-7; the
    /// eight QML sites that hardcode `"source": "qobuz"` — survey §2.6) into
    /// visible failures instead of silently mis-rendered rows.
    #[error("ambiguous id {id:?} ({kind:?}): candidates {candidates:?}; caller must supply a source")]
    Ambiguous {
        id: String,
        kind: Option<ItemKind>,
        candidates: Vec<SourceId>,
    },

    /// The named home for what bug 2 used to be. A source that RECOGNISES a
    /// shape but rejects it in this position says so LOUDLY here, instead of
    /// forwarding it to an API that answers 404 two layers down.
    #[error("{by}: id shape rejected: {id} ({why})")]
    BadIdShape {
        by: SourceId,
        id: String,
        why: &'static str,
    },

    /// The source exists but has nothing to work with (no Qobuz client, no
    /// Plex credentials, no bound user directory).
    #[error("{by}: not configured ({why})")]
    NotConfigured { by: SourceId, why: &'static str },

    /// The reference is well-formed and owned, and the thing is not there.
    /// A zero-track resolve is this, with the item's id — there is deliberately
    /// no separate `Empty` variant (two variants for one condition is how the
    /// six vocabularies in survey §1 multiplied in the first place).
    #[error("{by}: {kind:?} {id} not found")]
    NotFound {
        by: SourceId,
        kind: ItemKind,
        id: String,
    },

    /// This source does not resolve that kind. In particular, local and LAN
    /// sources do not expose playlist resolution.
    #[error("{by}: {kind:?} is not supported")]
    Unsupported { by: SourceId, kind: ItemKind },

    /// Anything the underlying crate reported as a string.
    #[error("{by}: {msg}")]
    Backend { by: SourceId, msg: String },
}

impl SourceError {
    /// Which source produced this error, when one did.
    pub fn by(&self) -> Option<SourceId> {
        match self {
            SourceError::Unclaimed(_) | SourceError::Ambiguous { .. } => None,
            SourceError::BadIdShape { by, .. }
            | SourceError::NotConfigured { by, .. }
            | SourceError::NotFound { by, .. }
            | SourceError::Unsupported { by, .. }
            | SourceError::Backend { by, .. } => Some(*by),
        }
    }
}
