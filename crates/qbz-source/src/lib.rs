//! `qbz-source` — the source-agnostic seam.
//!
//! # The problem this exists to delete
//!
//! Views mix items from Qobuz, local folders and Plex in ONE list, and every
//! view re-derives the per-source handling by hand. Four concerns that should
//! be one seam each are decided in **7 + 5 + 4 + ~15** separate places
//! (`01-survey.md` §1), and splitting them is mechanically why "siempre falta
//! algo": a view that learned two of the three sniffs silently drops the third,
//! and nothing fails loudly — it renders a placeholder disc, returns an empty
//! track list, or 404s a resolve.
//!
//! | concern | Qobuz path today | local path today | Plex path today | who wires it |
//! |---|---|---|---|---|
//! | tracks | `qbz_mixtape::resolve_qobuz_album` | `LocalSource::tracks` | `PlexSource::tracks` | `SourceRegistry` |
//! | artwork | `artwork_qt::cached_path` | `local_artwork::local_thumbnail` | `local_plex::thumb_url` | every view |
//! | playback | `core().play_track_resolved` | `play_local_file` | `play_plex_track` | 2 duplicated routers |
//! | context/meta | `to_item` + resolve cache | `map_track` / `map_album` | same, plus merges | every view |
//!
//! This crate collapses that table into **one trait with four methods and one
//! registry**: [`Source`] and [`SourceRegistry`].
//!
//! # The three measured bugs, and where each dies
//!
//! 1. **`library.db` reopened per item** (23–34 opens for ONE MyQBZ
//!    collection). `LocalSource` owns a connection pool; `local_state::with_db`
//!    and `library_db_qt::with_db` keep their signatures and forward to
//!    [`sources::LocalSource::with`] / `with_creating`, so their 48 call sites
//!    are untouched and only the number of opens changes.
//! 2. **A `plex:<hash>` album key used as a rating key** → 404, the track never
//!    starts and the PREVIOUS track keeps playing under the new card. Killed by
//!    four independent properties: [`MediaRef`] has no public constructor;
//!    [`SourceRegistry::claim`] is the single normalisation point on EVERY path
//!    including playback; [`PlaybackTicket`] is opaque data with no field a
//!    source-specific id could be smuggled through; and there is exactly one
//!    router.
//! 3. **Missing artwork on the local rows of a hybrid list.** [`Source::artwork`]
//!    is on the trait, so a hybrid list makes ONE call per row regardless of
//!    source and cannot wire fewer pipelines than it has sources.
//!
//! # The owner's rules, and where they live
//!
//! [`acceptance`] is the ONE home of **R1** (a Collection takes albums, singles
//! and EPs from any source; a Mixtape takes albums, playlists and tracks) and
//! **R3** (an ephemeral item may only be played and queued — never stored in a
//! playlist, a mixtape or a collection). Ask it with [`Container::accepts`].
//!
//! **R2** — an unsupported combination must never be VISIBLE — is why the rule
//! belongs here rather than in a view: the resolver errors
//! (`"local playlists not supported in this release"`, [`SourceError`]) stay
//! exactly where they are as the safety net that only reaches the log, and the
//! UI simply does not offer what cannot work. **R4** — a row's source comes
//! from the ROW, never a literal — is enforced by shape: [`ItemFacts`] is built
//! from a [`RawRef`] / [`MediaRef`], and the rule contains no source literal to
//! get wrong.
//!
//! # Shape rules
//!
//! - **One provider seam.** `qbz-mixtape` retains generic collection expansion
//!   and Qobuz mapping; local and LAN provider resolution lives here. This
//!   crate implements `qbz_mixtape::ItemResolver`, it does not absorb it.
//! - **Nothing long-lived is cached that another crate owns.** The Qobuz client
//!   is READ THROUGH a [`ClientLens`] on every call, because `qbz-core`
//!   replaces its client wholesale (`core.rs:346`, `:384`) and a cached clone
//!   fails silently. Install it once with [`init_registry`].
//! - **The PROTECTED audio backend is not linked.** `qbz-audio` and
//!   `qbz-player` are not dependencies, so sample rate, resampling and device
//!   selection are unreachable from here. [`PlaybackTicket`] is data; the
//!   frontend performs the entry.
//! - **No executor.** This crate never spawns and does not depend on `tokio`.
//!   The blocking arms (SQLite, `stat`) stay blocking and the caller keeps
//!   wrapping them in `spawn_blocking` exactly where it does today.
//! - **Three impls, one registry, one obvious slot for a fourth.** No dynamic
//!   registry, no config loading, no trait hierarchy.
//!
//! # Adding a fourth source (Jellyfin, Navidrome)
//!
//! `sources/jellyfin.rs` (one impl), one `pub mod` + `pub use` in
//! [`sources`], one [`SourceId`] constant, one word in
//! [`SourceId::from_word`], one [`SourceBadge`] variant, and one line in
//! [`SourceRegistry`]'s `build`. **Zero** changes in any view, any
//! playback path, any artwork path — because all four concerns hang off the one
//! trait. And zero changes in [`acceptance`]: R1 says a Collection takes
//! releases "from ANY source", so the rule has no per-source arm to extend.

pub mod acceptance;
pub mod art;
pub mod error;
pub mod id;
pub mod meta;
pub mod mixtape_adapter;
pub mod playback;
pub mod registry;
pub mod source;
pub mod sources;

pub use acceptance::{
    is_ephemeral_id, is_release_word, Accepted, Container, ItemFacts, Refusal, RELEASE_TYPES,
};
pub use art::{ArtRef, ArtSize, PLEX_THUMB_PX};
pub use error::SourceError;
pub use id::{ItemKind, MediaRef, RawRef, SourceId};
pub use meta::{ItemMeta, QualityHint, SourceBadge};
pub use mixtape_adapter::RegistryResolver;
pub use playback::PlaybackTicket;
pub use registry::{init_registry, registry, SourceRegistry};
pub use source::Source;
pub use sources::{
    local_queue_track, CacheHandle, ClientFuture, ClientLens, EphemeralTracks, JellyfinCreds,
    JellyfinSource, LocalSource, PlexCreds, PlexSource, QobuzSource, SubsonicCreds, SubsonicSource,
};
