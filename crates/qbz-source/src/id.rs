//! Identity: who owns an id, and what the canonical form of that id is.
//!
//! This module is the whole of the seam's identity half. Two types matter:
//!
//!  - [`RawRef`] — what a CALLER has: strings of unknown provenance. It is the
//!    only reference a frontend may construct, and it is deliberately dumb.
//!  - [`MediaRef`] — a NORMALISED, source-owned reference. It has **no public
//!    constructor**: the only way to obtain one outside this crate is
//!    [`crate::SourceRegistry::claim`], i.e. from the source that owns the id
//!    shape. That is bug 2's structural fix on the identity side — a caller
//!    cannot mint `MediaRef { PLEX, Track, "plex:deadbeef" }` because it cannot
//!    mint a `MediaRef` at all.

use qbz_models::mixtape::{AlbumSource, ItemType, MixtapeCollectionItem};
use qbz_models::QueueTrack;

use crate::meta::SourceBadge;

/// A source's stable wire name.
///
/// These strings ARE the values already carried by `QueueTrack.source` /
/// `LocalTrack.source`, so nothing on disk or in the queue has to be migrated.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SourceId(&'static str);

impl SourceId {
    /// The Qobuz streaming catalog.
    pub const QOBUZ: SourceId = SourceId("qobuz");
    /// A Plex Media Server (through the local Plex cache).
    pub const PLEX: SourceId = SourceId("plex");
    /// A Jellyfin server (through `qbz-media-cache`).
    pub const JELLYFIN: SourceId = SourceId("jellyfin");
    /// A Subsonic-compatible server — Navidrome, Gonic, Airsonic, Astiga,
    /// Ampache. ONE source, because they speak one API; the server's own
    /// flavour is a display detail, not an identity.
    pub const SUBSONIC: SourceId = SourceId("subsonic");
    /// Files on this machine: the indexed library, Qobuz downloads/purchases
    /// and the session-scoped ephemeral folder.
    pub const LOCAL: SourceId = SourceId("local");

    /// The wire name, e.g. `"qobuz"`.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The ONE mapping from a source WORD to a source.
    ///
    /// Six vocabularies exist on disk and in the queue (survey §1); this table
    /// is the single place that reconciles them:
    ///
    /// | word | where | → |
    /// |---|---|---|
    /// | `"qobuz"` | `QueueTrack.source`, `AlbumSource::Qobuz` | `QOBUZ` |
    /// | `"qobuz_connect_remote"` | legacy persisted QConnect queue rows | `QOBUZ` |
    /// | `""` / absent | `local_tracks.source`, `HomeCard` (no field at all) | `None` |
    /// | `"local"`, `"user"` | `QueueTrack.source`, `LocalTrack.source` | `LOCAL` |
    /// | `"qobuz_download"`, `"qobuz_purchase"`, `"offline"` | `LocalTrack.source`, badges | `LOCAL` (an offline copy is a FILE; the distinction is a [`SourceBadge`]) |
    /// | `"ephemeral"` | `lyrics_qt.rs:816`, `foryou_qt.rs:775` | `LOCAL` (session-scoped arm inside `LocalSource`) |
    /// | `"plex"` | `QueueTrack.source`, `local_favorites.db` CHECK | `PLEX` |
    /// | `"jellyfin"` | `QueueTrack.source`, `remote_cache_tracks.source` | `JELLYFIN` |
    /// | `"subsonic"` | `QueueTrack.source`, `remote_cache_tracks.source` | `SUBSONIC` |
    /// | `"navidrome"`, `"gonic"`, `"airsonic"` | a user's or a producer's spelling of the server they run | `SUBSONIC` |
    ///
    /// Unknown words return `None`. They are never guessed into `QOBUZ`, which
    /// is `myqbz_add_qt::source_from_str`'s bug (survey IC-6).
    pub fn from_word(w: &str) -> Option<SourceId> {
        match w.trim().to_ascii_lowercase().as_str() {
            "qobuz" | "qobuz_connect_remote" => Some(SourceId::QOBUZ),
            "plex" => Some(SourceId::PLEX),
            "jellyfin" => Some(SourceId::JELLYFIN),
            // The Subsonic FLAVOURS fold into one source. They speak one API
            // and QBZ reads them through one impl, so a row stamped with the
            // server's brand must still resolve — a `"navidrome"` word that
            // returned None would be `Unclaimed` and the row would refuse to
            // play, which is a worse answer than the honest fold.
            "subsonic" | "navidrome" | "gonic" | "airsonic" | "astiga" => Some(SourceId::SUBSONIC),
            "local" | "user" | "qobuz_download" | "qobuz_purchase" | "offline" | "ephemeral" => {
                Some(SourceId::LOCAL)
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// What KIND of thing an id names.
///
/// `Artist` and `Folder` exist because two current callers route by them
/// (`library_qt.rs:1037` favourites kind `"artist"`; `local_playback::enqueue`
/// kind `"folder"`). Neither is resolved to tracks by every source — the
/// `Unsupported` error is the honest answer where it is not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ItemKind {
    Album,
    Track,
    Playlist,
    Artist,
    Folder,
}

impl ItemKind {
    /// The untranslated kind key a view renders. i18n stays in the frontend —
    /// this crate never links `qbz-i18n`.
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Album => "album",
            ItemKind::Track => "track",
            ItemKind::Playlist => "playlist",
            ItemKind::Artist => "artist",
            ItemKind::Folder => "folder",
        }
    }

    /// Map `qbz_models::mixtape::ItemType` (the persisted mixtape vocabulary).
    pub fn from_item_type(t: ItemType) -> ItemKind {
        match t {
            ItemType::Album => ItemKind::Album,
            ItemType::Track => ItemKind::Track,
            ItemType::Playlist => ItemKind::Playlist,
        }
    }
}

/// What a CALLER has: strings of unknown provenance.
///
/// This is the only reference a frontend constructs. Every field is a fact the
/// caller genuinely holds; nothing here is a guess, and nothing here is
/// interpreted by the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawRef {
    /// The caller's source word, NORMALISED through [`SourceId::from_word`].
    /// `None` = the caller genuinely does not know.
    pub source: Option<SourceId>,
    /// The badge the source word implies. Kept separately from `source`
    /// because `"qobuz_download"` and `"local"` are the SAME source and a
    /// DIFFERENT badge — and because `LocalSource` needs the distinction to
    /// pick the right id out of an offline row (see `local_playback.rs:85-90`,
    /// where an offline queue track carries the Qobuz track id in `id` and the
    /// local row id in `source_item_id_hint`).
    pub badge: SourceBadge,
    /// What the caller believes it is naming. `None` = unknown; each source
    /// then applies its own default (Plex: `plex:` prefix ⇒ Album, else Track).
    pub kind: Option<ItemKind>,
    /// The id as the caller has it. May be a Qobuz id, a local group key, a
    /// `plex:<hash>` album key, a namespaced numeric row id, an absolute path…
    pub id: String,
    /// `QueueTrack.is_local`, when the caller holds a live `QueueTrack`, or
    /// `AlbumSource::Local` when it holds a mixtape item. This is the ONLY
    /// disambiguator for a bare numeric id whose `source` is `None` —
    /// `resolve_bulk_qobuz_track_ids` (myqbz_play_qt.rs:246-248) already uses
    /// exactly this rule and it is the only correct one.
    pub is_local: Option<bool>,
    /// `QueueTrack.source_item_id_hint` verbatim. NEVER interpreted by a
    /// caller — only by the source that owns the shape.
    pub hint: Option<String>,
}

impl RawRef {
    /// Build a reference from a source WORD (normalised through
    /// [`SourceId::from_word`]), a kind and an id.
    pub fn new(source: &str, kind: ItemKind, id: &str) -> Self {
        Self {
            source: SourceId::from_word(source),
            badge: SourceBadge::from_word(Some(source)),
            kind: Some(kind),
            id: id.to_string(),
            is_local: None,
            hint: None,
        }
    }

    /// Build a reference when the caller already holds a typed [`SourceId`].
    pub fn typed(source: SourceId, kind: ItemKind, id: &str) -> Self {
        Self {
            source: Some(source),
            badge: SourceBadge::from_word(Some(source.as_str())),
            kind: Some(kind),
            id: id.to_string(),
            is_local: None,
            hint: None,
        }
    }

    /// Build a reference from a live queue row.
    ///
    /// Moved from the four hand-written routers that read these same fields:
    /// `local_playback::play_audible` (local_playback.rs:237-245),
    /// `local_album_actions::play_audible` (local_album_actions.rs:439-450),
    /// `local_playback::play_current_if_local` (:253-264) and
    /// `cast_qt::cast_track` (cast_qt.rs:459-478).
    pub fn from_queue_track(t: &QueueTrack) -> Self {
        let word = t.source.as_deref().unwrap_or("");
        Self {
            source: SourceId::from_word(word),
            badge: SourceBadge::from_word(Some(word)),
            kind: Some(ItemKind::Track),
            id: t.id.to_string(),
            is_local: Some(t.is_local),
            hint: t.source_item_id_hint.clone(),
        }
    }

    /// Build a reference from a persisted MyQBZ collection item.
    ///
    /// `AlbumSource::Local` is deliberately NOT mapped to [`SourceId::LOCAL`].
    /// The model's own doc comment says so (`qbz-models/src/mixtape.rs:39-43`):
    /// `Local` is an UMBRELLA covering file / plex / qobuz_download / …, so
    /// every producer of a Plex item has to store it as `Local` (survey IC-6).
    /// Mapping it to `LOCAL` would make `LocalSource` claim `plex:<hash>` keys
    /// and collide with `PlexSource`. It means "not Qobuz", which is exactly
    /// `is_local = Some(true)` — and the id shape then decides.
    pub fn from_mixtape_item(it: &MixtapeCollectionItem) -> Self {
        let (source, is_local) = match it.source {
            AlbumSource::Qobuz => (Some(SourceId::QOBUZ), Some(false)),
            AlbumSource::Local => (None, Some(true)),
        };
        Self {
            source,
            badge: match it.source {
                AlbumSource::Qobuz => SourceBadge::Qobuz,
                AlbumSource::Local => SourceBadge::Local,
            },
            kind: Some(ItemKind::from_item_type(it.item_type)),
            id: it.source_item_id.clone(),
            is_local,
            hint: None,
        }
    }

    /// `self.id` parsed as an unsigned integer, or `None`.
    ///
    /// THE single numeric accessor. There is deliberately no separate
    /// `numeric_id` field: the queue path and the mixtape path put the same
    /// number in different places (a live `QueueTrack.id` is a `u64`; a
    /// `MixtapeCollectionItem.source_item_id` is that same number as a
    /// `String`), and a design that reads only the former silently drops every
    /// mixtape Plex TRACK item (survey IC-3).
    pub fn numeric(&self) -> Option<u64> {
        self.id.trim().parse::<u64>().ok()
    }

    /// The hint, trimmed, when it is present and non-empty.
    pub fn hint_str(&self) -> Option<&str> {
        self.hint.as_deref().map(str::trim).filter(|h| !h.is_empty())
    }
}

/// A NORMALISED, source-owned reference.
///
/// Fields are `pub(crate)`: the ONLY way to obtain one outside this crate is
/// [`crate::SourceRegistry::claim`]. See the module docs for why.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct MediaRef {
    pub(crate) source: SourceId,
    pub(crate) kind: ItemKind,
    pub(crate) id: String,
}

impl MediaRef {
    /// Crate-private constructor. Only a `Source::claim` implementation calls
    /// it, which is what makes the id shape the source's property.
    pub(crate) fn new(source: SourceId, kind: ItemKind, id: impl Into<String>) -> Self {
        Self {
            source,
            kind,
            id: id.into(),
        }
    }

    /// Which source owns this reference.
    pub fn source(&self) -> SourceId {
        self.source
    }

    /// What kind of thing it names.
    pub fn kind(&self) -> ItemKind {
        self.kind
    }

    /// The normalised id — safe to persist and to round-trip back through
    /// [`RawRef`], and meaningless to interpret. Read it for logging and for
    /// storage; never parse it.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Round-trip back to a caller-side reference (for persisting a queue row
    /// or re-claiming later). The source word is stamped, so the round trip is
    /// unambiguous.
    pub fn to_raw(&self) -> RawRef {
        RawRef::typed(self.source, self.kind, &self.id)
    }
}

impl std::fmt::Display for MediaRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.source, self.kind.label(), self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_words_map_through_one_table() {
        // survey §1: six vocabularies, one mapping.
        assert_eq!(SourceId::from_word("qobuz"), Some(SourceId::QOBUZ));
        assert_eq!(SourceId::from_word("plex"), Some(SourceId::PLEX));
        assert_eq!(SourceId::from_word("local"), Some(SourceId::LOCAL));
        assert_eq!(SourceId::from_word("user"), Some(SourceId::LOCAL));
        assert_eq!(SourceId::from_word("qobuz_download"), Some(SourceId::LOCAL));
        assert_eq!(SourceId::from_word("qobuz_purchase"), Some(SourceId::LOCAL));
        assert_eq!(SourceId::from_word("offline"), Some(SourceId::LOCAL));
        assert_eq!(SourceId::from_word("ephemeral"), Some(SourceId::LOCAL));
        assert_eq!(SourceId::from_word("jellyfin"), Some(SourceId::JELLYFIN));
        // Every Subsonic BRAND folds to one source: they speak one API and QBZ
        // reads them through one impl, so a row stamped with the server's name
        // must still resolve. A `"navidrome"` that returned None would be
        // `Unclaimed`, and the row would refuse to play.
        for brand in ["subsonic", "navidrome", "gonic", "airsonic", "astiga"] {
            assert_eq!(
                SourceId::from_word(brand),
                Some(SourceId::SUBSONIC),
                "{brand} did not fold"
            );
        }
        // Unknown / absent is NOT guessed into Qobuz (survey IC-6). This
        // assertion used to be spelled with "jellyfin", which is why it needed
        // rewriting rather than deleting when Jellyfin became real — the
        // PROPERTY is what matters, not the example.
        assert_eq!(SourceId::from_word(""), None);
        assert_eq!(SourceId::from_word("emby"), None);
        assert_eq!(SourceId::from_word("???"), None);
    }

    #[test]
    fn offline_is_local_source_but_offline_badge() {
        // "an offline copy is a FILE; the distinction is a badge, not a source"
        let r = RawRef::new("qobuz_download", ItemKind::Track, "139578884");
        assert_eq!(r.source, Some(SourceId::LOCAL));
        assert_eq!(r.badge, SourceBadge::Offline);
    }

    #[test]
    fn legacy_qconnect_queue_source_is_qobuz() {
        let r = RawRef::new("qobuz_connect_remote", ItemKind::Track, "128659874");
        assert_eq!(r.source, Some(SourceId::QOBUZ));
        assert_eq!(r.badge, SourceBadge::Qobuz);
    }

    #[test]
    fn numeric_reads_the_id_on_both_paths() {
        // survey IC-3: the queue path holds a u64, the mixtape path holds the
        // same number as a String. One accessor, both paths.
        assert_eq!(
            RawRef::new("plex", ItemKind::Track, "1099511671736").numeric(),
            Some(1_099_511_671_736)
        );
        assert_eq!(
            RawRef::new("qobuz", ItemKind::Album, "e94no900otyrz").numeric(),
            None
        );
        assert_eq!(
            RawRef::new("qobuz", ItemKind::Album, "0060254702523").numeric(),
            Some(60_254_702_523)
        );
    }

    #[test]
    fn mixtape_local_umbrella_is_not_the_local_source() {
        // qbz-models/src/mixtape.rs:39-43 — `Local` covers file/plex/download.
        let it = MixtapeCollectionItem {
            collection_id: "c".into(),
            position: 0,
            item_type: ItemType::Album,
            source: AlbumSource::Local,
            source_item_id: "plex:5677211365378243606".into(),
            title: "t".into(),
            subtitle: None,
            artwork_url: None,
            year: None,
            track_count: None,
            added_at: 0,
        };
        let raw = RawRef::from_mixtape_item(&it);
        assert_eq!(raw.source, None, "Local is an umbrella, not SourceId::LOCAL");
        assert_eq!(raw.is_local, Some(true));
        assert_eq!(raw.kind, Some(ItemKind::Album));
    }
}
