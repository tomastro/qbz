//! Qobuz account migration (contract:
//! `qbz-nix-docs/specs/2026-09-02-account-migration-contract.md`, §9).
//!
//! Two halves, both frontend-agnostic (ADR 006):
//!
//! 1. [`snapshot`]: while signed in with the SOURCE account, capture its
//!    favorites (five kinds, with `favorited_at`), its own playlists with
//!    their track ids in order, and the playlists it follows, into one JSON
//!    file in that account's profile directory.
//! 2. [`plan`] + [`apply`]: while signed in with the TARGET account, read a
//!    snapshot, compute the delta against the target's live state (captured
//!    with the same code) and the [`ledger`] of earlier runs, and write it
//!    — additively, never deleting on either side. Every write lands in the
//!    ledger before the next one starts, so a run can be resumed and re-run
//!    to "0 changes".
//!
//! Nothing here touches subscriptions, purchases or account data: the
//! writes are the same `favorite/create`, `playlist/create`,
//! `playlist/addTracks` and `playlist/subscribe` the app already issues
//! when the user hearts or creates something.

pub mod api;
pub mod apply;
pub mod ledger;
pub mod local;
pub mod plan;
pub mod sink;
pub mod snapshot;

pub use api::MigrationApi;
pub use apply::{apply, ApplyReport, SectionReport};
pub use ledger::Ledger;
pub use local::{copy_profile, LocalOptions, LocalReport};
pub use plan::{plan, CloudPlan, PlaylistAction};
pub use sink::{MigrationEvent, MigrationPhase, MigrationSink};
pub use snapshot::{
    capture, AccountSnapshot, FavoriteItem, FavoriteKind, OwnedPlaylist, SubscribedPlaylist,
};
