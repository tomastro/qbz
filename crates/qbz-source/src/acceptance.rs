//! **R1 / R3 — which container accepts which item.** One rule, one predicate,
//! one place.
//!
//! # The rules, verbatim
//!
//! - **R1.** A **Collection** takes albums, singles and EPs, from ANY source —
//!   `qobuz` or `local`, and `local` covers folders, Plex and anything added
//!   later. A **Mixtape** takes albums, playlists and tracks.
//! - **R2.** An unsupported combination must NEVER be visible. The `Err` a
//!   resolver returns stays where it is as the safety net that only reaches the
//!   log; the fix belongs in the UI, which must simply not offer the impossible
//!   action. That is what this module is for: [`Container::accepts`] is the
//!   filter the UI applies BEFORE it renders an action.
//! - **R3.** Ephemeral honours its name. An ephemeral item can only be PLAYED
//!   and only be added to the QUEUE — never to a playlist, a mixtape or a
//!   collection. It leaves no trace in any library.
//! - **R4.** A row's source comes from the ROW, never from a literal. Note how
//!   the rule below reads: the Collection arm never looks at
//!   [`ItemFacts::source`] at all (R1 says "any source"), and the one arm that
//!   does needs POSITIVE evidence of Qobuz. There is no `"qobuz"` literal for a
//!   caller to get wrong — build [`ItemFacts`] from the row with
//!   [`ItemFacts::from_raw`] / [`ItemFacts::from_media`] / [`ItemFacts::with_source_word`].
//!
//! # What this replaces
//!
//! `myqbz_add_qt`'s `restrict_to_mixtape` boolean, which could only say
//! "mixtapes only" vs "everything" — it could not say that a LOCAL album
//! belongs in a Collection while a LOCAL playlist belongs nowhere, and it could
//! not say anything at all about a per-row flyout. The rule is a property of
//! the ITEM, so it lives with the item's other properties.
//!
//! # "album", "single" and "EP" are RELEASE TYPES, not kinds
//!
//! The codebase already spells this out: `myqbz_builder_qt::classify_release`
//! (myqbz_builder_qt.rs:287-327) returns one of `"album" | "ep" | "single" |
//! "live" | "compilation"` for something that is one `ItemType::Album`, and
//! `qbz_models`' `Album.release_type` (types.rs:470) carries the same word off
//! the wire. So R1's "albums, singles, EPs" is ONE [`ItemKind::Album`] slot
//! ([`Accepted::Release`]), not three kinds — see [`RELEASE_TYPES`].

use qbz_library::ephemeral::EPHEMERAL_ID_FLOOR;

use crate::id::{ItemKind, MediaRef, RawRef, SourceId};
use crate::meta::ItemMeta;

/// Every release-type word the codebase can produce for an
/// [`ItemKind::Album`], from `classify_release` (myqbz_builder_qt.rs:287-327)
/// and `Album.release_type` (`qbz-models/src/types.rs:470`).
///
/// R1 names the first three. The predicate does **not** gate on this list: a
/// live record and a compilation are still albums, and an unrecognised or
/// missing word must not make a legitimate album disappear from the picker
/// (that would be R2 in reverse — an invisible refusal). The list is here so
/// the vocabulary has ONE home and so the tests can pin it.
pub const RELEASE_TYPES: [&str; 5] = ["album", "single", "ep", "live", "compilation"];

/// Is `w` a release-type word this codebase knows?
pub fn is_release_word(w: &str) -> bool {
    RELEASE_TYPES
        .iter()
        .any(|r| w.trim().eq_ignore_ascii_case(r))
}

/// Is this id a session-scoped EPHEMERAL id (R3)?
///
/// The test is on the ID ALONE, deliberately — never on the row's source word.
/// Several call sites stamp a literal `"source": "qobuz"` onto whatever is
/// playing (`PlayerBar.qml:559`, `NowPlayingBarSmall.qml:508`), so a
/// source-gated test would wave an ephemeral now-playing track straight
/// through. Ephemeral ids are synthetic and high (`>= 2^48`,
/// `qbz_library::ephemeral::EPHEMERAL_ID_FLOOR`); neither a `local_tracks` row
/// id, nor a Plex namespaced id (`2^40`), nor a Qobuz track id ever reaches
/// that floor. Same test as `LocalSource::is_ephemeral`
/// (`sources/local.rs:244`) and `local_ephemeral::is_ephemeral_id`.
pub fn is_ephemeral_id(id: &str) -> bool {
    matches!(id.trim().parse::<i64>(), Ok(n) if n >= EPHEMERAL_ID_FLOOR)
}

/// A container the user can put an item INTO.
///
/// [`Collection`](Container::Collection) and [`Mixtape`](Container::Mixtape)
/// are R1's two. [`Queue`](Container::Queue) and
/// [`Playlist`](Container::Playlist) are named by R3 ("only be added to the
/// QUEUE… never to a playlist"), and are needed for R3 to be expressible at
/// all.
///
/// The MyQBZ picker's third row kind, an *artist collection* (a built
/// discography), is a [`Collection`](Container::Collection) here: it holds
/// whole albums and accepts exactly what a collection accepts. The only
/// difference is that it cannot be created from the picker, which is a UI
/// property, not an acceptance rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Container {
    /// The play queue. The only container R3 lets an ephemeral item reach.
    Queue,
    /// A MyQBZ collection (and an artist collection).
    Collection,
    /// A MyQBZ mixtape.
    Mixtape,
    /// A playlist.
    Playlist,
}

/// One thing a container accepts — R1's vocabulary, as data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accepted {
    /// A release: an album, a single or an EP. All three are RELEASE TYPES of
    /// the one [`ItemKind::Album`] (see [`RELEASE_TYPES`]), so this is one
    /// slot, not three.
    Release,
    /// A playlist — and only one this build can actually resolve.
    ///
    /// Neither [`crate::LocalSource`] nor [`crate::PlexSource`] claims a
    /// playlist: both answer [`crate::SourceError::Unsupported`]. That error
    /// is the unsupported combination R2 talks about; it stays as the safety
    /// net, and this slot is what makes it unreachable from the UI.
    Playlist,
    /// A single track.
    Track,
    /// A filesystem folder. `local_playback::enqueue` routes kind `"folder"`
    /// and [`crate::LocalSource`] resolves it (`local.rs:589-591`); it is
    /// playable, so it is queueable, and it is storable nowhere.
    Folder,
}

/// Why a container refused an item.
///
/// **Log-only, by R2.** These strings are never rendered: an unsupported
/// combination must never be visible, so a refusal means the UI should not have
/// offered the action in the first place, and reaching one is a bug report to
/// the log — not a message to a user. There is deliberately no msgid for any of
/// it and there must not be one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// R3: ephemeral leaves no trace.
    Ephemeral,
    /// R1: this container does not take this kind of item at all.
    Kind(ItemKind),
    /// The container takes playlists, but not one from this source — nothing
    /// outside Qobuz resolves to a playlist (see [`Accepted::Playlist`]).
    /// `None` = the row did not say where it came from.
    UnresolvablePlaylist(Option<SourceId>),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Ephemeral => f.write_str("ephemeral items are play/queue only (R3)"),
            Refusal::Kind(k) => write!(f, "this container does not accept a {} (R1)", k.label()),
            Refusal::UnresolvablePlaylist(src) => match src {
                Some(s) => write!(
                    f,
                    "a {s} playlist cannot be resolved; only qobuz playlists can"
                ),
                None => f.write_str("a playlist with no source cannot be resolved"),
            },
        }
    }
}

/// The facts a row carries about ONE item — everything the rule needs, and
/// nothing else.
///
/// Every field comes from the ROW (R4). Build it with [`ItemFacts::from_raw`]
/// or [`ItemFacts::from_media`] where a reference is already in hand, or with
/// [`ItemFacts::new`] plus the `with_*` builders where the caller is holding
/// loose row fields.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemFacts<'a> {
    /// What the row is.
    pub kind: ItemKind,
    /// The release-type word for an [`ItemKind::Album`] — one of
    /// [`RELEASE_TYPES`], as `classify_release` spells it. Informational: see
    /// [`RELEASE_TYPES`] for why the rule does not gate on it.
    pub release: Option<&'a str>,
    /// Where the row came from, normalised. `None` = the row did not say.
    pub source: Option<SourceId>,
    /// R3. Normally derived from the id by [`is_ephemeral_id`].
    pub ephemeral: bool,
}

impl<'a> ItemFacts<'a> {
    /// The minimum: a kind, no source word, not ephemeral.
    pub fn new(kind: ItemKind) -> Self {
        Self {
            kind,
            release: None,
            source: None,
            ephemeral: false,
        }
    }

    /// Stamp the row's source, already typed.
    pub fn with_source(mut self, source: SourceId) -> Self {
        self.source = Some(source);
        self
    }

    /// Stamp the row's source WORD, normalised through the one table
    /// ([`SourceId::from_word`]). An unknown word stays `None` — it is never
    /// guessed into Qobuz.
    pub fn with_source_word(mut self, word: &str) -> Self {
        self.source = SourceId::from_word(word);
        self
    }

    /// Stamp the release-type word (`classify_release`'s output, or
    /// `Album.release_type` off the wire).
    pub fn with_release(mut self, release: &'a str) -> Self {
        self.release = Some(release);
        self
    }

    /// Force the R3 bit, for a caller that knows an item is ephemeral by other
    /// means than its id — e.g. an ephemeral ALBUM, whose group key
    /// `<album>|<artist>` is not numeric and so cannot carry the floor.
    pub fn with_ephemeral(mut self, ephemeral: bool) -> Self {
        self.ephemeral = ephemeral;
        self
    }

    /// The facts of a caller-side reference. `None` when the row does not know
    /// its own kind — there is nothing to offer for an item nobody can
    /// classify, and guessing is what this crate exists to stop.
    pub fn from_raw(raw: &RawRef) -> Option<ItemFacts<'static>> {
        Some(ItemFacts {
            kind: raw.kind?,
            release: None,
            source: raw.source,
            ephemeral: is_ephemeral_id(&raw.id),
        })
    }

    /// The facts of a claimed reference.
    pub fn from_media(item: &MediaRef) -> ItemFacts<'static> {
        ItemFacts {
            kind: item.kind(),
            release: None,
            source: Some(item.source()),
            ephemeral: is_ephemeral_id(item.id()),
        }
    }

    /// The facts of a claimed reference plus the metadata its source produced —
    /// which is where the release-type word comes from
    /// ([`ItemMeta::kind_label`], stamped by `QobuzSource::meta` from the V2
    /// wire's `release_type`).
    pub fn from_meta(item: &MediaRef, meta: &ItemMeta) -> ItemFacts<'static> {
        ItemFacts {
            release: Some(meta.kind_label),
            ..ItemFacts::from_media(item)
        }
    }
}

// ── The rule, as data ───────────────────────────────────────────────────────
//
// Four lines. R1 is the middle two, verbatim; R3 is the `Queue` line plus the
// ephemeral veto in `refusal` below.

/// R1: "Collection: albums, singles, EPs."
const COLLECTION: &[Accepted] = &[Accepted::Release];
/// R1: "Mixtape: albums, playlists, tracks."
const MIXTAPE: &[Accepted] = &[Accepted::Release, Accepted::Playlist, Accepted::Track];
/// R3: an ephemeral item may be played and queued. Everything else that a
/// source can expand into tracks may be queued too — `Artist` is absent
/// because no source resolves one (`local.rs:601-606`, `plex.rs:307-310`, and
/// `QobuzSource` has no artist arm either).
const QUEUE: &[Accepted] = &[
    Accepted::Release,
    Accepted::Playlist,
    Accepted::Track,
    Accepted::Folder,
];
/// A playlist holds tracks. R1 does not describe playlists as containers — this
/// line exists so R3's "never to a playlist" is expressible.
const PLAYLIST: &[Accepted] = &[Accepted::Track];

impl Accepted {
    /// Does this slot match the item?
    fn matches(self, item: &ItemFacts<'_>) -> bool {
        match self {
            // Any release type of the one album kind — see `RELEASE_TYPES`.
            Accepted::Release => item.kind == ItemKind::Album,
            // POSITIVE evidence of Qobuz, never "not local" (R4, and the
            // `_ => Qobuz` guess this crate refuses to reproduce).
            Accepted::Playlist => {
                item.kind == ItemKind::Playlist && item.source == Some(SourceId::QOBUZ)
            }
            Accepted::Track => item.kind == ItemKind::Track,
            Accepted::Folder => item.kind == ItemKind::Folder,
        }
    }
}

impl Container {
    /// Every container, for a caller that wants to project the rule onto its
    /// own row list in one pass.
    pub const ALL: [Container; 4] = [
        Container::Queue,
        Container::Collection,
        Container::Mixtape,
        Container::Playlist,
    ];

    /// What this container accepts — R1/R3 as data, readable and testable
    /// without going through the predicate.
    pub fn accepted(self) -> &'static [Accepted] {
        match self {
            Container::Queue => QUEUE,
            Container::Collection => COLLECTION,
            Container::Mixtape => MIXTAPE,
            Container::Playlist => PLAYLIST,
        }
    }

    /// **THE predicate.** May this item go in this container?
    ///
    /// This is what the UI asks before it renders an action. A `false` here
    /// means the action is NOT OFFERED — never offered and then refused (R2).
    pub fn accepts(self, item: &ItemFacts<'_>) -> bool {
        self.refusal(item).is_none()
    }

    /// The same question, with the reason — for the LOG only (see [`Refusal`]).
    pub fn refusal(self, item: &ItemFacts<'_>) -> Option<Refusal> {
        // R3 first, and it vetoes every container but the queue.
        if item.ephemeral && self != Container::Queue {
            return Some(Refusal::Ephemeral);
        }
        let slots = self.accepted();
        if slots.iter().any(|s| s.matches(item)) {
            return None;
        }
        if item.kind == ItemKind::Playlist && slots.contains(&Accepted::Playlist) {
            // The container does take playlists; this one just cannot be
            // resolved. Worth its own reason — it is the arm R2 is about.
            return Some(Refusal::UnresolvablePlaylist(item.source));
        }
        Some(Refusal::Kind(item.kind))
    }

    /// The BATCH form: a container is offered only when it accepts EVERY item.
    ///
    /// An empty payload accepts nothing — there is nothing to add, so no
    /// container should light up.
    pub fn accepts_all(self, items: &[ItemFacts<'_>]) -> bool {
        !items.is_empty() && items.iter().all(|it| self.accepts(it))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(source: SourceId) -> ItemFacts<'static> {
        ItemFacts::new(ItemKind::Album).with_source(source)
    }

    // ── R1: what a Collection takes ────────────────────────────────────────

    #[test]
    fn a_collection_takes_albums_singles_and_eps_from_any_source() {
        for source in [SourceId::QOBUZ, SourceId::LOCAL, SourceId::PLEX] {
            for release in ["album", "single", "ep"] {
                let it = album(source).with_release(release);
                assert!(
                    Container::Collection.accepts(&it),
                    "a {release} from {source} belongs in a collection (R1)"
                );
            }
        }
    }

    #[test]
    fn a_plex_album_is_accepted_by_a_collection() {
        // R1: "from ANY source — qobuz, local (which means folders AND Plex
        // AND anything added later)".
        assert!(Container::Collection.accepts(&album(SourceId::PLEX)));
        assert_eq!(Container::Collection.refusal(&album(SourceId::PLEX)), None);
    }

    #[test]
    fn a_release_word_never_hides_an_album() {
        // "live" and "compilation" are release types of the SAME kind. R1
        // names three of the five; refusing the other two would be an
        // invisible refusal of a legitimate album.
        for release in RELEASE_TYPES {
            assert!(is_release_word(release));
            assert!(Container::Collection.accepts(&album(SourceId::QOBUZ).with_release(release)));
        }
        // An unclassified album, and one carrying a word nobody knows.
        assert!(Container::Collection.accepts(&album(SourceId::LOCAL)));
        assert!(Container::Collection.accepts(&album(SourceId::LOCAL).with_release("box set")));
    }

    #[test]
    fn a_collection_takes_neither_tracks_nor_playlists() {
        let track = ItemFacts::new(ItemKind::Track).with_source(SourceId::QOBUZ);
        assert!(!Container::Collection.accepts(&track));
        assert_eq!(
            Container::Collection.refusal(&track),
            Some(Refusal::Kind(ItemKind::Track))
        );

        let playlist = ItemFacts::new(ItemKind::Playlist).with_source(SourceId::QOBUZ);
        assert!(!Container::Collection.accepts(&playlist));
        assert_eq!(
            Container::Collection.refusal(&playlist),
            Some(Refusal::Kind(ItemKind::Playlist)),
            "a collection does not take playlists at all — not even qobuz ones"
        );
    }

    // ── R1: what a Mixtape takes ───────────────────────────────────────────

    #[test]
    fn a_playlist_is_refused_by_a_collection_and_accepted_by_a_mixtape() {
        let playlist = ItemFacts::new(ItemKind::Playlist).with_source_word("qobuz");
        assert!(!Container::Collection.accepts(&playlist));
        assert!(Container::Mixtape.accepts(&playlist));
    }

    #[test]
    fn a_mixtape_takes_albums_playlists_and_tracks() {
        assert!(Container::Mixtape.accepts(&album(SourceId::LOCAL)));
        assert!(Container::Mixtape
            .accepts(&ItemFacts::new(ItemKind::Track).with_source(SourceId::LOCAL)));
        assert!(Container::Mixtape
            .accepts(&ItemFacts::new(ItemKind::Playlist).with_source(SourceId::QOBUZ)));
    }

    #[test]
    fn a_non_qobuz_playlist_belongs_nowhere() {
        // enqueue.rs:418-424 — "local playlists not supported in this
        // release". R2: the error stays as the safety net; the UI simply never
        // offers the action.
        for source in [Some(SourceId::LOCAL), Some(SourceId::PLEX), None] {
            let mut it = ItemFacts::new(ItemKind::Playlist);
            it.source = source;
            assert!(!Container::Mixtape.accepts(&it), "{source:?} playlist");
            assert_eq!(
                Container::Mixtape.refusal(&it),
                Some(Refusal::UnresolvablePlaylist(source))
            );
            assert!(!Container::Collection.accepts(&it));
            assert!(!Container::Queue.accepts(&it));
        }
    }

    // ── R3: ephemeral ──────────────────────────────────────────────────────

    #[test]
    fn an_ephemeral_item_is_refused_by_both_collection_and_mixtape() {
        // qbz-library/src/ephemeral.rs:43 — EPHEMERAL_ID_FLOOR = 2^48.
        let raw = RawRef {
            kind: Some(ItemKind::Track),
            id: "281474976710656".into(),
            ..Default::default()
        };
        let it = ItemFacts::from_raw(&raw).expect("a track kind");
        assert!(it.ephemeral);

        for c in [
            Container::Collection,
            Container::Mixtape,
            Container::Playlist,
        ] {
            assert!(!c.accepts(&it), "{c:?} must refuse an ephemeral item (R3)");
            assert_eq!(c.refusal(&it), Some(Refusal::Ephemeral));
        }
    }

    #[test]
    fn an_ephemeral_item_can_still_be_queued() {
        // R3: "only be played and only be added to the QUEUE".
        let it = ItemFacts::new(ItemKind::Track).with_ephemeral(true);
        assert!(Container::Queue.accepts(&it));
        assert_eq!(Container::Queue.refusal(&it), None);
    }

    #[test]
    fn the_ephemeral_test_reads_the_id_not_the_source_word() {
        // PlayerBar.qml:559 / NowPlayingBarSmall.qml:508 stamp a literal
        // "qobuz" onto whatever is playing; a source-gated test would wave the
        // ephemeral row straight through.
        let lying = RawRef::new("qobuz", ItemKind::Track, "281474976710657");
        let it = ItemFacts::from_raw(&lying).expect("a track kind");
        assert!(it.ephemeral);
        assert_eq!(it.source, Some(SourceId::QOBUZ));
        assert!(!Container::Mixtape.accepts(&it));

        // …and a normal library row id is NOT ephemeral.
        assert!(!is_ephemeral_id("2954"));
        assert!(!is_ephemeral_id("139578884"));
        assert!(!is_ephemeral_id("HIT ME HARD AND SOFT|Billie Eilish"));
        assert!(is_ephemeral_id(&EPHEMERAL_ID_FLOOR.to_string()));
    }

    #[test]
    fn an_ephemeral_album_needs_the_explicit_bit() {
        // An ephemeral ALBUM is grouped by `<album>|<artist>`, which carries no
        // floor. The caller that knows says so; without it the id test cannot.
        let key = RawRef {
            kind: Some(ItemKind::Album),
            id: "Folder Drop|Some Artist".into(),
            ..Default::default()
        };
        let derived = ItemFacts::from_raw(&key).expect("an album kind");
        assert!(
            !derived.ephemeral,
            "no floor in a group key — see the report"
        );
        assert!(!Container::Collection.accepts(&derived.with_ephemeral(true)));
    }

    // ── Shape ──────────────────────────────────────────────────────────────

    #[test]
    fn a_row_with_no_kind_has_no_facts() {
        let raw = RawRef {
            id: "2954".into(),
            ..Default::default()
        };
        assert!(ItemFacts::from_raw(&raw).is_none());
    }

    #[test]
    fn a_batch_is_offered_only_when_every_item_fits() {
        let items = [
            album(SourceId::QOBUZ),
            album(SourceId::PLEX),
            ItemFacts::new(ItemKind::Track).with_source(SourceId::LOCAL),
        ];
        assert!(Container::Mixtape.accepts_all(&items));
        assert!(
            !Container::Collection.accepts_all(&items),
            "one track in the batch closes the collection"
        );
        assert!(Container::Collection.accepts_all(&items[..2]));
        // An empty payload lights nothing up.
        assert!(!Container::Collection.accepts_all(&[]));
        assert!(!Container::Mixtape.accepts_all(&[]));
    }

    #[test]
    fn the_rule_is_readable_as_data() {
        assert_eq!(Container::Collection.accepted(), &[Accepted::Release]);
        assert_eq!(
            Container::Mixtape.accepted(),
            &[Accepted::Release, Accepted::Playlist, Accepted::Track]
        );
        assert_eq!(Container::ALL.len(), 4);
    }

    #[test]
    fn facts_come_from_a_claimed_reference_too() {
        let item = MediaRef::new(SourceId::PLEX, ItemKind::Album, "plex:5677211365378243606");
        let facts = ItemFacts::from_media(&item);
        assert_eq!(facts.source, Some(SourceId::PLEX));
        assert!(Container::Collection.accepts(&facts));

        let meta = ItemMeta {
            kind_label: "ep",
            ..Default::default()
        };
        let with_meta = ItemFacts::from_meta(&item, &meta);
        assert_eq!(with_meta.release, Some("ep"));
        assert!(Container::Collection.accepts(&with_meta));
    }
}
