//! Offline cache for QBZ — frontend-agnostic.
//!
//! Downloading Qobuz content for offline playback and managing the local
//! store: the on-disk CMAF bundle store, the per-track secret vault, the
//! SQLite index, maintenance/purge/migration, and the pure cached-playback
//! resolution.
//!
//! Extracted out of `src-tauri/src/offline_cache/` so both the Tauri and
//! the Slint frontends share ONE implementation (ADR-006). The download
//! pipeline emits progress through a `CacheEventSink` callback instead of
//! Tauri events, so this crate has no Tauri dependency.
//!
//! Migration status (Slice 0 — extraction): modules are moved here
//! incrementally; `src-tauri` keeps its own copy until the re-point step,
//! so the workspace stays green throughout.

pub mod cmaf_store;
pub mod db;
pub mod downloader;
pub mod event;
pub mod maintenance;
pub mod metadata;
pub mod migration;
pub mod path_validator;
pub mod playback;
pub mod purchases_service;
pub mod purge;
pub mod secret_vault;
pub mod state;
pub mod types;

pub use db::{CmafBundleRow, OfflineCacheDb};
pub use downloader::{spawn_track_cache_download, StreamFetcher};
pub use event::{CacheEvent, CacheEventSink, CacheFormat};
pub use metadata::{sanitize_filename, CompleteTrackMetadata};
pub use purge::purge_all_cached_files;
pub use state::OfflineCacheState;
pub use migration::{
    detect_legacy_cached_files, migrate_legacy_cached_files, MigrationError, MigrationStatus,
};
pub use path_validator::{is_offline_root_available, validate_path, PathStatus};
pub use playback::{load_cmaf_bundle, load_cmaf_bundle_with_ui_events};
pub use types::{
    CacheProgress, CachedTrackInfo, OfflineCacheStats, OfflineCacheStatus, ReadyTrackForSync,
    TrackCacheInfo,
};
