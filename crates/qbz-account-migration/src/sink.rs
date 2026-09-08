//! Frontend-agnostic progress reporting (the playlist importer's shape).

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationPhase {
    ReadingSource,
    ReadingTarget,
    Favorites,
    Playlists,
    Subscriptions,
    Done,
}

#[derive(Debug, Clone, Serialize)]
pub enum MigrationEvent {
    Phase(MigrationPhase),
    /// `done` of `total` units in the current phase, with a short label for
    /// the status line (a playlist name, a favorites kind).
    Progress {
        done: usize,
        total: usize,
        label: String,
    },
}

pub trait MigrationSink: Send + Sync {
    fn emit(&self, event: MigrationEvent);
}

impl<F: Fn(MigrationEvent) + Send + Sync> MigrationSink for F {
    fn emit(&self, event: MigrationEvent) {
        self(event)
    }
}

/// A sink that drops everything (tests, the CLI's quiet mode).
pub struct NullSink;

impl MigrationSink for NullSink {
    fn emit(&self, _event: MigrationEvent) {}
}
