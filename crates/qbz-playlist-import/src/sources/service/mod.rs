//! Public-read services: ListenBrainz and Last.fm.
//!
//! # No authentication, by design
//!
//! Both arms read PUBLIC data with a username or a URL. That matches what the
//! importer already is — you import a public Spotify playlist without logging
//! into Spotify — and it honours the standing "integrations are strictly
//! opt-in" rule: connecting an account in Settings only PREFILLS the field, it
//! is never required, and nothing here writes to either service.
//!
//! The one optional credential is the ListenBrainz token, sent when the user
//! has already connected one: it raises the rate limit and reaches their own
//! private playlists. Absent, public reads work exactly the same.
//!
//! # No `qbz-integrations` edge
//!
//! `qbz-integrations` has a ListenBrainz client already, and it is the wrong
//! tool here for two concrete reasons, both verified in its source rather than
//! assumed: `get_playlist_tracks` returns tracks with NO duration field, and it
//! DROPS the playlist title and annotation (those come from the separate list
//! call). A direct `GET /1/playlist/<mbid>` reads title, annotation and
//! per-track JSPF duration in one request, in about thirty lines.
//!
//! It also keeps `cargo test -p qbz-playlist-import` free of that crate's
//! bundled `rusqlite` and `discord-rich-presence`, which is the only real
//! build-cost difference — and it is confined to this crate's own test loop.
//! (The final binary already links `qbz-integrations`, so the edge would have
//! changed app build cost by zero. That argument is not being made.)

pub mod lastfm;
pub mod listenbrainz;

/// A browser-like User-Agent for the scraper-class calls.
///
/// The crate's shared client sends NO User-Agent, which is right for the
/// providers that already work. Last.fm's CDN 403s an empty-UA request, so
/// every call in `lastfm.rs` sets this per-request. ListenBrainz asks for a
/// descriptive one instead and gets [`LB_USER_AGENT`].
pub(crate) const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/126.0.0.0 Safari/537.36";

/// ListenBrainz asks API clients to identify themselves.
pub(crate) const LB_USER_AGENT: &str = "QBZ/2.0 ( https://github.com/vicrodh/qbz )";
