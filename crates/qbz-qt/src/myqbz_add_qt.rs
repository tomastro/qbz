//! "Add to Mixtape/Collection" controller — the Slint-free port of
//! `crates/qbz/src/myqbz_add.rs` plus its `main.rs:5893-6015` action wiring.
//!
//! Every app-wide "Add to Mixtape/Collection" trigger hands this module a batch
//! of [`AddItem`]s (from QML as a JSON array through `QbzMyQbzAdd.open`, or
//! directly from a Rust seam such as the Local Library bulk bar). The batch is
//! held in a process-global PENDING store; the picker list is loaded
//! R1-filtered + recency-sorted + `item_exists`-resolved, and on pick /
//! create-and-add the items are written into the chosen collection through the
//! shared `qbz_mixtape::repo`.
//!
//! WHAT MAY GO WHERE is decided ONCE, by [`payload_accepts`] (the R1 rule
//! below). The picker never renders a container that cannot accept the payload,
//! never offers a create chip or a create radio for one, and refuses to open at
//! all when NOTHING can accept it. Every refusal is a `log::warn!` and nothing
//! else: an impossible combination must not be reachable, so the user never
//! sees a block — the block only exists as a safety net behind the UI (R2).
//!
//! Dedup is the backend's job: `add_item_with(allow_duplicate = false)` returns
//! `Ok(false)` for an exact `(collection_id, source, source_item_id)` duplicate
//! — that is NOT an error. We count added vs skipped and surface the net result
//! as a toast, which for a fully-duplicate batch ("Already in {name}") is the
//! ONLY feedback the flow produces.
//!
//! Publishes ONE document, `addJson` (spec 02 §5.3), on its own singleton:
//! `QbzMyQbzAdd` is separate from `QbzMyQbz` because it is the only MyQBZ
//! surface other domains' QML touches (TrackRow, PlayerBar,
//! NowPlayingBarSmall, the Local Library bulk surfaces), so none of them can
//! couple to detail state.
//!
//! Threading: every DB touch runs inside `tokio::task::spawn_blocking`
//! (`LibraryDatabase` wraps a `!Send` rusqlite `Connection`), one open
//! connection per batch.

use std::sync::{LazyLock, Mutex};

use cxx_qt_lib::QString;
use qbz_models::mixtape::{AlbumSource, CollectionKind, ItemType, MixtapeCollection};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Payload (spec 02 §5.3.1)
// ---------------------------------------------------------------------------

/// One pending item to add. Built by each call site from its row / album /
/// playlist data. `source_item_id` is ALWAYS a string (numeric ids are
/// stringified by the caller). Unknown keys are ignored and every optional key
/// deserializes to `None`.
#[derive(Clone, Debug, Deserialize)]
pub struct AddItem {
    /// "album" | "track" | "playlist".
    #[serde(rename = "itemType")]
    pub item_type: String,
    /// "qobuz" | "local" — Plex rows pass "local"; there is no "plex" source
    /// (`myqbz_add.rs:30`, and `source_from_str` maps anything that is not
    /// "local" to Qobuz).
    pub source: String,
    #[serde(rename = "sourceItemId")]
    pub source_item_id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default, rename = "artworkUrl")]
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default, rename = "trackCount")]
    pub track_count: Option<i32>,
}

/// Pending items for the currently-open picker. Set by [`open_items`], read by
/// the add / create handlers, cleared on [`close`].
static PENDING: LazyLock<Mutex<Vec<AddItem>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Snapshot of the pending items (a clone — the store is cleared on close, not
/// on read, so a failed write can be retried from the same open modal).
fn pending_snapshot() -> Vec<AddItem> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn item_type_from_str(s: &str) -> ItemType {
    match s {
        "track" => ItemType::Track,
        "playlist" => ItemType::Playlist,
        _ => ItemType::Album,
    }
}

fn source_from_str(s: &str) -> AlbumSource {
    match s {
        "local" => AlbumSource::Local,
        _ => AlbumSource::Qobuz,
    }
}

// ---------------------------------------------------------------------------
// R1 — which container accepts which item. THE ONE PLACE.
// ---------------------------------------------------------------------------
//
// >>> TEMPORARY HOME. This rule MUST move to `qbz-source` <<<
//
// It is a property of the ITEM (its kind and its source), not of the MyQBZ
// picker, and the picker is only its first caller: the per-row flyouts
// (TrackRow, PlayerBar, NowPlayingBarSmall, the Local Library bulk bars) have
// to ask the same question to decide whether to render "Add to mixtape" at all.
// The moment a second call site re-derives it by hand, the rule has been
// scattered and the class of bug this replaces is back. When `qbz-source`
// grows the predicate, delete `Accepts` / `accepts_item` / `payload_accepts`
// here and forward to it; NOTHING else in this file reads the rule directly.

/// The set of container kinds that may accept a payload.
///
/// Read it as three independent yes/no answers rather than a "restriction
/// level": the old `restrict_to_mixtape` boolean could only express "mixtapes
/// only" vs "everything", which cannot say that a LOCAL album belongs in a
/// Collection while a LOCAL playlist belongs nowhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Accepts {
    pub mixtape: bool,
    pub collection: bool,
    /// A built discography. It is a Collection in every way that matters here
    /// (it holds whole albums), it is just not creatable from this picker —
    /// hence a third flag rather than reusing `collection`.
    pub artist_collection: bool,
}

impl Accepts {
    const NONE: Accepts = Accepts {
        mixtape: false,
        collection: false,
        artist_collection: false,
    };
    const ALL: Accepts = Accepts {
        mixtape: true,
        collection: true,
        artist_collection: true,
    };
    const MIXTAPE_ONLY: Accepts = Accepts {
        mixtape: true,
        collection: false,
        artist_collection: false,
    };

    /// Anything at all can take this payload.
    fn any(self) -> bool {
        self.mixtape || self.collection || self.artist_collection
    }

    /// A batch is offerable to a container only if EVERY item is.
    fn intersect(self, other: Accepts) -> Accepts {
        Accepts {
            mixtape: self.mixtape && other.mixtape,
            collection: self.collection && other.collection,
            artist_collection: self.artist_collection && other.artist_collection,
        }
    }

    /// The picker's row filter.
    fn allows(self, kind: CollectionKind) -> bool {
        match kind {
            CollectionKind::Mixtape => self.mixtape,
            CollectionKind::Collection => self.collection,
            CollectionKind::ArtistCollection => self.artist_collection,
        }
    }

    /// The create sub-panel's filter. `"collection"` maps to `Collection`,
    /// anything else to `Mixtape` — the same mapping [`create_collection`]
    /// applies, so the gate and the write can never disagree.
    fn allows_create(self, kind: &str) -> bool {
        if kind == "collection" {
            self.collection
        } else {
            self.mixtape
        }
    }

    /// The kind the create panel should open on when `wanted` is not offered:
    /// the one that IS. `None` = nothing is creatable for this payload.
    fn create_fallback(self) -> Option<&'static str> {
        if self.mixtape {
            Some("mixtape")
        } else if self.collection {
            Some("collection")
        } else {
            None
        }
    }
}

/// R3 — an ephemeral row leaves NO trace: it can be played and queued, never
/// stored. Ephemeral ids are synthetic and high (`>= 2^48`,
/// `local_ephemeral::EPHEMERAL_ID_FLOOR`), and neither a `local_tracks` row id
/// (`< 2^40`, below even `local_plex::PLEX_TRACK_ID_FLOOR`) nor a Qobuz track
/// id ever reaches that floor.
///
/// The test is on the ID ALONE, deliberately — NOT on `it.source`. Several
/// call sites stamp a literal `"source": "qobuz"` onto whatever is playing
/// (PlayerBar.qml:559, NowPlayingBarSmall.qml:508), so a source-gated check
/// would wave an ephemeral now-playing track straight through.
fn is_ephemeral_item(it: &AddItem) -> bool {
    it.source_item_id
        .trim()
        .parse::<i64>()
        .is_ok_and(crate::local_ephemeral::is_ephemeral_id)
}

/// R1 for ONE item.
///
/// - **Collection** (and artist collection): albums, singles and EPs — which
///   in this payload vocabulary are all one thing, `itemType == "album"`; there
///   is no release-type field and R1 draws no line between them. From ANY
///   source: `"local"` covers folders, Plex and Qobuz downloads alike
///   (`qbz-models/src/mixtape.rs` — `AlbumSource::Local` is an umbrella), so
///   the source is NOT consulted for an album.
/// - **Mixtape**: albums, playlists and tracks.
/// - **A LOCAL playlist belongs nowhere.** The local and LAN implementations
///   in `qbz-source` reject playlist resolution; there is no local-only
///   playlist id to resolve against. That error is the unsupported combination
///   R2 talks about, and this arm makes it unreachable from the UI.
/// - **Ephemeral: nothing** (R3).
///
/// Kind and source are read through `item_type_from_str` / `source_from_str`,
/// the same two functions [`add_items`] writes with, so the gate can never
/// classify an item differently from the row that would be inserted.
fn accepts_item(it: &AddItem) -> Accepts {
    if is_ephemeral_item(it) {
        return Accepts::NONE;
    }
    match item_type_from_str(&it.item_type) {
        ItemType::Album => Accepts::ALL,
        ItemType::Track => Accepts::MIXTAPE_ONLY,
        ItemType::Playlist => match source_from_str(&it.source) {
            AlbumSource::Qobuz => Accepts::MIXTAPE_ONLY,
            AlbumSource::Local => Accepts::NONE,
        },
    }
}

/// R1 for a BATCH: a container is offered only when it accepts every item.
/// An empty payload accepts nothing (the picker has nothing to add).
pub(crate) fn payload_accepts(items: &[AddItem]) -> Accepts {
    if items.is_empty() {
        return Accepts::NONE;
    }
    items
        .iter()
        .fold(Accepts::ALL, |acc, it| acc.intersect(accepts_item(it)))
}

/// The rule applied to whatever is currently pending. Used by the mutating
/// handlers so they re-derive from the payload rather than trusting a document
/// flag a stale QML frame could still be holding.
fn pending_accepts() -> Accepts {
    payload_accepts(&pending_snapshot())
}

// ---------------------------------------------------------------------------
// Document (spec 02 §5.3)
// ---------------------------------------------------------------------------

#[derive(Clone, Default, Serialize)]
pub struct AddRow {
    pub id: String,
    pub name: String,
    /// "mixtape" | "collection" | "artist_collection".
    pub kind: String,
    /// "cassette" | "user" | "library-big" — resolved here, not in QML.
    pub icon: String,
    /// Translated, uppercase.
    #[serde(rename = "kindLabel")]
    pub kind_label: String,
    /// "1 album" / "N albums".
    pub meta: String,
    /// True when EVERY pending item is already in this collection.
    #[serde(rename = "alreadyHas")]
    pub already_has: bool,
}

#[derive(Clone, Serialize)]
struct AddDoc {
    open: bool,
    loading: bool,
    #[serde(rename = "headerTitle")]
    header_title: String,
    #[serde(rename = "headerSubtitle")]
    header_subtitle: String,
    #[serde(rename = "bulkMode")]
    bulk_mode: bool,
    /// R1, projected for the create surfaces: whether a NEW mixtape / a NEW
    /// collection may be created for this payload. They are the create chips'
    /// and the create radios' visibility, and they are NOT a "restriction
    /// level" — both can be true, both can be false (in which case the picker
    /// never opened at all). The ROW list is filtered in Rust, so QML has no
    /// second copy of the rule to keep in sync.
    #[serde(rename = "allowMixtape")]
    allow_mixtape: bool,
    #[serde(rename = "allowCollection")]
    allow_collection: bool,
    search: String,
    /// The collection currently being written to ("" = idle).
    #[serde(rename = "busyId")]
    busy_id: String,
    creating: bool,
    /// "mixtape" | "collection".
    #[serde(rename = "createKind")]
    create_kind: String,
    #[serde(rename = "createBusy")]
    create_busy: bool,
    rows: Vec<AddRow>,
}

impl Default for AddDoc {
    fn default() -> Self {
        Self {
            open: false,
            loading: false,
            header_title: String::new(),
            header_subtitle: String::new(),
            bulk_mode: false,
            // A closed document offers nothing; `open_items` seeds the real
            // answer before `open` ever becomes true.
            allow_mixtape: false,
            allow_collection: false,
            search: String::new(),
            busy_id: String::new(),
            creating: false,
            create_kind: "mixtape".to_string(),
            create_busy: false,
            rows: Vec::new(),
        }
    }
}

static DOC: LazyLock<Mutex<AddDoc>> = LazyLock::new(|| Mutex::new(AddDoc::default()));

/// Last-loaded picker rows, so a search re-filters client-side with no refetch.
static ROWS_CACHE: LazyLock<Mutex<Vec<LoadedRow>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn with_doc<R>(f: impl FnOnce(&mut AddDoc) -> R) -> R {
    f(&mut DOC.lock().unwrap_or_else(|e| e.into_inner()))
}

fn publish() {
    let json = with_doc(|doc| serde_json::to_string(doc).unwrap_or_else(|_| "{}".into()));
    crate::myqbz_add_bridge::ui(move |mut b| {
        b.as_mut().set_add_json(QString::from(json.as_str()));
    });
}

// ---------------------------------------------------------------------------
// Row loading
// ---------------------------------------------------------------------------

/// A loaded picker row (the collection plus whether it already contains every
/// pending item). Built on a blocking worker by [`load_rows`].
pub struct LoadedRow {
    pub id: String,
    pub name: String,
    pub kind: CollectionKind,
    pub item_count: usize,
    pub already_has: bool,
}

/// Load the collections offered as targets, R1-filtered + recency-sorted +
/// `item_exists`-resolved. BLOCKING (DB) — call from `spawn_blocking`.
///
/// - `accepts` is [`payload_accepts`] for this batch and is the ONLY row
///   filter. A container the payload cannot go into is dropped here, in Rust,
///   so it never reaches the model and cannot be clicked (R2). An album batch
///   still sees artist_collections, so a built discography can be augmented
///   (`myqbz_add.rs:159-167`);
/// - sort = `last_played_at ?? updated_at` DESC;
/// - `already_has` = every pending item's `(source, source_item_id)` is already
///   in that collection.
pub(crate) fn load_rows(accepts: Accepts, items: &[AddItem]) -> Vec<LoadedRow> {
    crate::library_db_qt::with_db(false, |db| {
        Ok(db.with_connection(|conn| {
            let mut cols: Vec<MixtapeCollection> = qbz_mixtape::repo::list_collections(conn, None)
                .unwrap_or_else(|e| {
                    log::warn!("[qbz-qt] myqbz_add list_collections failed: {e}");
                    Vec::new()
                });

            cols.retain(|c| accepts.allows(c.kind));

            cols.sort_by(|a, b| {
                let ra = a.last_played_at.unwrap_or(a.updated_at);
                let rb = b.last_played_at.unwrap_or(b.updated_at);
                rb.cmp(&ra)
            });

            cols.into_iter()
                .map(|c| {
                    let already_has = !items.is_empty()
                        && items.iter().all(|it| {
                            qbz_mixtape::repo::item_exists(
                                conn,
                                &c.id,
                                source_from_str(&it.source),
                                &it.source_item_id,
                            )
                            .unwrap_or(false)
                        });
                    LoadedRow {
                        id: c.id,
                        name: c.name,
                        kind: c.kind,
                        item_count: c.items.len(),
                        already_has,
                    }
                })
                .collect::<Vec<_>>()
        }))
    })
    .unwrap_or_default()
}

fn kind_icon(kind: CollectionKind) -> &'static str {
    match kind {
        CollectionKind::Mixtape => "cassette",
        CollectionKind::ArtistCollection => "user",
        CollectionKind::Collection => "library-big",
    }
}

fn kind_str(kind: CollectionKind) -> &'static str {
    match kind {
        CollectionKind::Mixtape => "mixtape",
        CollectionKind::Collection => "collection",
        CollectionKind::ArtistCollection => "artist_collection",
    }
}

/// Rebuild the visible rows from the cache, honouring the search filter
/// (`myqbz_add.rs:239`).
fn rebuild() {
    let cache = ROWS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    with_doc(|doc| {
        let query = doc.search.trim().to_lowercase();
        doc.rows = cache
            .iter()
            .filter(|r| query.is_empty() || r.name.to_lowercase().contains(&query))
            .map(|r| AddRow {
                id: r.id.clone(),
                name: r.name.clone(),
                kind: kind_str(r.kind).to_string(),
                icon: kind_icon(r.kind).to_string(),
                kind_label: crate::myqbz_qt::label_for(r.kind),
                meta: crate::myqbz_qt::album_count_label(r.item_count),
                already_has: r.already_has,
            })
            .collect();
    });
}

// ---------------------------------------------------------------------------
// Open / close
// ---------------------------------------------------------------------------

/// Open the picker for a JSON **array** of [`AddItem`]s — the QML entry point
/// (`QbzMyQbzAdd.open(itemsJson)`). A malformed payload is logged and ignored.
pub(crate) fn open(items_json: &str) {
    match serde_json::from_str::<Vec<AddItem>>(items_json) {
        Ok(items) => open_items(items),
        Err(e) => log::warn!("[qbz-qt] myqbz_add: bad items payload ({e}): {items_json}"),
    }
}

/// Open the picker for one or more items. Empty input is a no-op (1:1 with
/// `openAddToMixtape([])`). Stores the payload, computes the header strings and
/// the R1 answer, marks loading, then loads the rows on a worker.
///
/// **It does not always open.** When [`payload_accepts`] answers "nothing"
/// — an ephemeral row (R3), a local playlist — the picker stays shut and the
/// refusal goes to the log only (R2): an empty picker is a visible block, and
/// a visible block is the thing being removed. The right fix for a caller that
/// trips this is upstream — do not offer the action for that row.
pub(crate) fn open_items(items: Vec<AddItem>) {
    if items.is_empty() {
        return;
    }

    let bulk = items.len() > 1;
    let accepts = payload_accepts(&items);
    if !accepts.any() {
        log::warn!(
            "[qbz-qt] myqbz_add: no container accepts this payload, picker not opened \
             ({} item(s), first = {}/{} id {}) — the caller should not be offering \
             \"Add to Mixtape/Collection\" for it",
            items.len(),
            items[0].item_type,
            items[0].source,
            items[0].source_item_id
        );
        return;
    }

    let first_title = items[0].title.clone();
    let first_subtitle = items[0].subtitle.clone().unwrap_or_default();
    let header_title = if bulk {
        qbz_i18n::tf(
            "{} item",
            "{} items",
            items.len() as i64,
            &[&items.len().to_string()],
        )
    } else {
        first_title.clone()
    };
    let header_subtitle = if bulk {
        let more = (items.len() - 1).to_string();
        qbz_i18n::t_args("{} + {} more", &[&first_title, &more])
    } else {
        first_subtitle
    };

    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = items;
    *ROWS_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Vec::new();

    with_doc(|doc| {
        doc.rows = Vec::new();
        doc.header_title = header_title;
        doc.header_subtitle = header_subtitle;
        doc.bulk_mode = bulk;
        doc.allow_mixtape = accepts.mixtape;
        doc.allow_collection = accepts.collection;
        doc.search = String::new();
        doc.busy_id = String::new();
        doc.creating = false;
        // Never preset the panel to a kind this payload cannot create;
        // `accepts.any()` above guarantees the fallback exists.
        doc.create_kind = accepts.create_fallback().unwrap_or("mixtape").to_string();
        doc.create_busy = false;
        doc.loading = true;
        doc.open = true;
    });
    publish();

    let items = pending_snapshot();
    crate::spawn(async move {
        let rows = tokio::task::spawn_blocking(move || load_rows(accepts, &items))
            .await
            .unwrap_or_default();
        *ROWS_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = rows;
        rebuild();
        with_doc(|doc| doc.loading = false);
        publish();
    });
}

/// Close the picker and clear the pending payload.
pub(crate) fn close() {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).clear();
    with_doc(|doc| {
        doc.open = false;
        doc.creating = false;
        doc.create_busy = false;
        doc.busy_id = String::new();
    });
    publish();
}

/// Drop every per-user trace on logout — the pending batch (PENDING), the
/// loaded picker rows (ROWS_CACHE) and the modal document (DOC) — then publish
/// the cleared document so a picker left open cannot survive the user switch.
///
/// The Slint reference has NO MyQBZ teardown at all (`qbz/src/auth.rs:336-356`
/// tears down offline/fav_cache/reco/discover_prefs/blacklist/pinned/
/// local_favorites/search/lyrics and nothing MyQBZ), so this is not a parity
/// item — it is this port's own rule, already applied to the five siblings
/// called from `auth_qt::logout`. What it prevents is concrete: ROWS_CACHE
/// holds the previous account's collection ids and names, so the next account
/// would open the picker on a list of collections it does not own and `pick()`
/// would `add_item_with` against a foreign `collection_id`; PENDING would hand
/// that write the previous account's items.
pub(crate) fn teardown() {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).clear();
    ROWS_CACHE.lock().unwrap_or_else(|e| e.into_inner()).clear();
    with_doc(|doc| *doc = AddDoc::default());
    publish();
}

/// Re-filter the loaded rows client-side (no refetch).
pub(crate) fn search(query: &str) {
    with_doc(|doc| doc.search = query.to_string());
    rebuild();
    publish();
}

/// Open the create sub-panel preset to a kind ("mixtape" | "collection").
///
/// R1-clamped: a kind the pending payload cannot go into is replaced by one it
/// can, and if it can go nowhere the panel does not open. The chip that would
/// ask for a forbidden kind is not rendered in the first place — this is the
/// safety net behind it, and like every other one here it is log-only.
pub(crate) fn show_create(kind: &str) {
    let accepts = pending_accepts();
    let wanted = if kind == "collection" {
        "collection"
    } else {
        "mixtape"
    };
    let resolved = if accepts.allows_create(wanted) {
        Some(wanted)
    } else {
        accepts.create_fallback()
    };
    let Some(resolved) = resolved else {
        log::warn!(
            "[qbz-qt] myqbz_add: create '{kind}' refused — nothing is creatable for this payload"
        );
        return;
    };
    if resolved != wanted {
        log::warn!("[qbz-qt] myqbz_add: create '{wanted}' is not offered for this payload, using '{resolved}'");
    }
    with_doc(|doc| {
        doc.create_kind = resolved.to_string();
        doc.create_busy = false;
        doc.creating = true;
    });
    publish();
}

/// Back to the picker list.
pub(crate) fn create_back() {
    with_doc(|doc| doc.creating = false);
    publish();
}

// ---------------------------------------------------------------------------
// Add / create
// ---------------------------------------------------------------------------

/// How many rows were inserted and how many were skipped as duplicates.
pub struct AddOutcome {
    pub added: usize,
    pub skipped: usize,
}

/// Insert every pending item into `collection_id` with
/// `allow_duplicate = false`, ONE open connection for the whole batch.
/// BLOCKING (DB). `Ok(false)` from the repo is a dedup rejection, not an error;
/// a real `Err` is logged and the batch continues.
pub(crate) fn add_items(collection_id: &str, items: &[AddItem]) -> AddOutcome {
    let mut added = 0usize;
    let mut skipped = 0usize;
    let _ = crate::library_db_qt::with_db(true, |db| {
        Ok(db.with_connection(|conn| {
            for it in items {
                match qbz_mixtape::repo::add_item_with(
                    conn,
                    collection_id,
                    item_type_from_str(&it.item_type),
                    source_from_str(&it.source),
                    &it.source_item_id,
                    &it.title,
                    it.subtitle.as_deref(),
                    it.artwork_url.as_deref(),
                    it.year,
                    it.track_count,
                    false,
                ) {
                    Ok(true) => added += 1,
                    Ok(false) => skipped += 1,
                    Err(e) => log::warn!("[qbz-qt] myqbz_add add_item failed: {e}"),
                }
            }
        }))
    });
    AddOutcome { added, skipped }
}

/// The add outcome as a toast (`myqbz_add.rs:313`). Three msgids are in play
/// and the "some skipped" arm NESTS one inside the other — it is not one flat
/// sentence.
fn toast_outcome(name: &str, outcome: &AddOutcome) {
    if outcome.added == 0 {
        // Nothing inserted -> everything was a duplicate. This Info toast is
        // the ONLY signal the user gets that the add did nothing.
        crate::toast_qt::info(qbz_i18n::t_args("Already in {}", &[name]));
        return;
    }
    let msg = if outcome.skipped > 0 {
        let skipped_label = qbz_i18n::tf(
            "{} duplicate skipped",
            "{} duplicates skipped",
            outcome.skipped as i64,
            &[&outcome.skipped.to_string()],
        );
        qbz_i18n::t_args(
            "Added {} to {} ({})",
            &[&outcome.added.to_string(), name, &skipped_label],
        )
    } else {
        qbz_i18n::t_args("Added {} to {}", &[&outcome.added.to_string(), name])
    };
    crate::toast_qt::success(msg);
}

/// The chosen collection's display name, for the toast. `None` when the id is
/// not one of the rows this picker loaded.
fn row_name(collection_id: &str) -> Option<String> {
    ROWS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|r| r.id == collection_id)
        .map(|r| r.name.clone())
}

/// Pick a target: add every pending item, toast the outcome, close.
/// Re-entrant clicks while a write is in flight are ignored.
///
/// The id MUST be one of the loaded rows, and those are already R1-filtered by
/// [`load_rows`] — so this one lookup is also the R2 net on the write path: an
/// invokable called with any other collection id (a stale QML frame, a row that
/// was filtered out) writes nothing. Log-only, per R2.
pub(crate) fn pick(collection_id: &str) {
    let Some(name) = row_name(collection_id) else {
        log::warn!(
            "[qbz-qt] myqbz_add: pick('{collection_id}') is not an offered row for this \
             payload, ignored"
        );
        return;
    };

    let accepted = with_doc(|doc| {
        if !doc.busy_id.is_empty() {
            return false;
        }
        doc.busy_id = collection_id.to_string();
        true
    });
    if !accepted {
        return;
    }
    publish();

    let items = pending_snapshot();
    let cid = collection_id.to_string();
    crate::spawn(async move {
        let outcome = tokio::task::spawn_blocking(move || add_items(&cid, &items))
            .await
            .unwrap_or(AddOutcome {
                added: 0,
                skipped: 0,
            });
        toast_outcome(&name, &outcome);
        close();
        // `nav_qt::back()` never re-dispatches a load, so a landed mutation
        // republishes both grids or the card's item count stays stale
        // (spec 02 §7 T4).
        if outcome.added > 0 {
            crate::myqbz_qt::reload_grids();
        }
    });
}

/// Create a new manual collection of `kind` named `name`, returning
/// `(id, name)`. BLOCKING (DB). `"collection"` maps to `Collection`, anything
/// else to `Mixtape` — `artist_collection` is never creatable here.
pub(crate) fn create_collection(kind: &str, name: &str) -> Option<(String, String)> {
    let kind = match kind {
        "collection" => CollectionKind::Collection,
        _ => CollectionKind::Mixtape,
    };
    crate::myqbz_qt::create_collection(kind, name).map(|c| (c.id, c.name))
}

/// Create then add: on success toast the outcome and close; on failure the
/// create panel STAYS open with `createBusy = false` and an error toast — the
/// raw English literal the reference uses (`main.rs:6009`, absent from all
/// eight catalogs, kept 1:1).
pub(crate) fn create_and_add(kind: &str, name: &str) {
    // R1 net on the create path: the radio for a forbidden kind is not
    // rendered, so reaching here means a stale frame or a direct invokable
    // call. Log-only (R2) — never a toast.
    if !pending_accepts().allows_create(kind) {
        log::warn!(
            "[qbz-qt] myqbz_add: createAndAdd('{kind}') refused — this payload cannot go \
             into a {kind}"
        );
        with_doc(|doc| doc.create_busy = false);
        publish();
        return;
    }

    let trimmed = name.trim().to_string();
    let accepted = with_doc(|doc| {
        if trimmed.is_empty() || doc.create_busy {
            return false;
        }
        doc.create_busy = true;
        true
    });
    if !accepted {
        return;
    }
    publish();

    let kind = kind.to_string();
    let items = pending_snapshot();
    crate::spawn(async move {
        let created = tokio::task::spawn_blocking(move || create_collection(&kind, &trimmed))
            .await
            .ok()
            .flatten();
        match created {
            Some((cid, cname)) => {
                let outcome = tokio::task::spawn_blocking(move || add_items(&cid, &items))
                    .await
                    .unwrap_or(AddOutcome {
                        added: 0,
                        skipped: 0,
                    });
                toast_outcome(&cname, &outcome);
                close();
                crate::myqbz_qt::reload_grids();
            }
            None => {
                with_doc(|doc| doc.create_busy = false);
                publish();
                crate::toast_qt::error("Failed to create".to_string());
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Payload builders for the Rust-side seams
// ---------------------------------------------------------------------------

/// `track` payloads from a batch of LocalTracks (source "local" — Plex rows
/// included, since there is no "plex" source). Subtitle = "artist · album"; no
/// artwork / year / track count (`myqbz_add.rs:348`). Used by the Local
/// Library bulk-bar and folder-rail seams, which reach Rust rather than QML.
pub(crate) fn track_items_from_local(tracks: &[qbz_library::LocalTrack]) -> Vec<AddItem> {
    tracks
        .iter()
        .map(|t| {
            let subtitle = [t.artist.clone(), t.album.clone()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            AddItem {
                item_type: "track".into(),
                source: "local".into(),
                source_item_id: t.id.to_string(),
                title: t.title.clone(),
                subtitle: (!subtitle.is_empty()).then_some(subtitle),
                artwork_url: None,
                year: None,
                track_count: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_accepts_the_minimal_object_and_ignores_extras() {
        let items: Vec<AddItem> = serde_json::from_str(
            r#"[{"itemType":"album","source":"qobuz","sourceItemId":"123","title":"X",
                 "somethingElse":true}]"#,
        )
        .expect("minimal payload deserializes");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, "album");
        assert!(items[0].subtitle.is_none());
        assert!(items[0].year.is_none());
        assert!(items[0].track_count.is_none());
    }

    #[test]
    fn payload_reads_the_camelcase_optionals() {
        let items: Vec<AddItem> = serde_json::from_str(
            r#"[{"itemType":"track","source":"local","sourceItemId":"7","title":"T",
                 "subtitle":"A · B","artworkUrl":"/x.jpg","year":1999,"trackCount":12}]"#,
        )
        .expect("full payload deserializes");
        let it = &items[0];
        assert_eq!(it.source_item_id, "7");
        assert_eq!(it.artwork_url.as_deref(), Some("/x.jpg"));
        assert_eq!(it.year, Some(1999));
        assert_eq!(it.track_count, Some(12));
        assert_eq!(source_from_str(&it.source), AlbumSource::Local);
        assert_eq!(item_type_from_str(&it.item_type), ItemType::Track);
    }

    #[test]
    fn source_and_item_type_default_to_qobuz_album() {
        assert_eq!(source_from_str("plex"), AlbumSource::Qobuz);
        assert_eq!(source_from_str("anything"), AlbumSource::Qobuz);
        assert_eq!(item_type_from_str("nonsense"), ItemType::Album);
        assert_eq!(item_type_from_str("playlist"), ItemType::Playlist);
    }

    fn add(item_type: &str, source: &str, id: &str) -> AddItem {
        AddItem {
            item_type: item_type.into(),
            source: source.into(),
            source_item_id: id.into(),
            title: "t".into(),
            subtitle: None,
            artwork_url: None,
            year: None,
            track_count: None,
        }
    }

    #[test]
    fn r1_albums_go_everywhere_from_every_source() {
        // "Collection: albums, singles, EPs. From ANY source — qobuz, local
        // (folders AND Plex AND anything added later)."
        for source in ["qobuz", "local"] {
            let a = payload_accepts(&[add("album", source, "12345")]);
            assert_eq!(a, Accepts::ALL, "album/{source} must reach every container");
        }
        // A Plex album key is a `local` album — the id shape changes nothing.
        assert_eq!(
            payload_accepts(&[add("album", "local", "plex:5677211365378243606")]),
            Accepts::ALL
        );
    }

    #[test]
    fn r1_tracks_are_mixtape_only() {
        for source in ["qobuz", "local"] {
            assert_eq!(
                payload_accepts(&[add("track", source, "42")]),
                Accepts::MIXTAPE_ONLY
            );
        }
    }

    #[test]
    fn r1_qobuz_playlists_are_mixtape_only_and_local_playlists_go_nowhere() {
        assert_eq!(
            payload_accepts(&[add("playlist", "qobuz", "998877")]),
            Accepts::MIXTAPE_ONLY
        );
        // R2: qbz-source rejects (Playlist, Local). Offering ANY container for
        // it would make that error reachable, so the picker never opens.
        let local_playlist = payload_accepts(&[add("playlist", "local", "whatever")]);
        assert_eq!(local_playlist, Accepts::NONE);
        assert!(!local_playlist.any());
    }

    #[test]
    fn r3_ephemeral_items_are_accepted_by_nothing() {
        let eph = (1i64 << 48) + 7;
        let a = payload_accepts(&[add("track", "local", &eph.to_string())]);
        assert_eq!(a, Accepts::NONE, "an ephemeral row leaves no trace");
        // And a caller that stamps a literal "qobuz" source onto whatever is
        // playing (PlayerBar.qml:559) must not slip past — the ID decides.
        assert_eq!(
            payload_accepts(&[add("track", "qobuz", &eph.to_string())]),
            Accepts::NONE
        );
        // An ephemeral ALBUM is still ephemeral, despite being album-shaped.
        assert_eq!(
            payload_accepts(&[add("album", "local", &eph.to_string())]),
            Accepts::NONE
        );
    }

    #[test]
    fn ids_below_the_ephemeral_floor_are_untouched() {
        // A Plex track id is namespaced at 2^40 — well under 2^48, so it stays
        // a normal local track.
        let plex = (1i64 << 40) | 4444;
        assert_eq!(
            payload_accepts(&[add("track", "local", &plex.to_string())]),
            Accepts::MIXTAPE_ONLY
        );
        // A non-numeric id is not an ephemeral id.
        assert_eq!(
            payload_accepts(&[add("album", "qobuz", "e94no900otyrz")]),
            Accepts::ALL
        );
    }

    #[test]
    fn a_batch_is_offered_only_what_every_item_accepts() {
        // Album + track -> mixtape only (the album's Collection slot loses).
        assert_eq!(
            payload_accepts(&[add("album", "qobuz", "1"), add("track", "qobuz", "2")]),
            Accepts::MIXTAPE_ONLY
        );
        // One local playlist poisons the batch — exactly the item that would
        // hard-error at resolve time.
        assert_eq!(
            payload_accepts(&[add("album", "qobuz", "1"), add("playlist", "local", "p")]),
            Accepts::NONE
        );
        // Empty is nothing, not everything.
        assert_eq!(payload_accepts(&[]), Accepts::NONE);
    }

    #[test]
    fn accepts_drives_rows_and_create_consistently() {
        let tracks = Accepts::MIXTAPE_ONLY;
        assert!(tracks.allows(CollectionKind::Mixtape));
        assert!(!tracks.allows(CollectionKind::Collection));
        assert!(
            !tracks.allows(CollectionKind::ArtistCollection),
            "an artist collection is a discography — albums only"
        );
        assert!(tracks.allows_create("mixtape"));
        assert!(!tracks.allows_create("collection"));
        assert_eq!(tracks.create_fallback(), Some("mixtape"));

        let albums = Accepts::ALL;
        assert!(albums.allows(CollectionKind::ArtistCollection));
        assert!(albums.allows_create("collection"));

        assert_eq!(Accepts::NONE.create_fallback(), None);
        // `allows_create` uses create_collection's own mapping: unknown = mixtape.
        assert!(albums.allows_create("nonsense"));
        assert!(!Accepts::NONE.allows_create("nonsense"));
    }

    #[test]
    fn kind_icons_match_the_reference() {
        assert_eq!(kind_icon(CollectionKind::Mixtape), "cassette");
        assert_eq!(kind_icon(CollectionKind::ArtistCollection), "user");
        assert_eq!(kind_icon(CollectionKind::Collection), "library-big");
    }
}
