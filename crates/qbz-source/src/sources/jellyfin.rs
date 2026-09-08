//! Jellyfin — the acceptance test of design 02 §10, cashed in.
//!
//! §10 said a fourth source should be "one impl plus one registration line".
//! This is that impl. What it does NOT contain is the interesting part: no arm
//! in any view, no arm in the playback router, no arm in the artwork pipeline,
//! no entry in a url taxonomy. Those all became structural during stages 3 and
//! 4, and this file is what proves it.
//!
//! It reads the shared cache (`qbz-media-cache`), never the network — except
//! for the two places where a URL is HANDED OUT rather than followed:
//! [`Source::playback`] and the fetch half of artwork.

use std::path::Path;
use std::sync::{Arc, RwLock};

use qbz_media_cache::RemoteSource;
use qbz_models::QueueTrack;

use crate::art::{ArtRef, ArtSize};
use crate::error::SourceError;
use crate::id::{ItemKind, MediaRef, RawRef, SourceId};
use crate::meta::{ItemMeta, SourceBadge};
use crate::playback::PlaybackTicket;
use crate::source::Source;
use crate::sources::remote::{cached_to_queue_track, meta_of_rows, not_found, CacheHandle};

/// The Jellyfin connection, implemented by the frontend over its own settings
/// store.
///
/// Injected for the same reason [`crate::PlexCreds`] is: opening a second copy
/// of the settings database here would be a second authority, and it would drag
/// `qbz-app` into this crate's graph, which design §8 forbids.
pub trait JellyfinCreds: Send + Sync + 'static {
    /// The master toggle.
    fn is_enabled(&self) -> bool;
    /// `(base_url, access_token)`, both non-empty, or `None`.
    fn server(&self) -> Option<(String, String)>;
}

/// The Jellyfin source.
pub struct JellyfinSource {
    creds: RwLock<Option<Arc<dyn JellyfinCreds>>>,
    cache: CacheHandle,
}

impl JellyfinSource {
    pub fn new() -> Self {
        Self {
            creds: RwLock::new(None),
            cache: CacheHandle::new(RemoteSource::Jellyfin, SourceId::JELLYFIN),
        }
    }

    /// Publish the frontend's credentials handle. `None` clears it.
    pub fn set_creds(&self, c: Option<Arc<dyn JellyfinCreds>>) {
        if let Ok(mut slot) = self.creds.write() {
            *slot = c;
        }
    }

    /// `(base_url, token)` when Jellyfin is usable right now.
    ///
    /// Clone-then-drop: the guard is never held across an `.await`. It is a
    /// `std::sync::RwLock`, whose guard is `!Send`, so holding it across one
    /// inside an `#[async_trait]` method is a COMPILE ERROR rather than a
    /// code-review rule.
    fn server(&self) -> Option<(String, String)> {
        let creds = { self.creds.read().ok()?.clone()? };
        if !creds.is_enabled() {
            return None;
        }
        creds
            .server()
            .filter(|(base, token)| !base.is_empty() && !token.is_empty())
    }

    /// The cache handle, for the frontend's sync to write through.
    pub fn cache(&self) -> &CacheHandle {
        &self.cache
    }

    /// The rows behind a claimed reference.
    fn rows(&self, item: &MediaRef) -> Vec<qbz_media_cache::CachedTrack> {
        self.cache
            .with(|c| match item.kind() {
                ItemKind::Album => {
                    qbz_media_cache::album_tracks(c, RemoteSource::Jellyfin, item.id())
                        .unwrap_or_default()
                }
                ItemKind::Track => {
                    // A normalised track ref carries the SERVER's item id — see
                    // `claim`, which is where the two identities are reconciled.
                    qbz_media_cache::track_by_item_id(c, RemoteSource::Jellyfin, item.id())
                        .ok()
                        .flatten()
                        .into_iter()
                        .collect()
                }
                _ => Vec::new(),
            })
            .unwrap_or_default()
    }
}

impl Default for JellyfinSource {
    fn default() -> Self {
        Self::new()
    }
}

/// POSITIVE ownership: the source word, or the namespace bit on a numeric id.
///
/// Never by elimination, and never by the SHAPE of an opaque id — a Jellyfin
/// item id is a 32-character hex GUID and a Subsonic one is base62, but neither
/// is guaranteed, and guessing a source from a string's alphabet is exactly the
/// class of inference the seam exists to delete.
pub(crate) fn recognises(raw: &RawRef) -> bool {
    if raw.source == Some(SourceId::JELLYFIN) {
        return true;
    }
    // AN EXPLICIT, DIFFERENT SOURCE WORD IS EVIDENCE AGAINST OWNERSHIP — see
    // the long note in sources/subsonic.rs. A namespace bit must never
    // outvote a caller who already said which source this is.
    if raw.source.is_some() {
        return false;
    }

    // The PREFIXED album key the Local Library grid publishes. Without
    // this arm an album card carrying no source word — which is every card
    // that round-trips through a QML string property — would be claimed by
    // nobody and the album page would open empty.
    if raw.id.trim().starts_with("jellyfin:") {
        return true;
    }

    // A namespaced numeric is a TRACK rowid by construction; a Jellyfin album
    // id is a 32-character hex GUID, never one of these.
    if raw.kind == Some(ItemKind::Album) {
        return false;
    }
    raw.numeric()
        .map(|n| RemoteSource::of_id(n as i64) == Some(RemoteSource::Jellyfin))
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl Source for JellyfinSource {
    fn id(&self) -> SourceId {
        SourceId::JELLYFIN
    }

    fn claim(&self, raw: &RawRef) -> Option<Result<MediaRef, SourceError>> {
        if !recognises(raw) {
            return None;
        }
        let id = raw.id.trim();
        let kind = raw.kind.unwrap_or(ItemKind::Track);
        Some(match kind {
            // An album id is the server's own, opaque and non-numeric.
            ItemKind::Album => {
                // The grid publishes `jellyfin:<server album id>`; a producer
                // that holds the raw id may pass it bare. STRIP the prefix —
                // the cache and the server both speak the raw id, and carrying
                // the prefixed form inward is how Plex ended up with two album
                // keys only one of its queries understood (survey IC-2).
                let bare = id.strip_prefix("jellyfin:").unwrap_or(id).trim();
                if bare.is_empty() {
                    Err(SourceError::BadIdShape {
                        by: SourceId::JELLYFIN,
                        id: id.to_string(),
                        why: "a jellyfin album id is the server's own id, never empty",
                    })
                } else {
                    Ok(MediaRef::new(SourceId::JELLYFIN, ItemKind::Album, bare))
                }
            }
            // A TRACK arrives in one of two shapes and both must normalise to
            // the SERVER's item id, because that is the only thing the server
            // and the cache both understand:
            //
            //  1. the namespaced numeric id the Local Library grid published,
            //     plus the server id in the hint (`cached_to_queue_track`);
            //  2. the server id outright, from a producer that has it.
            //
            // Preferring the HINT is what makes a queue row survive a cache
            // rebuild: rowids are reassigned, the server's id is not.
            ItemKind::Track => {
                if let Some(hint) = raw.hint_str().filter(|h| !h.is_empty()) {
                    Ok(MediaRef::new(SourceId::JELLYFIN, ItemKind::Track, hint))
                } else if let Some(n) = raw.numeric() {
                    match self
                        .cache
                        .with(|c| qbz_media_cache::track_by_id(c, n as i64))
                        .and_then(|r| r.ok())
                        .flatten()
                    {
                        Some(t) => Ok(MediaRef::new(
                            SourceId::JELLYFIN,
                            ItemKind::Track,
                            &t.item_id,
                        )),
                        // The row is gone from the cache: a named error at the
                        // moment of the mistake, not a 404 from the server two
                        // layers down.
                        None => Err(SourceError::NotFound {
                            by: SourceId::JELLYFIN,
                            kind: ItemKind::Track,
                            id: raw.id.clone(),
                        }),
                    }
                } else if !id.is_empty() {
                    Ok(MediaRef::new(SourceId::JELLYFIN, ItemKind::Track, id))
                } else {
                    Err(SourceError::BadIdShape {
                        by: SourceId::JELLYFIN,
                        id: id.to_string(),
                        why: "a jellyfin track ref needs the server item id or a namespaced row id",
                    })
                }
            }
            other => Err(SourceError::Unsupported {
                by: SourceId::JELLYFIN,
                kind: other,
            }),
        })
    }

    async fn tracks(&self, item: &MediaRef) -> Result<Vec<QueueTrack>, SourceError> {
        // Cache reads only — no `.await` in this body.
        let rows = self.rows(item);
        if rows.is_empty() {
            return Err(not_found(SourceId::JELLYFIN, item));
        }
        Ok(rows
            .iter()
            .map(|t| cached_to_queue_track(t, SourceId::JELLYFIN))
            .collect())
    }

    async fn meta(&self, item: &MediaRef) -> Result<ItemMeta, SourceError> {
        let rows = self.rows(item);
        if rows.is_empty() {
            return Err(not_found(SourceId::JELLYFIN, item));
        }
        let art = self.artwork(item, ArtSize::Card);
        Ok(meta_of_rows(&rows, item, SourceBadge::Jellyfin, art))
    }

    fn artwork(&self, item: &MediaRef, size: ArtSize) -> ArtRef {
        let rows = self.rows(item);
        let collection_first = item.kind == ItemKind::Album;
        let find = |collection: bool| {
            rows.iter().find_map(|track| {
                let token = if collection {
                    &track.collection_artwork_token
                } else {
                    &track.artwork_token
                };
                token.clone().filter(|value| !value.is_empty())
            })
        };
        let Some(token) = (if collection_first {
            find(true).or_else(|| find(false))
        } else {
            find(false).or_else(|| find(true))
        }) else {
            return ArtRef::None;
        };
        self.artwork_token(&token, size)
    }

    fn artwork_token(&self, token: &str, size: ArtSize) -> ArtRef {
        // The token here is a bare image TAG with no item to hang it off, which
        // Jellyfin cannot address. The rows path (`artwork`) is the one that
        // has both; this arm exists so a producer that only kept the tag gets
        // an honest `None` rather than a broken URL.
        //
        // A producer that stamped `"<itemId>/<tag>"` — where item may be the
        // audio row for disc art or its MusicAlbum for collection art — is
        // addressable. An empty tag is valid and means "do not cache-bust".
        let token = token.trim();
        if token.is_empty() {
            return ArtRef::None;
        }
        match token.split_once('/') {
            Some((item_id, tag)) => {
                self.image_ref(item_id, (!tag.trim().is_empty()).then_some(tag), size)
            }
            None => ArtRef::None,
        }
    }

    async fn playback(
        &self,
        item: &MediaRef,
        track: &QueueTrack,
    ) -> Result<PlaybackTicket, SourceError> {
        if item.kind() != ItemKind::Track {
            return Err(SourceError::Unsupported {
                by: SourceId::JELLYFIN,
                kind: item.kind(),
            });
        }
        let (base, token) = self.server().ok_or(SourceError::NotConfigured {
            by: SourceId::JELLYFIN,
            why: "no Jellyfin credentials configured",
        })?;
        // `item.id()` IS the server's item id by construction (see `claim`), so
        // nothing is re-derived here and no caller can smuggle a row id in.
        //
        // `stream_url` asks for `?static=true`: the ORIGINAL bytes, verified
        // md5-identical to the file on the server's disk, with a
        // `Content-Length` and `Accept-Ranges` so the feeder can Range-stream
        // it. No transcode is requested and none may be.
        Ok(PlaybackTicket::Stream {
            url: qbz_jellyfin::stream_url(&base, &token, item.id()),
            play_id: track.id,
            duration_secs: track.duration_secs,
            start_secs: 0,
            log_tag: "JELLYFIN",
            request_headers: Vec::new(),
        })
    }

    fn bind_user(&self, _uid: u64, dir: &Path) {
        self.cache.bind(dir);
    }

    fn teardown(&self) {
        self.set_creds(None);
        self.cache.teardown();
    }
}

impl JellyfinSource {
    /// The [`ArtRef`] for an album image.
    ///
    /// **No credentials in the url** — measured: `/Items/{id}/Images/Primary`
    /// answers 200 unauthenticated. That is why `cache_key` can be the url
    /// itself: unlike a Plex thumb it does not rotate, so keying on it can
    /// never miss a cover already on disk.
    fn image_ref(&self, album_id: &str, tag: Option<&str>, size: ArtSize) -> ArtRef {
        if album_id.is_empty() {
            return ArtRef::None;
        }
        let Some((base, _)) = self.server() else {
            // The art EXISTS; the server is not connected. Distinct from `None`
            // so the miss is logged for what it is rather than looking like a
            // dead download.
            return ArtRef::Unavailable("jellyfin is not connected");
        };
        let px = match size {
            ArtSize::Full => qbz_jellyfin::IMAGE_PX_LARGE,
            _ => qbz_jellyfin::IMAGE_PX,
        };
        let url = qbz_jellyfin::image_url(&base, album_id, tag, px);
        ArtRef::Fetch {
            cache_key: url.clone(),
            url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Creds;
    impl JellyfinCreds for Creds {
        fn is_enabled(&self) -> bool {
            true
        }
        fn server(&self) -> Option<(String, String)> {
            Some(("http://jf:8096".into(), "tok".into()))
        }
    }

    fn connected() -> JellyfinSource {
        let s = JellyfinSource::new();
        s.set_creds(Some(Arc::new(Creds)));
        s
    }

    /// A minimal `QueueTrack`. `qbz_models::QueueTrack` has no `Default` — its
    /// `streamable` field defaults to TRUE through serde, and a derived
    /// `Default` would silently make it false, so the model refuses to provide
    /// one. Building it out here keeps that guarantee where it belongs.
    fn qt(id: u64, duration_secs: u64) -> QueueTrack {
        QueueTrack {
            id,
            title: String::new(),
            version: None,
            artist: String::new(),
            album: String::new(),
            album_version: None,
            duration_secs,
            artwork_url: None,
            hires: false,
            bit_depth: None,
            sample_rate: None,
            is_local: true,
            album_id: None,
            artist_id: None,
            streamable: true,
            source: None,
            parental_warning: false,
            source_item_id_hint: None,
            context_kind: None,
            context_id: None,
            isrc: None,
            recording_mbid: None,
        }
    }

    fn track_ref(id: &str, hint: Option<&str>) -> RawRef {
        RawRef {
            source: Some(SourceId::JELLYFIN),
            badge: SourceBadge::Jellyfin,
            kind: Some(ItemKind::Track),
            id: id.into(),
            is_local: Some(true),
            hint: hint.map(String::from),
        }
    }

    // ── claim ──────────────────────────────────────────────────────────────

    /// The HINT wins over the numeric id, and that is the point: rowids are
    /// reassigned by a cache rebuild, the server's item id is not. A queue row
    /// stamped before the rebuild still resolves.
    #[test]
    fn a_track_normalises_to_the_server_id_from_the_hint() {
        let s = connected();
        let m = s
            .claim(&track_ref("2199023255559", Some("srv-abc")))
            .unwrap()
            .unwrap();
        assert_eq!(m.source(), SourceId::JELLYFIN);
        assert_eq!(m.id(), "srv-abc", "the row id was used instead of the hint");
    }

    /// Recognition by the namespace bit alone, with no source word — the shape
    /// a mixtape item arrives in.
    #[test]
    fn the_namespace_bit_is_enough_to_recognise() {
        let namespaced = RemoteSource::Jellyfin.namespace(7).to_string();
        assert!(recognises(&RawRef {
            kind: Some(ItemKind::Track),
            id: namespaced,
            ..Default::default()
        }));
    }

    /// And it must claim NOTHING that belongs to a neighbour. Plex's floor is
    /// bit 40, Subsonic's is 42, the ephemeral store's is 48 — a predicate that
    /// answered "mine" for any of them would route the row to the wrong server.
    #[test]
    fn it_never_claims_another_sources_id() {
        for foreign in [
            (1i64 << 40) | 44_440, // Plex
            RemoteSource::Subsonic.namespace(7),
            (1i64 << 48) + 10, // ephemeral
            2954,              // a local_tracks rowid
        ] {
            assert!(
                !recognises(&RawRef {
                    kind: Some(ItemKind::Track),
                    id: foreign.to_string(),
                    ..Default::default()
                }),
                "claimed {foreign}"
            );
        }
        // A Qobuz album id, and a local group key.
        assert!(!recognises(&RawRef::new(
            "qobuz",
            ItemKind::Album,
            "0060254702523"
        )));
        assert!(!recognises(&RawRef {
            kind: Some(ItemKind::Album),
            id: "HIT ME HARD AND SOFT|Billie Eilish".into(),
            ..Default::default()
        }));
    }

    /// The grid's album key is PREFIXED, and it must round-trip: recognised
    /// with no source word at all, and stripped back to the raw server id the
    /// cache and the server both speak.
    #[test]
    fn a_prefixed_album_key_is_recognised_and_stripped() {
        let s = connected();
        // No source word — the shape a QML string property hands back.
        let raw = RawRef {
            kind: Some(ItemKind::Album),
            id: "jellyfin:alb-1".into(),
            ..Default::default()
        };
        assert!(
            recognises(&raw),
            "an unlabelled prefixed key was not recognised"
        );
        let m = s.claim(&raw).unwrap().unwrap();
        assert_eq!(m.id(), "alb-1", "the prefix was carried inward");
        // A bare id with the source word still works.
        let bare = s
            .claim(&RawRef {
                source: Some(SourceId::JELLYFIN),
                kind: Some(ItemKind::Album),
                id: "alb-1".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        assert_eq!(bare.id(), "alb-1");
        // A prefix with nothing after it is a named error, not an empty lookup.
        let err = s
            .claim(&RawRef {
                kind: Some(ItemKind::Album),
                id: "jellyfin:".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            err,
            SourceError::BadIdShape {
                by: SourceId::JELLYFIN,
                ..
            }
        ));
    }

    /// ...and it must not answer for the NEIGHBOUR's prefix.
    #[test]
    fn it_does_not_recognise_the_other_prefix() {
        assert!(!recognises(&RawRef {
            kind: Some(ItemKind::Album),
            id: "subsonic:al-abc".into(),
            ..Default::default()
        }));
        assert!(!recognises(&RawRef {
            kind: Some(ItemKind::Album),
            id: "plex:5677211365378243606".into(),
            ..Default::default()
        }));
    }

    #[test]
    fn an_empty_album_id_is_a_named_error_not_a_lookup() {
        let s = connected();
        let err = s
            .claim(&RawRef {
                source: Some(SourceId::JELLYFIN),
                kind: Some(ItemKind::Album),
                id: "  ".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            err,
            SourceError::BadIdShape {
                by: SourceId::JELLYFIN,
                ..
            }
        ));
    }

    #[test]
    fn an_artist_reference_is_unsupported_rather_than_guessed() {
        let s = connected();
        let err = s
            .claim(&RawRef {
                source: Some(SourceId::JELLYFIN),
                kind: Some(ItemKind::Artist),
                id: "abc".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            err,
            SourceError::Unsupported {
                by: SourceId::JELLYFIN,
                ..
            }
        ));
    }

    // ── artwork ────────────────────────────────────────────────────────────

    /// The cover url carries NO token, which is why its cache key can be the
    /// url itself.
    #[test]
    fn a_cover_url_is_tokenless_and_self_keying() {
        let s = connected();
        match s.artwork_token("alb-1/deadbeef", ArtSize::Card) {
            ArtRef::Fetch { url, cache_key } => {
                assert_eq!(url, cache_key, "a stable url should key on itself");
                assert!(
                    !url.contains("tok"),
                    "the access token leaked into a cover url"
                );
                assert!(url.contains("/Items/alb-1/Images/Primary"));
                assert!(url.contains("tag=deadbeef"));
                assert!(url.contains("maxWidth=256"));
            }
            other => panic!("expected Fetch, got {other:?}"),
        }
        // The Full tier asks for the larger transcode.
        match s.artwork_token("alb-1/deadbeef", ArtSize::Full) {
            ArtRef::Fetch { url, .. } => assert!(url.contains("maxWidth=1024")),
            other => panic!("expected Fetch, got {other:?}"),
        }
        match s.artwork_token("alb-1/", ArtSize::Card) {
            ArtRef::Fetch { url, .. } => {
                assert!(url.contains("/Items/alb-1/Images/Primary"));
                assert!(!url.contains("tag="));
            }
            other => panic!("expected tagless Fetch, got {other:?}"),
        }
    }

    /// Disconnected is UNAVAILABLE, never None: the art exists, the server does
    /// not answer yet, and collapsing the two loses the retry.
    #[test]
    fn a_cover_without_a_server_is_unavailable_not_none() {
        let s = JellyfinSource::new();
        assert!(matches!(
            s.artwork_token("alb-1/tag", ArtSize::Card),
            ArtRef::Unavailable(_)
        ));
        // ...but a token with nothing to hang it off is genuinely None.
        assert!(matches!(
            connected().artwork_token("bare-tag", ArtSize::Card),
            ArtRef::None
        ));
        assert!(matches!(
            connected().artwork_token("", ArtSize::Card),
            ArtRef::None
        ));
    }

    // ── playback ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn playback_asks_for_the_original_bytes() {
        let s = connected();
        let item = MediaRef::new(SourceId::JELLYFIN, ItemKind::Track, "srv-abc");
        let qt = qt(42, 188);
        match s.playback(&item, &qt).await.unwrap() {
            PlaybackTicket::Stream {
                url,
                play_id,
                duration_secs,
                log_tag,
                ..
            } => {
                assert!(url.contains("/Audio/srv-abc/stream?static=true"));
                assert!(
                    !url.contains("audioCodec"),
                    "a codec parameter IS a transcode"
                );
                assert_eq!(play_id, 42);
                assert_eq!(duration_secs, 188);
                assert_eq!(log_tag, "JELLYFIN");
            }
            other => panic!("expected Stream, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn playback_without_credentials_is_not_configured() {
        let s = JellyfinSource::new();
        let item = MediaRef::new(SourceId::JELLYFIN, ItemKind::Track, "x");
        let err = s.playback(&item, &qt(0, 0)).await.unwrap_err();
        assert!(matches!(
            err,
            SourceError::NotConfigured {
                by: SourceId::JELLYFIN,
                ..
            }
        ));
    }

    /// An ALBUM cannot be played directly — the caller expands it to tracks
    /// first. Saying so beats streaming something arbitrary.
    #[tokio::test]
    async fn an_album_is_not_playable_on_its_own() {
        let s = connected();
        let item = MediaRef::new(SourceId::JELLYFIN, ItemKind::Album, "alb");
        let err = s.playback(&item, &qt(0, 0)).await.unwrap_err();
        assert!(matches!(
            err,
            SourceError::Unsupported {
                by: SourceId::JELLYFIN,
                ..
            }
        ));
    }

    /// A disabled toggle is the same as no credentials — the source must not
    /// serve a stale connection after the user switches it off.
    #[test]
    fn a_disabled_source_reports_no_server() {
        struct Off;
        impl JellyfinCreds for Off {
            fn is_enabled(&self) -> bool {
                false
            }
            fn server(&self) -> Option<(String, String)> {
                Some(("http://jf:8096".into(), "tok".into()))
            }
        }
        let s = JellyfinSource::new();
        s.set_creds(Some(Arc::new(Off)));
        assert!(s.server().is_none());
    }

    /// THE BARCODE THAT BROKE FOUR ALBUMS AT ONCE (2026-08-22).
    ///
    /// `2500000000000` is a 13-digit barcode in Jellyfin's reserved band. It has
    /// bit 41 set and nothing above the payload, i.e. it satisfies the
    /// Subsonic namespace predicate exactly, so this source used to claim it
    /// alongside Qobuz and `claim` returned `Ambiguous`.
    ///
    /// The reserved band is 2_199_023_255_552 ..= 3_298_534_883_327 — EVERY
    /// 13-digit barcode from 2199… to 3298…. The pre-existing disjointness tests all used
    /// "0060254702523", whose LEADING ZERO puts it below every floor, so this
    /// entire class was untested.
    #[test]
    fn an_ean13_barcode_in_the_namespace_band_is_not_ours() {
        let id = "2500000000000";
        assert_eq!(
            id.parse::<i64>().unwrap() & (1 << 41),
            1 << 41,
            "the barcode really does sit in the reserved band — if this ever \
             fails the test has stopped covering the case it was written for"
        );
        // Named by the caller: the source word must win outright.
        assert!(!recognises(&RawRef::new("qobuz", ItemKind::Album, id)));
        // Even unnamed, an ALBUM id is never a namespaced track rowid.
        let mut unnamed = RawRef::new("qobuz", ItemKind::Album, id);
        unnamed.source = None;
        assert!(!recognises(&unnamed));
    }
}
