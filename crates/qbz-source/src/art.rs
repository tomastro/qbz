//! Artwork as DATA a source PRODUCES, instead of a string a caller sniffs.
//!
//! [`ArtRef`] generalises `artwork_qt::ArtUrl` (artwork_qt.rs:124-142) with one
//! change that is the whole of bug 3: it is produced by the source instead of
//! classified from a string by the caller. `artwork_qt::classify`
//! (artwork_qt.rs:147-180) is DELETED when its last caller moves — it only ever
//! existed because the string had already lost its provenance by the time a
//! view got it.
//!
//! A hybrid list then does ONE thing per row, identical for a Qobuz album, a
//! local folder and a Plex row:
//!
//! ```ignore
//! match registry.artwork(&item_ref, ArtSize::Row) {
//!     ArtRef::File(p)                       => row.art_path = file_url(&p),
//!     ArtRef::Fetch { url, cache_key }      => misses.push((cache_key, url)),
//!     ArtRef::None | ArtRef::Unavailable(_) => {}
//! }
//! ```

use std::path::PathBuf;

/// The drawn size class. The source picks the cheapest representation that
/// satisfies it; it is NOT a pixel promise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArtSize {
    /// ~50 px — MyQBZ / queue / now-playing rows.
    Row,
    /// ~256 px — grid cards.
    ///
    /// Matches `artwork_qt::PLEX_THUMB_PX` (artwork_qt.rs:78), which is
    /// deliberately ONE app-wide Plex transcode size so a cover is downloaded
    /// once. Do not add a second Plex size.
    Card,
    /// Full — hero headers, immersive.
    Full,
}

impl ArtSize {
    /// The Plex server-side transcode edge in px, or `None` for "raw
    /// full-res". One size app-wide (see [`ArtSize::Card`]).
    pub fn plex_px(self) -> Option<u32> {
        match self {
            ArtSize::Row | ArtSize::Card => Some(PLEX_THUMB_PX),
            ArtSize::Full => None,
        }
    }

    /// The Qobuz CDN `_<size>.jpg` token this class rewrites to. Mirrors the
    /// per-cell targets `myqbz_qt::small_qobuz_url` is called with
    /// (myqbz_qt.rs:404-407: `<=80 -> 50`, `<=200 -> 150`, else 300).
    pub fn qobuz_px(self) -> u32 {
        match self {
            ArtSize::Row => 50,
            ArtSize::Card => 300,
            ArtSize::Full => 600,
        }
    }
}

/// Server-side transcode size for EVERY Plex thumb this process fetches.
/// Moved verbatim from `artwork_qt::PLEX_THUMB_PX` (artwork_qt.rs:78): one size
/// app-wide is deliberate, because the cache key is the final tokenized url and
/// a second size would mean a second download of the same cover.
pub const PLEX_THUMB_PX: u32 = 256;

/// What a source says its art IS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtRef {
    /// Nothing to show.
    None,
    /// ALREADY on the local filesystem — a raw path, no `file://`, never
    /// fetched. Local Library covers, `qbz-library` thumbnails and
    /// offline-download covers.
    File(PathBuf),
    /// Fetchable http(s).
    ///
    /// `cache_key` is the STABLE key the caller memoizes under; `url` is what
    /// it GETs. They differ for Plex, whose tokenized url is rebuilt every pass
    /// while the `/library/...` path is stable — exactly the split
    /// `artwork_qt::disk_path(key, fetch)` already relies on
    /// (artwork_qt.rs:231-238).
    Fetch { url: String, cache_key: String },
    /// The art exists but cannot be resolved right now (Plex not connected, a
    /// Qobuz cover not fetched yet). Distinct from [`ArtRef::None`] so the miss
    /// is logged for what it is — `ArtUrl::PlexUnconfigured`'s reason for
    /// existing (artwork_qt.rs:139-141).
    Unavailable(&'static str),
}

impl Default for ArtRef {
    fn default() -> Self {
        ArtRef::None
    }
}

impl ArtRef {
    /// True when there is nothing a caller can render or fetch.
    pub fn is_empty(&self) -> bool {
        matches!(self, ArtRef::None | ArtRef::Unavailable(_))
    }

    /// A raw filesystem path, when the art is already on disk.
    pub fn as_path(&self) -> Option<&std::path::Path> {
        match self {
            ArtRef::File(p) => Some(p.as_path()),
            _ => None,
        }
    }
}
