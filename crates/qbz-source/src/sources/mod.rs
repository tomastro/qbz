//! The five implementations that exist.
//!
//! The header used to say "three, and the one obvious slot a fourth would take:
//! a new file here, a new `pub mod` / `pub use` line, a `SourceId` constant, a
//! badge value and one line in `SourceRegistry::with_defaults`. Zero edits in
//! any view."
//!
//! That prediction is now cashed in twice over, and it held — with one honest
//! correction. It was true only AFTER stages 3 and 4 landed: the seam existed
//! from stage 1, but playback and artwork never asked it anything until then,
//! so a source added earlier would have been claimed correctly and stayed
//! silent and coverless. See `qbz-qt/src/source_wiring.rs` for what that cost.
//!
//! `remote` holds what Jellyfin and Subsonic share — the cache handle and the
//! row mappers — so the pair cannot drift the way the three copies of
//! `play_audible` did.

pub mod jellyfin;
pub mod local;
pub mod plex;
mod pool;
pub mod qobuz;
pub mod remote;
pub mod subsonic;

pub use jellyfin::{JellyfinCreds, JellyfinSource};
pub use local::{local_queue_track, EphemeralTracks, LocalSource};
pub use plex::{is_thumb_path, PlexCreds, PlexSource, PLEX_TRACK_ID_FLOOR};
pub use qobuz::{ClientFuture, ClientLens, QobuzSource};
pub use remote::CacheHandle;
pub use subsonic::{SubsonicCreds, SubsonicSource};
