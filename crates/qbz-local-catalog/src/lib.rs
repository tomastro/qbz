//! Frontend-agnostic derived catalog for Local Library.
//!
//! Source databases remain authoritative. This crate owns only a rebuildable
//! read projection and query primitives; it has no dependency on Qt, QML,
//! playback, or any source protocol/API.

mod bootstrap;
mod catalog;
mod legacy;
mod model;
mod projection;
mod schema;

pub use bootstrap::{
    ActiveCatalog, BootstrapBatch, BootstrapLayout, BootstrapManifest, BootstrapOutcome,
    BootstrapProgress, BootstrapSession, FallbackReason, GenerationPruneReport, PreflightReport,
    SourceCheckpoint, SourceProbe, BOOTSTRAP_BATCH_ROWS,
};
pub use catalog::{
    normalize_artist_key, normalize_sort_key, Catalog, CatalogStats, IntegrityReport, QueryMetrics,
    RuntimeIntegrityReport,
};
pub use legacy::{
    bootstrap_legacy_caches, bootstrap_legacy_caches_at_with_progress,
    bootstrap_legacy_caches_with_progress, discover_legacy_sources, discover_legacy_sources_at,
    reconcile_legacy_caches, reconcile_legacy_caches_at_with_progress,
    reconcile_legacy_caches_with_progress, LegacyLocations, LegacySourceSpec,
};
pub use model::{
    AlbumCursor, AlbumPage, AlbumRecord, ArtistCredit, ArtistCursor, ArtistPage, ArtistRecord,
    CreditRole, ProjectedTrack, QueryDescriptor, QuerySurface, SourceKey, SourceKind, TrackCursor,
    TrackGroup, TrackPage, TrackRecord, TrackRef, TrackSort,
};
pub use projection::{
    ProjectionOutcome, ProjectionProgress, ProjectionSession, ReconciliationBatch,
};
pub use schema::SCHEMA_VERSION;

/// Artwork stored locally by a media-server metadata sidecar. Qt strips this
/// marker and routes the remainder through the Local artwork provider instead
/// of handing a filesystem path back to Plex/Jellyfin/Subsonic as a token.
pub const SIDECAR_LOCAL_ART_PREFIX: &str = "qbz-local-art:";

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("catalog sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("catalog schema {found} is not supported (expected {expected})")]
    UnsupportedSchema { found: u32, expected: u32 },
    #[error("refusing to open a non-catalog SQLite database")]
    NotCatalog,
    #[error("invalid catalog input: {0}")]
    InvalidInput(String),
    #[error("search terms shorter than three characters are deferred")]
    SearchTooShort,
    #[error("a cursor from a different query descriptor cannot be reused")]
    CursorDescriptorMismatch,
    #[error("catalog I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("catalog manifest error: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("bootstrap source {0:?} changed since its saved checkpoint")]
    SourceSnapshotChanged(SourceKey),
    #[error("bootstrap checkpoint does not match the committed cursor for {0:?}")]
    CheckpointMismatch(SourceKey),
    #[error("bootstrap batch has {found} rows; the maximum is {maximum}")]
    BatchTooLarge { found: usize, maximum: usize },
    #[error("catalog preflight needs {required_bytes} bytes but only {available_bytes} are free")]
    InsufficientSpace {
        required_bytes: u64,
        available_bytes: u64,
    },
    #[error("catalog bootstrap source is invalid: {0}")]
    InvalidSource(String),
    #[error("catalog bootstrap is already running")]
    BootstrapBusy,
    #[error("catalog activation is not ready: {0}")]
    ActivationNotReady(String),
}

pub type Result<T> = std::result::Result<T, CatalogError>;

#[cfg(test)]
mod bootstrap_tests;
#[cfg(test)]
mod projection_tests;
#[cfg(test)]
mod tests;
