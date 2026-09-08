//! My QBZ — collection / mixtape detail PLAYBACK. The Qt port of
//! `crates/qbz/src/myqbz_play.rs` (spec 02 §6.3).
//!
//! Wires the hero Play / Shuffle CTAs, the per-row Play + context menu
//! (play / play-next / play-later / add-to-queue), the select-mode bulk
//! enqueues and the expanded-mode inline-track actions to the SHARED
//! `qbz-mixtape` enqueue resolver, then drives the queue through this port's
//! stamping seam (`playback_qt::set_queue_stamped` / `stamped`).
//!
//! RESOLUTION GOES THROUGH `qbz-source` (design 02 §9 stage 2). Every path
//! here builds ONE `qbz_source::RegistryResolver` over the process-wide
//! registry instead of a per-call `ProdItemResolver { qobuz client, local
//! closure }`. Three things follow, and all three are the point:
//! - the hand-rolled `resolve_local` closure is GONE, and with it the
//!   `library_db_qt::with_db` open it performed once per item (23-34 opens for
//!   one collection); `LocalSource` owns one pooled handle.
//! - NO path is gated on a Qobuz client any more. A collection's local / Plex
//!   items RESOLVE — quality, artwork, tracks, and into the queue — offline,
//!   logged out or during an API hiccup, which they did not before.
//! - a Qobuz item with no client now fails PER ITEM (logged + skipped by
//!   `resolve_collection_tracks`) instead of emptying the whole resolve.
//!
//! No source is branched on by hand here: `SourceRegistry::claim` is the one
//! normalisation point, and the source of a row always comes from the ROW.
//!
//! CLOSED by stage 3, and it is why no `Play` arm below grew a source branch:
//! every arm still starts its first track through
//! `playback_qt::play_resolved_offline_aware`, but that funnel now CLAIMS the
//! row before it does anything else and hands a non-Qobuz one to `audible_qt`.
//! A local, ephemeral or Plex first track plays. It used to resolve here and
//! then fail AT THE PLAYER with "No Qobuz client available" — an error about a
//! service that has nothing to do with the track.
//!
//! That also removed the sharp edge the funnel arrived with (2026-08-17, #638
//! fix 3): it consulted the OFFLINE cache before the network, keyed on the raw
//! `u64`, and a `local_tracks` rowid or a Plex rating_key lives in the same
//! small-integer space as older Qobuz ids — a collision played THE WRONG TRACK
//! instead of failing cleanly. A claimed non-Qobuz row never reaches that
//! lookup now, so the id-space overlap is gone rather than guarded.
//!
//! Contract, all load-bearing and 1:1 with the reference:
//! - "Resolve all" uses the collection's persisted `play_mode`; the hero
//!   Shuffle forces `AlbumShuffle`.
//! - Failed items are logged + SKIPPED (partial playback > total failure) —
//!   `resolve_collection_tracks`' own contract; the per-item paths mirror it.
//! - `play_next` inserts in REVERSE so the first resolved track lands
//!   immediately after the current one (T12).
//! - The queue-source-collection stamp and `touch_play` happen ONLY on the
//!   whole-collection REPLACE paths — hero Play, hero Shuffle, DJ-mix confirm
//!   (T13/T14). Per-row replace, play-next, play-later, append and every bulk
//!   mode preserve the existing context.
//! - `PlayContext` is `None`: `playback_qt::derive_context` infers
//!   album-or-artist, which is byte-equivalent to the reference's unstamped
//!   `set_queue` + `refresh_now_playing` album fallback (T16). There is no
//!   "mixtape" context kind and inventing one would be a redesign.
//!
//! THE AUDIO BACKEND IS PROTECTED: everything here goes through
//! `crate::playback_qt` as-is. Nothing in this file touches sample rate,
//! resampling or device selection.
//!
//! `skip_to_next_item` / `skip_to_previous_item` are deliberately NOT ported —
//! they are `#[allow(dead_code)]` in the reference with zero UI call sites
//! (spec 02 §1.1).

use std::sync::Arc;

use qbz_app::shell::AppRuntime;
use qbz_core::LoggingAdapter;
use qbz_mixtape::enqueue::{resolve_collection_tracks, ItemResolver};
use qbz_models::mixtape::{CollectionPlayMode, MixtapeCollection, MixtapeCollectionItem};
use qbz_models::QueueTrack;
use qbz_source::{RawRef, RegistryResolver, SourceId};

type Runtime = Arc<AppRuntime<LoggingAdapter>>;

/// Per-row context-menu mode. The alias spellings are the reference's
/// (`myqbz_play.rs:59-67`).
enum RowMode {
    /// Replace-play this single item. NO queue-source stamp, NO `touch_play`.
    Play,
    /// Insert immediately after the current track (REVERSE order).
    PlayNext,
    /// Insert at the END of the manual block (#442) — NATURAL order.
    PlayLater,
    /// Append at the end of the queue.
    AddToQueue,
}

impl RowMode {
    fn parse(action: &str) -> Option<Self> {
        match action {
            "play" => Some(Self::Play),
            "play-next" | "play_next" => Some(Self::PlayNext),
            "play-later" | "play_later" => Some(Self::PlayLater),
            "add-to-queue" | "add_to_queue" | "append" => Some(Self::AddToQueue),
            _ => None,
        }
    }
}

/// Bulk-bar queue placement (spec 12 §13.1 + #442).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BulkEnqueueMode {
    Append,
    Next,
    Later,
}

/// Inline-track menu mode. NOTE the extra aliases: the inline menu maps
/// `queue` / `add-to-queue` / `append` to **PlayLater**, unlike the row menu
/// (`myqbz_play.rs:666-675`).
enum InlineTrackMode {
    Play,
    PlayNext,
    PlayLater,
}

impl InlineTrackMode {
    fn parse(action: &str) -> Option<Self> {
        match action {
            "play" => Some(Self::Play),
            "play-next" | "play_next" => Some(Self::PlayNext),
            "play-later" | "play_later" | "queue" | "add-to-queue" | "append" => {
                Some(Self::PlayLater)
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Resolver construction
// ---------------------------------------------------------------------------

/// THE resolver for every path in this file.
///
/// It replaces `ProdItemResolver::new(&client, resolve_local)` at all five
/// former construction sites. Two functions died with them:
///
/// - `resolve_local` — the synchronous local closure, whose body was
///   `library_db_qt::with_db(false, |db| resolve_local_item(db, item))`. It ran
///   ONCE PER ITEM, and `with_db` was `LibraryDatabase::open` per call: 23-34
///   opens for one collection, 200 for a 200-item one. `LocalSource` pools the
///   handle, so the same sequential resolve loop (`enqueue.rs:50-68`) now
///   reuses ONE.
/// - `qobuz_client` — the per-call client snapshot, and the four `let-else`
///   guards it fed. `RegistryResolver` needs no client: `QobuzSource` holds the
///   shared one (published by the auth path) and only the QOBUZ arm needs it.
///
/// `&LibraryDatabase` still never crosses an `.await` — `LocalSource::tracks`
/// has no await in its body and the pool hands out no guard, so the T1
/// constraint is now enforced by the type system instead of by convention.
fn registry_resolver() -> RegistryResolver {
    RegistryResolver::new(qbz_source::registry())
}

/// Resolve a whole collection's items into a flat queue. `force_shuffle`
/// overrides the persisted mode with `AlbumShuffle` (the hero Shuffle CTA).
///
/// Items that fail are logged and SKIPPED by `resolve_collection_tracks`, so a
/// collection whose Qobuz items cannot resolve (offline, logged out) still
/// plays its local and Plex ones. `_runtime` is kept in the signature because
/// `myqbz_mix_qt::…` calls this and the registry is process-wide.
pub(crate) async fn resolve_collection(
    _runtime: &Runtime,
    collection: &MixtapeCollection,
    force_shuffle: bool,
) -> Vec<QueueTrack> {
    let play_mode = if force_shuffle {
        CollectionPlayMode::AlbumShuffle
    } else {
        collection.play_mode
    };
    let resolver = registry_resolver();
    resolve_collection_tracks(collection.items.clone(), play_mode, &resolver).await
}

/// Resolve a SINGLE item (per-row actions). This path bypasses
/// `resolve_collection_tracks`, so it must stamp `source_item_id_hint` INLINE —
/// missing it silently breaks item-boundary detection.
async fn resolve_single_item(item: &MixtapeCollectionItem) -> Vec<QueueTrack> {
    let resolver = registry_resolver();
    match resolver.resolve(item).await {
        Ok(mut tracks) => {
            let hint = item.source_item_id.clone();
            for track in &mut tracks {
                track.source_item_id_hint = Some(hint.clone());
            }
            tracks
        }
        Err(e) => {
            log::warn!(
                "[qbz-qt] myqbz_play: item {}/{} resolve failed: {}",
                item.source_item_id,
                item.title,
                e
            );
            Vec::new()
        }
    }
}

/// Resolve one item's tracks for DISPLAY (the expanded view-mode + the detail's
/// resolveItems pass). No queue mutation, NO hint stamping.
///
/// The offline check does NOT short-circuit anything: the resolve is attempted
/// and the check only picks the LOG LEVEL of the failure, because the offline
/// gate refuses the Qobuz arm by design (spec 02 §1.1).
///
/// THE CLIENT GATE IS GONE, and that is a behaviour change, not a cleanup. This
/// function used to return early with an empty vec when there was no Qobuz
/// client, so every LOCAL and PLEX row of a collection showed no quality, no
/// artwork and no tracks — and refused to play — whenever the session was
/// offline, pre-login or mid-hiccup. The local arm never needed that client;
/// only `QobuzSource` does, and it now reports its own absence per item.
pub(crate) async fn fetch_item_tracks(
    _runtime: &Runtime,
    item: &MixtapeCollectionItem,
) -> Vec<QueueTrack> {
    let resolver = registry_resolver();
    match resolver.resolve(item).await {
        Ok(tracks) => tracks,
        Err(e) => {
            if crate::offline_fwd::engine().is_offline() {
                log::debug!(
                    "[qbz-qt] myqbz_play: fetch_item_tracks {}/{} failed offline: {}",
                    item.source_item_id,
                    item.title,
                    e
                );
            } else {
                log::warn!(
                    "[qbz-qt] myqbz_play: fetch_item_tracks {}/{} failed: {}",
                    item.source_item_id,
                    item.title,
                    e
                );
            }
            Vec::new()
        }
    }
}

/// Resolve the selected items' QOBUZ track ids for the bulk "Add to playlist"
/// flow: Qobuz playlists only accept Qobuz track ids, so local/Plex tracks are
/// skipped. Ids in resolution order.
pub(crate) async fn resolve_bulk_qobuz_track_ids(
    _runtime: &Runtime,
    items: &[MixtapeCollectionItem],
) -> Vec<String> {
    let resolver = registry_resolver();
    let registry = qbz_source::registry();

    let mut ids: Vec<String> = Vec::new();
    for item in items {
        match resolver.resolve(item).await {
            Ok(tracks) => {
                for t in tracks {
                    // The row decides. This was a hand-written
                    // `source == "qobuz" || (source.is_none() && !is_local)`
                    // — the one correct rule for an unlabelled queue row, and
                    // now the rule `QobuzSource::claim` applies, in ONE place,
                    // for every source word (`qobuz_download` and `offline`
                    // are local FILES and are correctly excluded here).
                    let is_qobuz = registry
                        .claim(&RawRef::from_queue_track(&t))
                        .map(|m| m.source() == SourceId::QOBUZ)
                        .unwrap_or(false);
                    if is_qobuz {
                        ids.push(t.id.to_string());
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "[qbz-qt] myqbz_play: bulk add-to-playlist resolve {}/{} failed: {}",
                    item.source_item_id,
                    item.title,
                    e
                );
            }
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

/// Load a collection (items hydrated) on a blocking worker.
pub(crate) async fn load_collection(collection_id: &str) -> Option<MixtapeCollection> {
    let id = collection_id.to_string();
    tokio::task::spawn_blocking(move || crate::myqbz_detail_qt::get_collection(&id))
        .await
        .ok()
        .flatten()
}

/// Best-effort `repo::touch_play` (bumps `last_played_at` + `play_count`).
/// Errors are ignored, exactly like the reference.
async fn touch_play(collection_id: &str) {
    let id = collection_id.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        crate::library_db_qt::with_db(true, |db| {
            Ok(db.with_connection(|conn| {
                if let Err(e) = qbz_mixtape::repo::touch_play(conn, &id) {
                    log::debug!("[qbz-qt] myqbz_play: touch_play({id}) failed: {e}");
                }
            }))
        })
    })
    .await;
}

// ---------------------------------------------------------------------------
// Whole-collection replace (hero Play / hero Shuffle / DJ-mix confirm)
// ---------------------------------------------------------------------------

/// Replace the queue with `tracks`, start at 0, STAMP the queue-source
/// collection and `touch_play`. The only paths that reach this are the three
/// whole-collection replaces (T13/T14). Empty `tracks` ⇒ error toast + no-op.
pub(crate) async fn play_all_tracks(
    runtime: &Runtime,
    collection_id: &str,
    tracks: Vec<QueueTrack>,
) {
    if tracks.is_empty() {
        crate::toast_qt::error(qbz_i18n::t("This collection resolved to 0 playable tracks"));
        return;
    }
    let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
        return;
    };
    // Context `None` — `derive_context` infers album-or-artist (T16).
    // F1: the anchor comes back from the seam, never from `tracks[0]`.
    let Some(anchor) = crate::playback_qt::set_queue_stamped(runtime, tracks, Some(0), None).await
    else {
        // The seam already toasted the count and left the previous queue alone.
        // `set_queue_source_collection` and `touch_play` below are bookkeeping
        // for a play that did not happen, so they are skipped with it.
        log::info!("[qbz-qt] myqbz_play: every track of {collection_id} was filtered");
        return;
    };
    let first_id = anchor.track_id;
    runtime
        .runtime()
        .set_queue_source_collection(Some(collection_id.to_string()))
        .await;
    // QConnect CONTROLLER mode (§7): route the play to the peer (after the
    // funnel, before the local audible step). NOT an early return here —
    // `touch_play` below is frontend-local recency bookkeeping that runs
    // regardless of where the audio plays (the Slint collection-play flow
    // records it on the routed path too).
    if !crate::playback_qt::route_play_remote(runtime, first_id, "play_all_tracks").await {
        if let Err(e) = crate::playback_qt::play_resolved_offline_aware(runtime, first_id, 0).await
        {
            log::error!("[qbz-qt] myqbz_play: play_track {first_id} failed: {e}");
        }
        crate::playback_qt::refresh_now_playing(runtime).await;
        crate::queue_qt::publish(runtime).await;
    }
    touch_play(collection_id).await;
}

/// Hero **Play**: resolve the whole collection with its persisted `play_mode`,
/// then replace-play.
pub(crate) fn play_all() {
    let collection_id = crate::myqbz_detail_qt::current_id();
    if collection_id.is_empty() {
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        let Some(collection) = load_collection(&collection_id).await else {
            crate::toast_qt::error(qbz_i18n::t("Couldn't load this collection"));
            return;
        };
        let tracks = resolve_collection(&runtime, &collection, false).await;
        play_all_tracks(&runtime, &collection_id, tracks).await;
    });
}

/// Hero **Shuffle**: forced `AlbumShuffle` ordering, then replace-play. SIDE
/// EFFECT (1:1): when the collection was not already `album_shuffle`, persist
/// it and reload the detail IF this collection is the one on screen, so the
/// overflow toggle label flips. A failed persist must NOT toast.
pub(crate) fn shuffle() {
    let collection_id = crate::myqbz_detail_qt::current_id();
    if collection_id.is_empty() {
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        let Some(collection) = load_collection(&collection_id).await else {
            crate::toast_qt::error(qbz_i18n::t("Couldn't load this collection"));
            return;
        };
        let already_shuffle = collection.play_mode == CollectionPlayMode::AlbumShuffle;
        let tracks = resolve_collection(&runtime, &collection, true).await;
        play_all_tracks(&runtime, &collection_id, tracks).await;

        if !already_shuffle {
            persist_album_shuffle(&collection_id).await;
        }
    });
}

/// Best-effort `set_play_mode(AlbumShuffle)`, then reload the open detail so
/// the overflow play-mode toggle label reflects the new mode. Failures are
/// logged, never surfaced — the shuffle already played.
async fn persist_album_shuffle(collection_id: &str) {
    let write_id = collection_id.to_string();
    let persisted = tokio::task::spawn_blocking(move || {
        crate::library_db_qt::with_db(true, |db| {
            db.with_connection(|conn| {
                qbz_mixtape::repo::set_play_mode(conn, &write_id, CollectionPlayMode::AlbumShuffle)
            })
            .map_err(|e| qbz_library::LibraryError::Database(format!("set_play_mode failed: {e}")))
        })
    })
    .await;
    match persisted {
        Ok(Some(())) => {}
        Ok(None) => {
            log::warn!(
                "[qbz-qt] myqbz_play: persist album_shuffle({collection_id}): db write failed"
            );
            return;
        }
        Err(e) => {
            log::warn!("[qbz-qt] myqbz_play: persist album_shuffle task panicked: {e}");
            return;
        }
    }
    crate::myqbz_detail_qt::reload_if_open(collection_id);
}

// ---------------------------------------------------------------------------
// Per-row actions
// ---------------------------------------------------------------------------

/// Per-row default **Play** — the context menu's `play` arm.
pub(crate) fn play_item(source_item_id: String) {
    item_action(source_item_id, "play".to_string());
}

/// Per-row context-menu action: play / play-next / play-later / add-to-queue
/// for the single item identified by `source_item_id`.
pub(crate) fn item_action(source_item_id: String, action: String) {
    let Some(mode) = RowMode::parse(&action) else {
        log::warn!("[qbz-qt] myqbz_play: unknown item action {action}");
        return;
    };
    let collection_id = crate::myqbz_detail_qt::current_id();
    if collection_id.is_empty() {
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        let Some(collection) = load_collection(&collection_id).await else {
            crate::toast_qt::error(qbz_i18n::t("Couldn't load this collection"));
            return;
        };
        let Some(item) = collection
            .items
            .iter()
            .find(|it| it.source_item_id == source_item_id)
            .cloned()
        else {
            log::warn!(
                "[qbz-qt] myqbz_play: item {source_item_id} not found in collection {collection_id}"
            );
            return;
        };

        let tracks = resolve_single_item(&item).await;
        if tracks.is_empty() {
            crate::toast_qt::error(qbz_i18n::t("This item resolved to 0 playable tracks"));
            return;
        }
        // Stamp ONCE, ahead of both the routed arm and the local arms: the
        // blacklist drop must apply to what reaches the PEER exactly as it
        // does to what lands locally (a blocked track never routes). The
        // Play arm's `set_queue_stamped` re-runs the same two passes, which
        // are idempotent (filter drops nothing twice; stamping skips
        // pre-stamped tracks).
        let tracks = crate::playback_qt::stamped(tracks, None);
        // The seam can empty a batch that was non-empty on entry. Guarding here
        // covers all four arms below — including the Play arm's `tracks[0]`,
        // which is a LIVE index panic today for a batch the blacklist empties.
        if tracks.is_empty() {
            log::info!("[qbz-qt] myqbz_play: the item's tracks were all filtered");
            return;
        }

        // QConnect CONTROLLER mode (contract §7): route the add to the peer's
        // queue — mode-matched to the row's LOCAL placement. Early-returns
        // when handled, so the local insert + sync tail below only run in
        // local/renderer mode. (Play replaces the queue instead — routed via
        // `route_play_to_peer` inside its own arm.)
        let routed_mode = match mode {
            RowMode::Play => None,
            RowMode::PlayNext => Some("next"),
            RowMode::PlayLater => Some("later"),
            RowMode::AddToQueue => Some("queue"),
        };
        if let Some(routed_mode) = routed_mode {
            if crate::playback_qt::route_enqueue_to_peer(&tracks, routed_mode).await {
                return;
            }
        }
        let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
            return;
        };
        let added_castable = crate::playback_qt::batch_all_qconnect_castable(&tracks);

        match mode {
            RowMode::Play => {
                // Replace-play this ONE item — no queue-source stamp, no
                // touch_play (per-row action, not "play the whole collection").
                // F1: the anchor comes back from the seam.
                let Some(anchor) =
                    crate::playback_qt::set_queue_stamped(&runtime, tracks, Some(0), None).await
                else {
                    log::info!("[qbz-qt] myqbz_play: the item's tracks were all filtered");
                    return;
                };
                let first_id = anchor.track_id;
                // QConnect CONTROLLER mode (§7): route the play to the peer
                // (after the funnel, before the local audible step).
                if crate::playback_qt::route_play_remote(&runtime, first_id, "item_action").await {
                    return;
                }
                if let Err(e) =
                    crate::playback_qt::play_resolved_offline_aware(&runtime, first_id, 0).await
                {
                    log::error!("[qbz-qt] myqbz_play: play_track {first_id} failed: {e}");
                }
                crate::playback_qt::refresh_now_playing(&runtime).await;
                crate::queue_qt::publish(&runtime).await;
            }
            RowMode::PlayNext => {
                // REVERSE so the first resolved track lands immediately after
                // the current one (T12).
                for track in tracks.into_iter().rev() {
                    runtime.core().add_track_next(track).await;
                }
                crate::playback_qt::sync_qconnect_after_add(added_castable).await;
                crate::queue_qt::publish(&runtime).await;
                crate::toast_qt::success(qbz_i18n::t("Playing next"));
            }
            RowMode::PlayLater => {
                // #442: NATURAL order — each insert appends to the END of the
                // manual block.
                for track in tracks {
                    runtime.core().add_track_later(track).await;
                }
                crate::playback_qt::sync_qconnect_after_add(added_castable).await;
                crate::queue_qt::publish(&runtime).await;
                crate::toast_qt::success(qbz_i18n::t("Added to play later"));
            }
            RowMode::AddToQueue => {
                runtime.core().add_tracks(tracks).await;
                crate::playback_qt::sync_qconnect_after_add(added_castable).await;
                crate::queue_qt::publish(&runtime).await;
                crate::toast_qt::success(qbz_i18n::t("Added to queue"));
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Bulk enqueue (select-mode bulk bar)
// ---------------------------------------------------------------------------

/// Bulk **Add to queue** / **Play next** for the current selection.
pub(crate) fn bulk_enqueue(play_next: bool) {
    let mode = if play_next {
        BulkEnqueueMode::Next
    } else {
        BulkEnqueueMode::Append
    };
    bulk_enqueue_inner(crate::myqbz_detail_qt::selected_full_items(), mode);
}

/// Bulk **Play later** (#442) for the current selection.
pub(crate) fn bulk_enqueue_later() {
    bulk_enqueue_inner(
        crate::myqbz_detail_qt::selected_full_items(),
        BulkEnqueueMode::Later,
    );
}

/// Resolve each selected item in SELECTION order (stamping the boundary hint
/// inline per item, since this path bypasses `resolve_collection_tracks`),
/// flatten, then place the batch per `mode`. NEVER replaces the queue and never
/// stamps the source collection. An all-empty batch toasts.
fn bulk_enqueue_inner(items: Vec<MixtapeCollectionItem>, mode: BulkEnqueueMode) {
    if items.is_empty() {
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        // No client gate: a selection of local / Plex items enqueues with no
        // Qobuz session at all, and a Qobuz item that cannot resolve is skipped
        // per item (below) instead of failing the whole batch.
        let resolver = registry_resolver();

        let mut tracks: Vec<QueueTrack> = Vec::new();
        for item in &items {
            match resolver.resolve(item).await {
                Ok(mut resolved) => {
                    let hint = item.source_item_id.clone();
                    for t in &mut resolved {
                        t.source_item_id_hint = Some(hint.clone());
                    }
                    tracks.extend(resolved);
                }
                Err(e) => {
                    log::warn!(
                        "[qbz-qt] myqbz_play: bulk item {}/{} resolve failed: {}",
                        item.source_item_id,
                        item.title,
                        e
                    );
                }
            }
        }

        if tracks.is_empty() {
            crate::toast_qt::error(qbz_i18n::t("These items resolved to 0 playable tracks"));
            return;
        }
        let tracks = crate::playback_qt::stamped(tracks, None);
        // The emptiness check above ran BEFORE the seam, so re-ask after it:
        // routing an empty batch to a peer and calling `add_tracks(vec![])`
        // would be a no-op dressed up as an action. The seam already said why.
        if tracks.is_empty() {
            log::info!("[qbz-qt] myqbz_play: every selected track was filtered");
            return;
        }

        // QConnect CONTROLLER mode (contract §7): route the add to the peer's
        // queue — mode-matched to the bulk bar's LOCAL placement (Next /
        // Later / Append). Early-returns when handled, so the local insert +
        // sync tail below only run in local/renderer mode.
        let routed_mode = match mode {
            BulkEnqueueMode::Next => "next",
            BulkEnqueueMode::Later => "later",
            BulkEnqueueMode::Append => "queue",
        };
        if crate::playback_qt::route_enqueue_to_peer(&tracks, routed_mode).await {
            return;
        }
        let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
            return;
        };
        let added_castable = crate::playback_qt::batch_all_qconnect_castable(&tracks);

        match mode {
            BulkEnqueueMode::Next => {
                for track in tracks.into_iter().rev() {
                    runtime.core().add_track_next(track).await;
                }
                crate::playback_qt::sync_qconnect_after_add(added_castable).await;
                crate::queue_qt::publish(&runtime).await;
                crate::toast_qt::success(qbz_i18n::t("Playing next"));
            }
            BulkEnqueueMode::Later => {
                for track in tracks {
                    runtime.core().add_track_later(track).await;
                }
                crate::playback_qt::sync_qconnect_after_add(added_castable).await;
                crate::queue_qt::publish(&runtime).await;
                crate::toast_qt::success(qbz_i18n::t("Added to play later"));
            }
            BulkEnqueueMode::Append => {
                runtime.core().add_tracks(tracks).await;
                crate::playback_qt::sync_qconnect_after_add(added_castable).await;
                crate::queue_qt::publish(&runtime).await;
                crate::toast_qt::success(qbz_i18n::t("Added to queue"));
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Inline-track actions (expanded view-mode)
// ---------------------------------------------------------------------------

/// Play / queue ONE inline track from an expanded item. The inline rows hold
/// only display text, so the PARENT item is re-resolved and the matching track
/// picked out.
///
/// DIVERGENCE, reproduced: `play-later` here appends via `add_tracks`
/// (`myqbz_play.rs:738`), NOT `add_track_later`, and toasts "Added to queue".
/// Keep it — the reference's inline menu and row menu genuinely differ.
///
/// `go-to-album` is handled here rather than in the bridge because this is the
/// single invokable the inline row calls for every menu entry
/// (`MyQbzDetailView.qml:628-630` sends it): the reference intercepts the same
/// action string before the playback modes (`qbz/src/main.rs:6790-6811`) and
/// opens the PARENT item with the parent's REAL type.
pub(crate) fn play_inline_track(item_source_item_id: String, track_id: String, action: String) {
    if action == "go-to-album" {
        // Route through the PARENT item, never the track's own ids, and with the
        // parent's real item_type — a playlist parent must reach the playlist
        // view, not be mis-routed to the album view. `open_item` ignores the
        // source argument, exactly as the reference's `invoke_open_item("", …)`
        // does.
        let parent_type = crate::myqbz_detail_qt::row_item_type(&item_source_item_id);
        crate::myqbz_detail_qt::open_item(String::new(), parent_type, item_source_item_id);
        return;
    }
    let Some(mode) = InlineTrackMode::parse(&action) else {
        log::warn!("[qbz-qt] myqbz_play: unknown inline-track action {action}");
        return;
    };
    let Ok(track_id) = track_id.parse::<u64>() else {
        log::warn!("[qbz-qt] myqbz_play: inline-track non-numeric id {track_id}");
        return;
    };
    let collection_id = crate::myqbz_detail_qt::current_id();
    if collection_id.is_empty() {
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        let Some(collection) = load_collection(&collection_id).await else {
            crate::toast_qt::error(qbz_i18n::t("Couldn't load this collection"));
            return;
        };
        let Some(item) = collection
            .items
            .iter()
            .find(|it| it.source_item_id == item_source_item_id)
            .cloned()
        else {
            log::warn!("[qbz-qt] myqbz_play: inline-track item {item_source_item_id} not found");
            return;
        };

        let tracks = fetch_item_tracks(&runtime, &item).await;
        let Some(track) = tracks.into_iter().find(|t| t.id == track_id) else {
            crate::toast_qt::error(qbz_i18n::t("This track is no longer available"));
            return;
        };
        let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
            return;
        };

        match mode {
            InlineTrackMode::Play => {
                // F1: the anchor comes back from the seam. No extra toast here —
                // the seam already raised the skipped-tracks one, and this
                // module's toast slot is single-shot (a second would replace it).
                let Some(anchor) =
                    crate::playback_qt::set_queue_stamped(&runtime, vec![track], Some(0), None)
                        .await
                else {
                    log::info!("[qbz-qt] myqbz_play: inline track {track_id} is not playable");
                    return;
                };
                let first_id = anchor.track_id;
                // QConnect CONTROLLER mode (§7): route the play to the peer
                // (after the funnel, before the local audible step).
                if crate::playback_qt::route_play_remote(&runtime, first_id, "play_inline_track")
                    .await
                {
                    return;
                }
                if let Err(e) =
                    crate::playback_qt::play_resolved_offline_aware(&runtime, first_id, 0).await
                {
                    log::error!("[qbz-qt] myqbz_play: play_track {first_id} failed: {e}");
                }
                crate::playback_qt::refresh_now_playing(&runtime).await;
                crate::queue_qt::publish(&runtime).await;
            }
            InlineTrackMode::PlayNext => {
                let Some(track) = crate::playback_qt::stamped(vec![track], None).pop() else {
                    return;
                };
                // QConnect CONTROLLER mode (§7): route the insert to the
                // peer's queue — early-returns when handled (stamped first: a
                // blocked track must not reach the peer when the local path
                // would drop it).
                if crate::playback_qt::route_track_to_peer(&track, "next").await {
                    return;
                }
                let added_castable =
                    crate::playback_qt::batch_all_qconnect_castable(std::slice::from_ref(&track));
                runtime.core().add_track_next(track).await;
                crate::playback_qt::sync_qconnect_after_add(added_castable).await;
                crate::queue_qt::publish(&runtime).await;
                crate::toast_qt::success(qbz_i18n::t("Playing next"));
            }
            InlineTrackMode::PlayLater => {
                let Some(track) = crate::playback_qt::stamped(vec![track], None).pop() else {
                    return;
                };
                // QConnect CONTROLLER mode (§7): route the add to the peer's
                // queue. Mode "queue", NOT "later": the local op here appends
                // via `add_tracks` (the documented inline/row divergence
                // above), so the route matches what the LOCAL path does.
                if crate::playback_qt::route_track_to_peer(&track, "queue").await {
                    return;
                }
                let added_castable =
                    crate::playback_qt::batch_all_qconnect_castable(std::slice::from_ref(&track));
                runtime.core().add_tracks(vec![track]).await;
                crate::playback_qt::sync_qconnect_after_add(added_castable).await;
                crate::queue_qt::publish(&runtime).await;
                crate::toast_qt::success(qbz_i18n::t("Added to queue"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_mode_accepts_both_spellings() {
        assert!(matches!(RowMode::parse("play"), Some(RowMode::Play)));
        assert!(matches!(
            RowMode::parse("play_next"),
            Some(RowMode::PlayNext)
        ));
        assert!(matches!(
            RowMode::parse("play-later"),
            Some(RowMode::PlayLater)
        ));
        assert!(matches!(
            RowMode::parse("append"),
            Some(RowMode::AddToQueue)
        ));
        assert!(RowMode::parse("nonsense").is_none());
    }

    #[test]
    fn inline_menu_maps_queue_to_play_later() {
        // The divergence from the row menu, pinned so a "cleanup" cannot
        // silently unify the two tables.
        assert!(matches!(
            InlineTrackMode::parse("add-to-queue"),
            Some(InlineTrackMode::PlayLater)
        ));
        assert!(matches!(
            InlineTrackMode::parse("queue"),
            Some(InlineTrackMode::PlayLater)
        ));
        assert!(matches!(
            RowMode::parse("add-to-queue"),
            Some(RowMode::AddToQueue)
        ));
        // `go-to-album` is NOT a playback mode: `play_inline_track` intercepts
        // it before the parse and routes to the parent item. If this ever starts
        // parsing, the nav arm has been made unreachable.
        assert!(InlineTrackMode::parse("go-to-album").is_none());
    }
}
