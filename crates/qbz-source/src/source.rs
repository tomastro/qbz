//! The trait. Four concerns, one `impl` block per source.
//!
//! A source cannot implement three of them and quietly skip artwork — the
//! compiler will not let it. That is the property the whole seam rests on:
//! today's four concerns are decided in 7 + 5 + 4 + ~15 separate places
//! (survey §1), and splitting them is exactly what makes a view forget one.

use qbz_models::QueueTrack;

use crate::art::{ArtRef, ArtSize};
use crate::error::SourceError;
use crate::id::{MediaRef, RawRef, SourceId};
use crate::meta::ItemMeta;
use crate::playback::PlaybackTicket;

/// One music source: Qobuz, the local filesystem, a Plex server — and, when it
/// arrives, Jellyfin or Navidrome.
#[async_trait::async_trait]
pub trait Source: Send + Sync + 'static {
    /// Which source this is.
    fn id(&self) -> SourceId;

    // ── OWNERSHIP / NORMALISATION ───────────────────────────────────────────

    /// Does this source own `raw`, and if so, what is the CANONICAL reference?
    ///
    /// - `None` — not mine; the registry asks the next source.
    /// - `Some(Ok(r))` — mine, normalised.
    /// - `Some(Err(e))` — MINE, and the shape is wrong in this position. The
    ///   walk STOPS here: a recognised-but-rejected shape is a
    ///   [`SourceError::BadIdShape`] at the moment of the mistake, not a
    ///   fallthrough to the next source and a 404 two layers down. This is bug
    ///   2's report path.
    ///
    /// ONE method, so a source cannot implement the recognition and forget the
    /// rejection — the exact "handles some sources and not others" shape the
    /// seam exists to kill.
    ///
    /// Implementations claim on POSITIVE evidence they own, never on "nobody
    /// else looked like it" (see [`crate::SourceRegistry::claim`]).
    fn claim(&self, raw: &RawRef) -> Option<Result<MediaRef, SourceError>>;

    // ── TRACKS ──────────────────────────────────────────────────────────────

    /// Expand `item` into playable queue tracks, IN ORDER.
    ///
    /// `source_item_id_hint` is left `None`: `qbz_mixtape::
    /// resolve_collection_tracks` stamps it centrally (enqueue.rs:52-56) and
    /// this crate must not fight it.
    async fn tracks(&self, item: &MediaRef) -> Result<Vec<QueueTrack>, SourceError>;

    // ── CONTEXT / METADATA ──────────────────────────────────────────────────

    /// Row-level display fields, WITHOUT expanding the item into tracks.
    async fn meta(&self, item: &MediaRef) -> Result<ItemMeta, SourceError>;

    // ── ARTWORK ─────────────────────────────────────────────────────────────

    /// CHEAP phase: memo lookups, at most one `stat`, and — for `LocalSource`
    /// only — at most ONE indexed `library.db` query on a cold cover miss.
    /// NEVER decodes, never hits the network, never opens a connection (the
    /// pool owns that).
    ///
    /// BLOCKING: the caller runs it on `spawn_blocking`, exactly where it does
    /// today (local_playback.rs:302, :313, :323).
    fn artwork(&self, item: &MediaRef, size: ArtSize) -> ArtRef;

    /// What a RAW artwork token from one of this source's OWN rows means.
    ///
    /// This is the artwork analogue of [`Source::claim`], and it exists for the
    /// same reason: the row mappers already hold the token
    /// (`LocalTrack.artwork_path`, `LocalAlbum.artwork_path`,
    /// `QueueTrack.artwork_url`) at the moment its provenance is still known,
    /// and the alternative was to throw that away into a `String` and sniff it
    /// back out later. `artwork_qt::classify` is that sniffing, and it is bug
    /// 3: it recognises a Plex thumb because somebody bolted
    /// `local_plex::is_thumb_path` onto it, so a Jellyfin token
    /// (`/Items/<id>/Images/Primary`) reads as a FILESYSTEM PATH — it starts
    /// with `/` — and the pipeline tries to open it from disk. Blank cover, no
    /// error, nowhere to look.
    ///
    /// Distinct from [`Source::artwork`] because the caller has no
    /// [`MediaRef`]: the windowed grid pipeline is keyed by `art_key` and holds
    /// no item (design §3.4). Going through `artwork` would mean claiming once
    /// per visible row AND letting the source look the token up again — for
    /// Plex that is one `plex_cache.db` query per row on a cold grid, on the
    /// exact grid whose scroll cost is already the open perf item.
    ///
    /// **MUST NOT touch the network, a database, or the filesystem.** The token
    /// is the whole input; this is a pure interpretation, called once per row
    /// per page load.
    fn artwork_token(&self, token: &str, size: ArtSize) -> ArtRef;

    /// EXPENSIVE phase: may decode + downscale.
    ///
    /// Takes an [`ArtRef`], NOT a [`MediaRef`] — the windowed pipeline it
    /// serves (`local_artwork::stream_cold`, local_artwork.rs:164-188) is keyed
    /// by `art_key` and has no item reference in scope (local_rows.rs:253,
    /// :283 → `local_state.art_index`).
    ///
    /// Defaults to identity; only `LocalSource` overrides it. The cheap/cold
    /// split is preserved verbatim from `local_artwork.rs` — collapsing the two
    /// is the documented "15-second blank grid" regression
    /// (local_artwork.rs:23-46).
    fn thumbnail(&self, art: &ArtRef, _size: ArtSize) -> ArtRef {
        art.clone()
    }

    // ── PLAYBACK ────────────────────────────────────────────────────────────

    /// What the player needs to actually start THIS track.
    ///
    /// `item` is the NORMALISED ref the registry already claimed — the source
    /// never re-derives its own id from `track`. `track` is passed only for the
    /// display/companion fields (`play_id`, cue offset) the ticket needs. The
    /// id the source's own API accepts is computed from `item.id()`, inside the
    /// source, and never escapes.
    async fn playback(
        &self,
        item: &MediaRef,
        track: &QueueTrack,
    ) -> Result<PlaybackTicket, SourceError>;

    // ── LIFECYCLE ───────────────────────────────────────────────────────────

    /// Bind to the active user's data directory.
    ///
    /// Called from ONE place: `auth_qt::bind_per_user_stores` (auth_qt.rs:202),
    /// and it must be that function's FIRST statement — `myqbz_qt::
    /// init_for_user` (auth_qt.rs:206) runs the mixtape migrations through
    /// `library_db_qt::with_db(true, …)`, and against an unbound pool a fresh
    /// account can never create a collection (library_db_qt.rs:41-49).
    fn bind_user(&self, _uid: u64, _dir: &std::path::Path) {}

    /// Drop every per-user handle and cache. Called from `auth_qt::logout`.
    ///
    /// **Overriding this is MANDATORY for any source that holds per-user
    /// state.** The default is a no-op, so an implementor that forgets it leaks
    /// the previous account's handle into a `'static` registry — the leak class
    /// this port already shipped once (the blacklist leak, auth_qt.rs:193-197).
    fn teardown(&self) {}
}
