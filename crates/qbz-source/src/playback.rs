//! What the player needs to start a track — as DATA.
//!
//! This crate does not link `qbz-player` or `qbz-audio` and therefore *cannot*
//! touch sample rate, resampling or device selection. The ticket says what to
//! do; the frontend performs the entry into the PROTECTED audio path, at the
//! same `play_data` / `play_dsd_file` / `play_track_resolved` seams it uses
//! today.

use std::path::PathBuf;

/// Everything the frontend needs to enter the PROTECTED audio path.
///
/// **NOT `#[non_exhaustive]`, deliberately.** `qbz-source` and `qbz-qt` ship in
/// the same workspace and are versioned together; the attribute would force the
/// frontend's `match` to carry a `_ =>` arm, which silently swallows a variant
/// added later. That is the opposite of the "the compiler will not let you skip
/// a source" property the whole design leans on.
///
/// That property already paid for itself once: the design (§3.3) predicted a
/// `Stream { .. }` variant "when the documented progressive-streaming follow-up
/// lands". It landed before stage 3 did — `local_playback::play_plex_track`
/// resolves the part URL and Range-streams it through
/// `qbz_player::remote_stream`, keeping the whole-file download only as the
/// fallback for a server that refuses Range. So [`PlaybackTicket::Stream`] is
/// here, and every frontend `match` had to be revisited to add it rather than
/// silently keeping the slower path.
#[derive(Clone, PartialEq)]
pub enum PlaybackTicket {
    /// Read this file and hand the bytes to `player().play_data(bytes, play_id)`.
    ///
    /// `seek_secs` is the CUE virtual-track offset (local_playback.rs:152-158,
    /// :177-180): every virtual track of a CUE album shares ONE audio file, so
    /// the frontend seeks after the load lands.
    File {
        path: PathBuf,
        play_id: u64,
        seek_secs: Option<f64>,
    },
    /// Stream from disk: `player().play_dsd_file(path, play_id)`
    /// (local_playback.rs:139-149). DSD stays on its additive path, untouched.
    DsdFile { path: PathBuf, play_id: u64 },
    /// The source already fetched the ORIGINAL bytes (Plex direct-play, no
    /// transcode requested): `player().play_data(bytes, play_id)`
    /// (local_playback.rs:199-206).
    Bytes { bytes: Vec<u8>, play_id: u64 },
    /// Range-stream the ORIGINAL bytes from `url` into the player:
    /// `qbz_player::remote_stream::stream_remote_track_into_player`. Audio
    /// starts on the first chunk instead of after the whole FLAC is in RAM.
    ///
    /// **The source resolved a URL, it did not fetch a body.** For Plex that
    /// is `plex_resolve_part_url`; for the media servers arriving in stage 5 it
    /// is a direct `?static=true` / `format=raw` URL. Every one of them serves
    /// `Content-Length` + `Accept-Ranges: bytes`, which is exactly what the
    /// feeder needs, and none of them is asked to transcode.
    ///
    /// **The frontend owns the fallback**, and it is a plain GET of THIS SAME
    /// url handed to `play_data` — a server that refuses Range still plays,
    /// just slowly. That is what `local_playback::play_plex_track` does today,
    /// except it re-runs the whole metadata round trip to rebuild a url it
    /// already had; carrying the url on the ticket removes that second trip.
    ///
    /// `url` may embed credentials (Plex's `?X-Plex-Token=`, Jellyfin's
    /// `?api_key=`, Subsonic's `?u=&t=&s=`). It is a SECRET-BEARING string:
    /// never log it whole.
    Stream {
        url: String,
        play_id: u64,
        /// The feeder's buffer maths input. `0` is acceptable — it only makes
        /// the estimate conservative (local_playback.rs:361-367).
        duration_secs: u64,
        /// Where to start. `0` for a normal play.
        start_secs: u64,
        /// Log prefix the feeder stamps its lines with (`"PLEX"`). Static
        /// because it names the SOURCE, never the item.
        log_tag: &'static str,
        /// Source-owned HTTP request headers required by the media endpoint.
        ///
        /// These are transport facts, not audio policy. Plex, for example,
        /// accepts the token in the URL but can reject a part request without
        /// its `X-Plex-*` client identity. Carrying the headers on the ticket
        /// keeps the shared player transport source-agnostic while preserving
        /// the request contract the source used to fetch the same bytes.
        request_headers: Vec<(String, String)>,
    },
    /// Let the core resolve + stream it: `core().play_track_resolved(track_id, …)`.
    Catalog { track_id: u64 },
    /// The current track is already loaded and this is a seek within the same
    /// container — the CUE fast path (local_playback.rs:150-158).
    ///
    /// NOTE (stage-1 status): **no source produces this yet.** The "is this
    /// container already loaded?" test reads `player().state.current_track_id()`
    /// and `has_loaded_audio()`, which this crate cannot link (§8 — the
    /// PROTECTED backend is not a dependency). `LocalSource::playback` emits
    /// `File { seek_secs: Some(_) }` instead and the frontend keeps making that
    /// decision where it makes it today. Kept because the design specifies it;
    /// if stage 3 confirms the frontend never wants a source to say it, this is
    /// the variant to cut.
    SeekLoaded { play_id: u64, secs: f64 },
    /// A CD-DA track: read the drive's raw sectors and feed them to the
    /// player as 44.1 kHz / 16-bit / stereo PCM.
    ///
    /// A track on a disc is NOT a file, so no path-shaped variant fits it: it
    /// is a device plus a sector RANGE, and the bytes only exist while that
    /// medium is in that drive. It is also the one ticket whose source can be
    /// physically removed mid-play, which is why the feeder that performs it
    /// must stop on a read error instead of filling the gap with zeros —
    /// silence indistinguishable from music is the failure a bit-perfect
    /// player must never ship.
    CdTrack {
        device: PathBuf,
        start_lsn: u32,
        /// Sectors of 2352 bytes. `sectors * 588` is the PCM frame count the
        /// player is promised up front.
        sectors: u32,
        play_id: u64,
    },
}

/// Hand-written so `Bytes` prints its LENGTH, not a megabyte of FLAC, and so
/// `Stream` prints its url's ORIGIN, not the token in its query string. A
/// derived `Debug` would dump the whole buffer into any log line that formats
/// a ticket — and, since the ticket arrived, the user's Plex/Jellyfin/Subsonic
/// credentials with it.
impl std::fmt::Debug for PlaybackTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaybackTicket::File {
                path,
                play_id,
                seek_secs,
            } => f
                .debug_struct("File")
                .field("path", path)
                .field("play_id", play_id)
                .field("seek_secs", seek_secs)
                .finish(),
            PlaybackTicket::DsdFile { path, play_id } => f
                .debug_struct("DsdFile")
                .field("path", path)
                .field("play_id", play_id)
                .finish(),
            PlaybackTicket::Bytes { bytes, play_id } => f
                .debug_struct("Bytes")
                .field("len", &bytes.len())
                .field("play_id", play_id)
                .finish(),
            PlaybackTicket::Stream {
                url,
                play_id,
                duration_secs,
                start_secs,
                log_tag,
                request_headers,
            } => f
                .debug_struct("Stream")
                .field("url", &redact_url(url))
                .field("play_id", play_id)
                .field("duration_secs", duration_secs)
                .field("start_secs", start_secs)
                .field("log_tag", log_tag)
                // Header VALUES may become sensitive for a future source.
                // Names are enough to diagnose a missing request profile.
                .field(
                    "request_headers",
                    &request_headers
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>(),
                )
                .finish(),
            PlaybackTicket::Catalog { track_id } => f
                .debug_struct("Catalog")
                .field("track_id", track_id)
                .finish(),
            PlaybackTicket::SeekLoaded { play_id, secs } => f
                .debug_struct("SeekLoaded")
                .field("play_id", play_id)
                .field("secs", secs)
                .finish(),
            PlaybackTicket::CdTrack {
                device,
                start_lsn,
                sectors,
                play_id,
            } => f
                .debug_struct("CdTrack")
                .field("device", device)
                .field("start_lsn", start_lsn)
                .field("sectors", sectors)
                .field("play_id", play_id)
                .finish(),
        }
    }
}

/// Everything up to the `?`, plus a marker. Enough to tell a LAN address from
/// a plex.tv relay in a log line — which is the question those log lines are
/// there to answer (local_playback.rs:344-347) — without printing the token
/// that follows.
fn redact_url(url: &str) -> String {
    match url.split_once('?') {
        Some((origin, _)) => format!("{origin}?<redacted>"),
        None => url.to_string(),
    }
}

impl PlaybackTicket {
    /// The player-side id this ticket plays under, when it has one.
    pub fn play_id(&self) -> Option<u64> {
        match self {
            PlaybackTicket::File { play_id, .. }
            | PlaybackTicket::DsdFile { play_id, .. }
            | PlaybackTicket::Bytes { play_id, .. }
            | PlaybackTicket::Stream { play_id, .. }
            | PlaybackTicket::SeekLoaded { play_id, .. }
            | PlaybackTicket::CdTrack { play_id, .. } => Some(*play_id),
            PlaybackTicket::Catalog { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ticket must never carry a credential into a log line. `Debug` is what
    /// a `log::debug!("{ticket:?}")` reaches for, so it is what has to be safe.
    #[test]
    fn stream_debug_redacts_the_query_string() {
        let t = PlaybackTicket::Stream {
            url: "http://192.168.0.69:32400/library/parts/1/f.flac?X-Plex-Token=sekrit".into(),
            play_id: 7,
            duration_secs: 188,
            start_secs: 0,
            log_tag: "PLEX",
            request_headers: vec![("X-Plex-Product".into(), "secret-value".into())],
        };
        let s = format!("{t:?}");
        assert!(!s.contains("sekrit"), "the token leaked into Debug: {s}");
        assert!(s.contains("X-Plex-Product"), "{s}");
        assert!(
            !s.contains("secret-value"),
            "header value leaked into Debug: {s}"
        );
        assert!(s.contains("/library/parts/1/f.flac?<redacted>"), "{s}");
        // The origin SURVIVES: telling a LAN address from a plex.tv relay is
        // the question those perf log lines exist to answer.
        assert!(s.contains("192.168.0.69:32400"), "{s}");
    }

    #[test]
    fn a_url_without_a_query_string_is_left_alone() {
        let t = PlaybackTicket::Stream {
            url: "http://nas.local/music/a.flac".into(),
            play_id: 1,
            duration_secs: 0,
            start_secs: 0,
            log_tag: "PLEX",
            request_headers: Vec::new(),
        };
        assert!(format!("{t:?}").contains("http://nas.local/music/a.flac"));
    }

    #[test]
    fn stream_carries_a_play_id_like_every_other_audible_variant() {
        let t = PlaybackTicket::Stream {
            url: "http://x/y".into(),
            play_id: 42,
            duration_secs: 0,
            start_secs: 0,
            log_tag: "PLEX",
            request_headers: Vec::new(),
        };
        assert_eq!(t.play_id(), Some(42));
        // `Catalog` is the one variant with no player-side id — the core mints
        // it. Asserted next to `Stream` so a future variant added without a
        // `play_id()` arm shows up here rather than as a silent `None`.
        assert_eq!(PlaybackTicket::Catalog { track_id: 9 }.play_id(), None);
    }
}
