//! Mixtapes & Collections backend — schema, repository, shuffle/DJ-mix, and
//! enqueue/resolution logic for QBZ.
//!
//! Runs headless over shared models and Qobuz APIs, while provider-specific
//! local/LAN resolution remains in `qbz-source` (ADR-006).
//!
//! - [`schema`]  — SQLite migrations (`run_mixtape_migrations`).
//! - [`repo`]    — CRUD over `&Connection` / `&mut Connection`.
//! - [`shuffle`] — pure DJ-mix sampler (`rand`, `strsim`).
//! - [`enqueue`] — `ItemResolver` trait, `resolve_collection_tracks`,
//!   `shuffle_items`, `next_item_index` / `previous_item_index`, plus the
//!   Qobuz resolver fns + `ProdItemResolver`.

pub mod enqueue;
pub mod repo;
pub mod schema;
pub mod shuffle;

// Convenience re-exports for application callers.
pub use enqueue::{
    next_item_index, previous_item_index, resolve_collection_tracks, ItemResolver, ProdItemResolver,
};
pub use schema::run_mixtape_migrations;
