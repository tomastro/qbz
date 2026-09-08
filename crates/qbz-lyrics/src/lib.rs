//! QBZ lyrics engine — headless, frontend-agnostic (ADR-006).
//!
//! One crate = the whole lyrics engine, reusable by Slint, Tauri and the TUI:
//!
//! - [`model`] — domain model ([`LyricsDoc`] structured lines, the
//!   Tauri-wire-compatible [`LyricsPayload`], cache-key builder).
//! - [`lrc`] — LRC parser + emitter. One Rust home for the semantics the
//!   Tauri frontend implemented in TypeScript (`src/lib/stores/lyricsStore.ts:131-185`):
//!   empty-text gap markers as end-of-vocal bounds, the 8s `MAX_SUNG_MS` cap,
//!   the median last-line end estimate. Fixes forward the F5 defects
//!   (multi-stamp lines, `[offset:]` tag).
//! - [`wsync`] — Qobuz word-synced document mirror: the persistable native
//!   form (additive `qobuz_wsync_json` cache column, amended Q5) and its
//!   conversions to the domain model / LRC.
//! - [`providers`] — external fallback providers ported VERBATIM from
//!   `src-tauri/src/lyrics/providers.rs`: LRCLIB (search-first + scorer) and
//!   lyrics.ovh (plain-only). Request shapes are byte-identical.
//! - [`cache`] — per-user SQLite cache, same schema/path Tauri uses
//!   (`<user cache dir>/lyrics/lyrics.db`, WAL per ADR-002) plus the additive
//!   `qobuz_wsync_json` column.
//! - [`service`] — the Qobuz-first orchestrator
//!   (spec `qbz-nix-docs/lyrics/2026-06-10-lyrics-slint-port-spec.md` §1.1):
//!   cache probe (plain-only = soft miss) -> Qobuz primary -> LRCLIB ->
//!   lyrics.ovh, offline cache-only mode, in-flight dedupe (F6), stale-guard
//!   echo (F2 support).
//! - [`sync`] — sync-engine pure functions (spec §4.2): active-line binary
//!   search, per-line progress with the 0.99 snap, the word-anchored karaoke
//!   clip fraction (Q2).
//!
//! No frontend types anywhere below this line — the Slint/Tauri glue is a
//! thin adapter over [`service::LyricsService`].

pub mod cache;
pub mod lrc;
pub mod model;
pub mod providers;
pub mod service;
pub mod sync;
pub mod wsync;

pub use cache::{CachedLyrics, LyricsCacheDb, LyricsCacheStats};
pub use lrc::{emit_lrc, parse_lrc, parse_plain, LrcSpan, MAX_SUNG_MS};
pub use model::{
    build_cache_key, derive_has_translation, LyricsDoc, LyricsKind, LyricsLine, LyricsPayload,
    LyricsProvider, TranslatedLyrics, Word,
};
pub use providers::LyricsData;
pub use service::{
    HttpLyricsProviders, LyricsOutcome, LyricsProviders, LyricsRequest, LyricsResponse,
    LyricsResult, LyricsService, LyricsSourceKind,
};
pub use sync::{find_active_line_index, line_fill_fraction, line_progress};
pub use wsync::{QobuzWsync, QobuzWsyncLine, QobuzWsyncWord};
