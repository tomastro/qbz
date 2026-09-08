//! qbz-radio — agnostic smart-radio pool builder.
//!
//! Ported verbatim (logic-wise) from the Tauri `radio_engine`, but
//! decoupled from Tauri: it depends only on `qbz-qobuz` (the API
//! client) and `qbz-models` (the domain types) plus `rusqlite` for
//! the local radio pool DB. Any frontend (Slint, TUI, CLI) can build
//! and play a smart radio without Tauri.

pub mod builder;
pub mod db;
pub mod engine;

pub use builder::{BuildRadioOptions, RadioPoolBuilder};
pub use db::{RadioDb, RadioSeed, RadioSession, RadioTrackRef};
pub use engine::RadioEngine;

#[cfg(test)]
mod tests;
