//! Daemon compatibility shim for the shared QConnect authority barrier.
//!
//! The implementation lives in `qconnect-app` so every frontend can use the
//! exact same stamps, action permits, and handoff drain boundary.

pub use qconnect_app::{
    AuthorityActionPermit, AuthorityCell, AuthorityOrigin, AuthorityStamp,
    OwnerAuthorityObservation, OwnerAuthorityToken,
};
