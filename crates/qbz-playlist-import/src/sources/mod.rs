//! Where a playlist can come FROM.
//!
//! Design: `qbz-nix-docs/deferred-2.0.3/playlist-importer-expansion-design.md`.
//!
//! # An enum, not a trait (ADR-007)
//!
//! The set of source classes is CLOSED and known at compile time. There is no
//! plugin story, no registry, nothing loaded at runtime — so a trait plus
//! dynamic dispatch would buy an extension point nobody can use and cost an
//! indirection plus a `Box<dyn>` on every path. Adding a source here is a new
//! variant and one arm of [`PlaylistSource::resolve`], and the compiler names
//! every place that has to grow.
//!
//! # Everything lands on ONE type
//!
//! Every arm returns [`ImportPlaylist`], which is exactly what the existing
//! URL scrapers already produce. Downstream — the Qobuz matcher, the 2000-track
//! split, the create-and-add loop, the progress sink, the summary — never
//! learns that a second kind of source exists. That is the whole reason the
//! expansion is additive: `importer::import_prepared_playlist` takes the
//! resolved playlist and does not care who resolved it.
//!
//! # The byte wall is here, not per-parser
//!
//! [`MAX_IMPORT_BYTES`] is checked before any parse. The APP checks it too,
//! before it reads the file at all (a 2 GB pick must never reach RAM); this is
//! defense in depth for every other caller, and it is O(1) on a length.

pub mod file;
pub mod json;
pub mod service;

use crate::errors::PlaylistImportError;
use crate::models::ImportPlaylist;

/// Refuse anything larger, before allocating or parsing.
///
/// 16 MiB. The largest realistic export — 10 000 tracks at ~1.5 KB of verbose
/// metadata — is about 15 MB; a 500-track export is under 1 MB. So this is
/// comfortably above legitimate use and roughly three orders of magnitude below
/// anything that could pressure the box. It is ONE constant: dropping it to
/// 8 MiB costs no code.
pub const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;

/// Refuse a body over the wall. Shared by every byte-taking source.
pub(crate) fn guard_size(bytes: &[u8]) -> Result<(), PlaylistImportError> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(PlaylistImportError::FileTooLarge);
    }
    Ok(())
}

/// The playlist-file formats this build reads. `M3u8` is not a variant: an
/// `.m3u8` is an M3U that promises to be UTF-8, and the decoder already tries
/// UTF-8 first — a second variant would fork the parser for an encoding hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    M3u,
    Pls,
    Xspf,
}

/// The three Last.fm player stations reachable without authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastFmStation {
    Library,
    Mix,
    Recommended,
}

impl LastFmStation {
    /// The path segment under `/player/station/user/<user>/`.
    pub fn slug(self) -> &'static str {
        match self {
            LastFmStation::Library => "library",
            LastFmStation::Mix => "mix",
            LastFmStation::Recommended => "recommended",
        }
    }

    /// Index order of the station picker (0 Library / 1 Mix / 2 Recommended).
    pub fn from_index(i: i32) -> LastFmStation {
        match i {
            1 => LastFmStation::Mix,
            2 => LastFmStation::Recommended,
            _ => LastFmStation::Library,
        }
    }
}

/// One resolvable playlist source.
///
/// `File` and `Json` carry BYTES, not a path, and that is deliberate: the crate
/// stays filesystem-free (ADR-006 — it runs headless, in tests, and on a
/// sandboxed Flatpak where the app owns the portal). The app reads through
/// `rfd` and hands the bytes over with the filename, which is used for the
/// playlist name and as a format hint.
#[derive(Debug, Clone)]
pub enum PlaylistSource {
    /// A streaming-provider URL — the pre-expansion path, untouched.
    Url(String),
    File {
        format: FileFormat,
        bytes: Vec<u8>,
        filename: String,
    },
    Json {
        bytes: Vec<u8>,
        filename: String,
    },
    /// A ListenBrainz playlist by MBID. The token is optional — public reads
    /// work without one; with one the user's own private playlists resolve and
    /// the rate limit is higher.
    ListenBrainz {
        mbid: String,
        token: Option<String>,
    },
    LastFmStation {
        user: String,
        station: LastFmStation,
    },
    LastFmPlaylist {
        user: String,
        playlist_id: String,
    },
}

impl PlaylistSource {
    pub async fn resolve(&self) -> Result<ImportPlaylist, PlaylistImportError> {
        match self {
            // The File / Json arms are pure synchronous work in an async fn.
            // They are cheap (a bounded parse over at most 16 MiB) and the
            // caller is already on a tokio task, so there is nothing to move
            // to a blocking pool.
            PlaylistSource::Url(u) => crate::importer::preview_public_playlist(u).await,
            PlaylistSource::File {
                format,
                bytes,
                filename,
            } => file::parse(*format, bytes, filename),
            PlaylistSource::Json { bytes, filename } => json::parse(bytes, filename),
            PlaylistSource::ListenBrainz { mbid, token } => {
                service::listenbrainz::fetch(mbid, token.as_deref()).await
            }
            PlaylistSource::LastFmStation { user, station } => {
                service::lastfm::fetch_station(user, *station).await
            }
            PlaylistSource::LastFmPlaylist { user, playlist_id } => {
                service::lastfm::fetch_playlist(user, playlist_id).await
            }
        }
    }

    /// A short label for the log line, before the playlist is resolved.
    pub fn label(&self) -> String {
        match self {
            PlaylistSource::Url(u) => u.trim().to_string(),
            PlaylistSource::File { filename, .. } | PlaylistSource::Json { filename, .. } => {
                filename.clone()
            }
            PlaylistSource::ListenBrainz { mbid, .. } => mbid.clone(),
            PlaylistSource::LastFmStation { user, station } => {
                format!("{user}/{}", station.slug())
            }
            PlaylistSource::LastFmPlaylist { user, playlist_id } => {
                format!("{user}/playlists/{playlist_id}")
            }
        }
    }

    /// Whether resolving this source needs the network. File and JSON do not,
    /// which is what lets a preview succeed offline (the IMPORT half still
    /// needs a session, and is gated separately).
    pub fn needs_network(&self) -> bool {
        !matches!(
            self,
            PlaylistSource::File { .. } | PlaylistSource::Json { .. }
        )
    }
}
