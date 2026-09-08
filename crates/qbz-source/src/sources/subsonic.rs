//! Subsonic / OpenSubsonic — Navidrome, Gonic, Airsonic, Astiga, Ampache.
//!
//! ONE source for all of them, because they speak one API. The server's brand
//! is a display detail, not an identity, and `SourceId::from_word` folds the
//! brand spellings accordingly.
//!
//! Structurally identical to [`crate::JellyfinSource`] — both read the shared
//! cache and both hand out a URL for playback. Two differences, and both come
//! from the protocol rather than from taste:
//!
//! 1. **A Subsonic cover url carries credentials.** So unlike Jellyfin's, its
//!    `cache_key` must be the opaque `coverArt` id, NOT the url: the url embeds
//!    the salt and the size, and keying on it would re-download a cover that is
//!    already on disk the moment either changes.
//! 2. **The token is derived, not stored.** `t = md5(password + salt)`, so the
//!    frontend hands over a `Credentials` rather than a bearer string.

use std::path::Path;
use std::sync::{Arc, RwLock};

use qbz_media_cache::RemoteSource;
use qbz_models::QueueTrack;
use qbz_subsonic::Credentials;

use crate::art::{ArtRef, ArtSize};
use crate::error::SourceError;
use crate::id::{ItemKind, MediaRef, RawRef, SourceId};
use crate::meta::{ItemMeta, SourceBadge};
use crate::playback::PlaybackTicket;
use crate::source::Source;
use crate::sources::remote::{cached_to_queue_track, meta_of_rows, not_found, CacheHandle};

/// The Subsonic connection, implemented by the frontend over its settings store.
pub trait SubsonicCreds: Send + Sync + 'static {
    fn is_enabled(&self) -> bool;
    /// `(base_url, credentials)` when the server is configured.
    ///
    /// The frontend derives the [`Credentials`] from the stored password and
    /// the install's FIXED salt — see `qbz_subsonic::Credentials` for why the
    /// salt does not roll per request.
    fn server(&self) -> Option<(String, Credentials)>;
}

pub struct SubsonicSource {
    creds: RwLock<Option<Arc<dyn SubsonicCreds>>>,
    cache: CacheHandle,
}

impl SubsonicSource {
    pub fn new() -> Self {
        Self {
            creds: RwLock::new(None),
            cache: CacheHandle::new(RemoteSource::Subsonic, SourceId::SUBSONIC),
        }
    }

    pub fn set_creds(&self, c: Option<Arc<dyn SubsonicCreds>>) {
        if let Ok(mut slot) = self.creds.write() {
            *slot = c;
        }
    }

    /// Clone-then-drop; the guard never crosses an `.await`.
    fn server(&self) -> Option<(String, Credentials)> {
        let creds = { self.creds.read().ok()?.clone()? };
        if !creds.is_enabled() {
            return None;
        }
        creds.server().filter(|(base, _)| !base.is_empty())
    }

    pub fn cache(&self) -> &CacheHandle {
        &self.cache
    }

    fn rows(&self, item: &MediaRef) -> Vec<qbz_media_cache::CachedTrack> {
        self.cache
            .with(|c| match item.kind() {
                ItemKind::Album => {
                    qbz_media_cache::album_tracks(c, RemoteSource::Subsonic, item.id())
                        .unwrap_or_default()
                }
                ItemKind::Track => {
                    qbz_media_cache::track_by_item_id(c, RemoteSource::Subsonic, item.id())
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

impl Default for SubsonicSource {
    fn default() -> Self {
        Self::new()
    }
}

/// POSITIVE ownership: the source word (in any of its brand spellings, which
/// `SourceId::from_word` has already folded), or the namespace bit.
pub(crate) fn recognises(raw: &RawRef) -> bool {
    if raw.source == Some(SourceId::SUBSONIC) {
        return true;
    }

    // AN EXPLICIT, DIFFERENT SOURCE WORD IS EVIDENCE AGAINST OWNERSHIP.
    //
    // Without this the namespace-bit arm below OUTVOTES the caller. Measured
    // 2026-08-22 from the owner's log: four George Harrison albums played
    // from a MyQBZ collection failed with
    //   ambiguous id "5099951336752" (Some(Album)):
    //   candidates [SourceId("qobuz"), SourceId("subsonic")]
    // `5099951336752` is an EAN-13 BARCODE. It happens to have bit 42 set and
    // nothing above the payload, which is exactly the Subsonic predicate, so
    // this source claimed an id it can never own while Qobuz claimed it
    // legitimately off the stored source word — two hits, and `claim` refuses
    // to guess.
    //
    // The collision is not a freak: the reserved band is
    // 4_398_046_511_104 ..= 5_497_558_138_879, which is EVERY 13-digit barcode
    // from 4398… to 5497… — all of EMI/Parlophone's 5099… prefix. That is why
    // four albums failed together and why it is deterministic, not flaky.
    if raw.source.is_some() {
        return false;
    }

    // The PREFIXED album key the Local Library grid publishes. Without
    // this arm an album card carrying no source word — which is every card
    // that round-trips through a QML string property — would be claimed by
    // nobody and the album page would open empty. Plex solves the same
    // problem the same way, with its `plex:` keys.
    if raw.id.trim().starts_with("subsonic:") {
        return true;
    }

    // A NAMESPACED NUMERIC IS A TRACK ROWID BY CONSTRUCTION
    // (qbz-media-cache mints them from cache rowids), so this arm must not
    // fire in ALBUM position at all: a Subsonic album id is the server's own
    // opaque string, never one of these. That alone would have spared the
    // barcode above, and it is the narrower statement of the two.
    if raw.kind == Some(ItemKind::Album) {
        return false;
    }
    raw.numeric()
        .map(|n| RemoteSource::of_id(n as i64) == Some(RemoteSource::Subsonic))
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl Source for SubsonicSource {
    fn id(&self) -> SourceId {
        SourceId::SUBSONIC
    }

    fn claim(&self, raw: &RawRef) -> Option<Result<MediaRef, SourceError>> {
        if !recognises(raw) {
            return None;
        }
        let id = raw.id.trim();
        let kind = raw.kind.unwrap_or(ItemKind::Track);
        Some(match kind {
            ItemKind::Album => {
                // The grid publishes `subsonic:<server album id>`; a producer
                // that holds the raw id may pass it bare. STRIP the prefix —
                // the cache and the server both speak the raw id, and carrying
                // the prefixed form inward is how Plex ended up with two album
                // keys only one of its queries understood (survey IC-2).
                let bare = id.strip_prefix("subsonic:").unwrap_or(id).trim();
                if bare.is_empty() {
                    Err(SourceError::BadIdShape {
                        by: SourceId::SUBSONIC,
                        id: id.to_string(),
                        why: "a subsonic album id is the server's own id, never empty",
                    })
                } else {
                    Ok(MediaRef::new(SourceId::SUBSONIC, ItemKind::Album, bare))
                }
            }
            // Same two shapes as Jellyfin, same reason for preferring the hint:
            // the server's id survives a cache rebuild, a rowid does not.
            ItemKind::Track => {
                if let Some(hint) = raw.hint_str().filter(|h| !h.is_empty()) {
                    Ok(MediaRef::new(SourceId::SUBSONIC, ItemKind::Track, hint))
                } else if let Some(n) = raw.numeric() {
                    match self
                        .cache
                        .with(|c| qbz_media_cache::track_by_id(c, n as i64))
                        .and_then(|r| r.ok())
                        .flatten()
                    {
                        Some(t) => Ok(MediaRef::new(
                            SourceId::SUBSONIC,
                            ItemKind::Track,
                            &t.item_id,
                        )),
                        None => Err(SourceError::NotFound {
                            by: SourceId::SUBSONIC,
                            kind: ItemKind::Track,
                            id: raw.id.clone(),
                        }),
                    }
                } else if !id.is_empty() {
                    Ok(MediaRef::new(SourceId::SUBSONIC, ItemKind::Track, id))
                } else {
                    Err(SourceError::BadIdShape {
                        by: SourceId::SUBSONIC,
                        id: id.to_string(),
                        why: "a subsonic track ref needs the server id or a namespaced row id",
                    })
                }
            }
            other => Err(SourceError::Unsupported {
                by: SourceId::SUBSONIC,
                kind: other,
            }),
        })
    }

    async fn tracks(&self, item: &MediaRef) -> Result<Vec<QueueTrack>, SourceError> {
        let rows = self.rows(item);
        if rows.is_empty() {
            return Err(not_found(SourceId::SUBSONIC, item));
        }
        Ok(rows
            .iter()
            .map(|t| cached_to_queue_track(t, SourceId::SUBSONIC))
            .collect())
    }

    async fn meta(&self, item: &MediaRef) -> Result<ItemMeta, SourceError> {
        let rows = self.rows(item);
        if rows.is_empty() {
            return Err(not_found(SourceId::SUBSONIC, item));
        }
        let art = self.artwork(item, ArtSize::Card);
        Ok(meta_of_rows(&rows, item, SourceBadge::Subsonic, art))
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
        match if collection_first {
            find(true).or_else(|| find(false))
        } else {
            find(false).or_else(|| find(true))
        } {
            Some(token) => self.artwork_token(&token, size),
            None => ArtRef::None,
        }
    }

    fn artwork_token(&self, token: &str, size: ArtSize) -> ArtRef {
        // The token is the opaque `coverArt` id — `al-<albumId>_<hash>` for an
        // album, `dc-<albumId>:<disc>_<n>` for a track. Opaque means opaque:
        // store it, never parse it, never build one.
        let token = token.trim();
        if token.is_empty() {
            return ArtRef::None;
        }
        let Some((base, creds)) = self.server() else {
            return ArtRef::Unavailable("subsonic is not connected");
        };
        let px = match size {
            ArtSize::Full => qbz_subsonic::IMAGE_PX_LARGE,
            _ => qbz_subsonic::IMAGE_PX,
        };
        ArtRef::Fetch {
            url: qbz_subsonic::cover_url(&base, &creds, token, px),
            // THE opaque id, NOT the url. The url carries the salt and the size
            // and both move; the token does not. Keying on the url would
            // re-download every cover the moment either changed — and would put
            // a credential in a cache key, which is its own problem.
            cache_key: format!("subsonic:{token}"),
        }
    }

    async fn playback(
        &self,
        item: &MediaRef,
        track: &QueueTrack,
    ) -> Result<PlaybackTicket, SourceError> {
        if item.kind() != ItemKind::Track {
            return Err(SourceError::Unsupported {
                by: SourceId::SUBSONIC,
                kind: item.kind(),
            });
        }
        let (base, creds) = self.server().ok_or(SourceError::NotConfigured {
            by: SourceId::SUBSONIC,
            why: "no Subsonic credentials configured",
        })?;
        // `format=raw` is the bit-perfect contract, verified md5-identical to
        // the file on disk. The bare `stream.view` happened to return raw bytes
        // on the bench, but only because that server had no transcoding policy
        // for this client — that is CONFIGURATION, not a guarantee.
        Ok(PlaybackTicket::Stream {
            url: qbz_subsonic::stream_url(&base, &creds, item.id()),
            play_id: track.id,
            duration_secs: track.duration_secs,
            start_secs: 0,
            log_tag: "SUBSONIC",
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Creds;
    impl SubsonicCreds for Creds {
        fn is_enabled(&self) -> bool {
            true
        }
        fn server(&self) -> Option<(String, Credentials)> {
            Some((
                "http://nd:4533".into(),
                Credentials::new("admin", "pw", "fixedsalt"),
            ))
        }
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

    fn connected() -> SubsonicSource {
        let s = SubsonicSource::new();
        s.set_creds(Some(Arc::new(Creds)));
        s
    }

    /// Every brand spelling folds to ONE source, or a row stamped `navidrome`
    /// would be `Unclaimed` and refuse to play.
    #[test]
    fn every_brand_spelling_resolves_to_this_source() {
        for word in ["subsonic", "navidrome", "gonic", "airsonic", "astiga"] {
            assert_eq!(
                SourceId::from_word(word),
                Some(SourceId::SUBSONIC),
                "{word} did not fold"
            );
            assert!(recognises(&RawRef {
                source: SourceId::from_word(word),
                kind: Some(ItemKind::Track),
                id: "abc".into(),
                ..Default::default()
            }));
        }
    }

    #[test]
    fn it_never_claims_another_sources_id() {
        for foreign in [
            (1i64 << 40) | 44_440,               // Plex
            RemoteSource::Jellyfin.namespace(7), // the neighbour
            (1i64 << 48) + 10,                   // ephemeral
            2954,                                // a local rowid
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
            id: "subsonic:al-abc".into(),
            ..Default::default()
        };
        assert!(
            recognises(&raw),
            "an unlabelled prefixed key was not recognised"
        );
        let m = s.claim(&raw).unwrap().unwrap();
        assert_eq!(m.id(), "al-abc", "the prefix was carried inward");
        // A bare id with the source word still works.
        let bare = s
            .claim(&RawRef {
                source: Some(SourceId::SUBSONIC),
                kind: Some(ItemKind::Album),
                id: "al-abc".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        assert_eq!(bare.id(), "al-abc");
        // A prefix with nothing after it is a named error, not an empty lookup.
        let err = s
            .claim(&RawRef {
                kind: Some(ItemKind::Album),
                id: "subsonic:".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            err,
            SourceError::BadIdShape {
                by: SourceId::SUBSONIC,
                ..
            }
        ));
    }

    /// ...and it must not answer for the NEIGHBOUR's prefix.
    #[test]
    fn it_does_not_recognise_the_other_prefix() {
        assert!(!recognises(&RawRef {
            kind: Some(ItemKind::Album),
            id: "jellyfin:alb-1".into(),
            ..Default::default()
        }));
        assert!(!recognises(&RawRef {
            kind: Some(ItemKind::Album),
            id: "plex:5677211365378243606".into(),
            ..Default::default()
        }));
    }

    #[test]
    fn a_track_normalises_to_the_server_id_from_the_hint() {
        let s = connected();
        let m = s
            .claim(&RawRef {
                source: Some(SourceId::SUBSONIC),
                kind: Some(ItemKind::Track),
                id: RemoteSource::Subsonic.namespace(3).to_string(),
                hint: Some("hpclx3k1zyDli7O1nbsII6".into()),
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        assert_eq!(m.id(), "hpclx3k1zyDli7O1nbsII6");
    }

    /// THE difference from Jellyfin: the url carries credentials, so the cache
    /// key must be the opaque id instead. Keying on the url would put a
    /// credential in a cache key AND re-download on every salt or size change.
    #[test]
    fn the_cover_key_is_the_opaque_id_not_the_credentialed_url() {
        let s = connected();
        match s.artwork_token("al-abc_59fec8ff", ArtSize::Card) {
            ArtRef::Fetch { url, cache_key } => {
                assert_ne!(url, cache_key, "the url must not be the cache key here");
                assert_eq!(cache_key, "subsonic:al-abc_59fec8ff");
                assert!(
                    !cache_key.contains("t="),
                    "a credential reached the cache key"
                );
                assert!(url.contains("getCoverArt.view"));
                assert!(
                    url.contains("t="),
                    "an unauthenticated cover request is refused"
                );
                assert!(url.contains("size=256"));
            }
            other => panic!("expected Fetch, got {other:?}"),
        }
    }

    /// A track cover id contains `:` and must be encoded, not pasted.
    #[test]
    fn an_opaque_track_cover_id_survives_the_url() {
        match connected().artwork_token("dc-abc:1_0", ArtSize::Card) {
            ArtRef::Fetch { url, cache_key } => {
                assert_eq!(cache_key, "subsonic:dc-abc:1_0");
                assert!(url.contains("id=dc-abc%3A1_0"), "{url}");
            }
            other => panic!("expected Fetch, got {other:?}"),
        }
    }

    #[test]
    fn a_cover_without_a_server_is_unavailable_not_none() {
        assert!(matches!(
            SubsonicSource::new().artwork_token("al-x", ArtSize::Card),
            ArtRef::Unavailable(_)
        ));
        assert!(matches!(
            connected().artwork_token("", ArtSize::Card),
            ArtRef::None
        ));
    }

    #[tokio::test]
    async fn playback_demands_raw_and_never_caps_the_bitrate() {
        let s = connected();
        let item = MediaRef::new(SourceId::SUBSONIC, ItemKind::Track, "trk-1");
        let qt = qt(9, 301);
        match s.playback(&item, &qt).await.unwrap() {
            PlaybackTicket::Stream {
                url,
                play_id,
                duration_secs,
                log_tag,
                ..
            } => {
                assert!(url.contains("stream.view"));
                assert!(url.contains("&format=raw"), "raw is the contract: {url}");
                assert!(!url.contains("maxBitRate"), "a bitrate cap IS a transcode");
                assert!(url.contains("&id=trk-1"));
                assert!(!url.contains("pw"), "the password reached the url");
                assert_eq!(play_id, 9);
                assert_eq!(duration_secs, 301);
                assert_eq!(log_tag, "SUBSONIC");
            }
            other => panic!("expected Stream, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn playback_without_credentials_is_not_configured() {
        let s = SubsonicSource::new();
        let item = MediaRef::new(SourceId::SUBSONIC, ItemKind::Track, "x");
        let err = s.playback(&item, &qt(0, 0)).await.unwrap_err();
        assert!(matches!(
            err,
            SourceError::NotConfigured {
                by: SourceId::SUBSONIC,
                ..
            }
        ));
    }

    /// THE BARCODE THAT BROKE FOUR ALBUMS AT ONCE (2026-08-22).
    ///
    /// `5099951336752` is George Harrison's "Somewhere In England" on
    /// EMI/Parlophone — an EAN-13 barcode Qobuz uses as an album id. It has
    /// bit 42 set and nothing above the payload, i.e. it satisfies the
    /// Subsonic namespace predicate exactly, so this source used to claim it
    /// alongside Qobuz and `claim` returned `Ambiguous`.
    ///
    /// The reserved band is 4_398_046_511_104 ..= 5_497_558_138_879 — EVERY
    /// 13-digit barcode from 4398… to 5497…, which is all of EMI/Parlophone's
    /// 5099… prefix. The pre-existing disjointness tests all used
    /// "0060254702523", whose LEADING ZERO puts it below every floor, so this
    /// entire class was untested.
    #[test]
    fn an_ean13_barcode_in_the_namespace_band_is_not_ours() {
        let id = "5099951336752";
        assert_eq!(
            id.parse::<i64>().unwrap() & (1 << 42),
            1 << 42,
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
