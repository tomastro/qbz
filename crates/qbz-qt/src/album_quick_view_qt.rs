//! Navigation-free Album Quick View controller.
//!
//! `AlbumCard` exists on many independently recycled surfaces. Its preview
//! therefore cannot borrow the full `QbzAlbum.albumJson` document: that
//! document belongs to AlbumView, has progressive carousel enrichment, and
//! replacing it from a card would repaint an unrelated page. This controller
//! fetches `/album/get` for catalog ids or a read-only physical-version
//! snapshot for local/media-server ids, maps the compact header + track table,
//! and publishes through the dedicated `QbzAlbum.quickViewJson` property.
//!
//! Every open/close advances `GENERATION`. A response for card A can neither
//! overwrite card B nor reopen the modal after the user dismissed it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use cxx_qt_lib::QString;
use qbz_library::LocalTrack;
use qbz_models::{Album, Track};
use serde::Serialize;

static GENERATION: AtomicU64 = AtomicU64::new(0);
static LOCAL_CONTEXT: Mutex<Option<LocalContext>> = Mutex::new(None);

#[derive(Clone)]
struct LocalContext {
    generation: u64,
    tracks: Vec<LocalTrack>,
}

#[derive(Default, Serialize)]
struct QuickViewDoc {
    open: bool,
    loading: bool,
    failed: bool,
    #[serde(rename = "requestedId")]
    requested_id: String,
    #[serde(rename = "albumId")]
    album_id: String,
    title: String,
    artist: String,
    #[serde(rename = "artistId")]
    artist_id: String,
    #[serde(rename = "qualityTier")]
    quality_tier: String,
    #[serde(rename = "qualityDetail")]
    quality_detail: String,
    /// Physical source indicator for a local/media-server preview. Catalog
    /// previews leave it empty because their provenance is already implicit.
    source: String,
    /// Local/media-server previews use their physical-row playback path;
    /// catalog previews retain the Qobuz album context/actions.
    #[serde(rename = "isLocal")]
    is_local: bool,
    tracks: Vec<QuickTrack>,
}

#[derive(Serialize)]
struct QuickTrack {
    id: String,
    number: String,
    title: String,
    duration: String,
    /// Only an explicit Qobuz `streamable: false` turns this off; local and
    /// media-server reachability is checked later by the playback preflight.
    available: bool,
}

fn publish(doc: QuickViewDoc) {
    let json = serde_json::to_string(&doc).unwrap_or_else(|e| {
        log::warn!("[qbz-qt] album quick view serialization failed: {e}");
        "{}".to_string()
    });
    crate::album_bridge::ui(move |mut b| {
        b.as_mut().set_quick_view_json(QString::from(json.as_str()));
    });
}

fn publish_if_current(generation: u64, doc: QuickViewDoc) {
    if GENERATION.load(Ordering::SeqCst) == generation {
        publish(doc);
    }
}

fn loading_doc(album_id: &str) -> QuickViewDoc {
    QuickViewDoc {
        open: true,
        loading: true,
        requested_id: album_id.to_string(),
        album_id: album_id.to_string(),
        ..Default::default()
    }
}

fn failed_doc(album_id: String) -> QuickViewDoc {
    QuickViewDoc {
        open: true,
        failed: true,
        requested_id: album_id.clone(),
        album_id,
        ..Default::default()
    }
}

fn format_track_title(track: &Track) -> String {
    match track
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(version) => format!("{} ({version})", track.title),
        None => track.title.clone(),
    }
}

fn map_album(requested_id: String, album: Album) -> QuickViewDoc {
    // `/album/get` normally carries the flat pair. The nested pair is kept as
    // the first choice so this stays correct if the endpoint adopts the V2
    // album shape already used by Discover.
    let bit_depth = album
        .audio_info
        .as_ref()
        .and_then(|a| a.maximum_bit_depth)
        .or(album.maximum_bit_depth);
    let sample_rate = album
        .audio_info
        .as_ref()
        .and_then(|a| a.maximum_sampling_rate)
        .or(album.maximum_sampling_rate);
    let artist = album.artist.name.clone();
    let artist_id = if album.artist.id == 0 {
        String::new()
    } else {
        album.artist.id.to_string()
    };
    let tracks = album
        .tracks
        .as_ref()
        .map(|container| {
            container
                .items
                .iter()
                .enumerate()
                .map(|(index, track)| QuickTrack {
                    id: track.id.to_string(),
                    // Quick View is a condensed flat table. A continuous
                    // ordinal avoids duplicated 1..N labels on multi-disc
                    // releases while preserving the API's track order.
                    number: (index + 1).to_string(),
                    title: format_track_title(track),
                    duration: format!("{}:{:02}", track.duration / 60, track.duration % 60),
                    available: track.is_streamable(),
                })
                .collect()
        })
        .unwrap_or_default();

    QuickViewDoc {
        open: true,
        requested_id: requested_id.clone(),
        // Use the requested id as the action context. It is the entity the
        // card represented, even if a future API response aliases its id.
        album_id: requested_id,
        title: crate::album_qt::format_album_title(&album.title, album.version.as_deref()),
        artist,
        artist_id,
        quality_tier: crate::home_qt::quality_tier_from_depth(bit_depth).to_string(),
        quality_detail: crate::home_qt::quality_detail_from_parts(bit_depth, sample_rate),
        is_local: false,
        tracks,
        ..Default::default()
    }
}

fn local_quality_rank(track: &LocalTrack) -> (u8, u32, u64) {
    crate::local_rows::media_quality_rank(&track.format, track.bit_depth, track.sample_rate)
}

/// Build a preview from the best physical copy without calling
/// `local_album_actions::open_versions`: that function owns the routed Local
/// Album page's selection/cache, and a navigation-free card preview must not
/// retarget it behind the user's back.
fn map_local_album(
    requested_id: String,
    tracks: Vec<LocalTrack>,
) -> Option<(QuickViewDoc, Vec<LocalTrack>)> {
    let selected = crate::local_album_actions::split_versions(tracks)
        .into_iter()
        .next()?
        .1;
    let first = selected.first()?;
    let title = if first.album_group_title.trim().is_empty() {
        first.album.clone()
    } else {
        first.album_group_title.clone()
    };
    let lead_artist = first
        .album_artist
        .as_deref()
        .map(str::trim)
        .filter(|artist| !artist.is_empty())
        .unwrap_or(first.artist.trim())
        .to_string();
    let artist = if selected.iter().all(|track| {
        track
            .album_artist
            .as_deref()
            .map(str::trim)
            .filter(|artist| !artist.is_empty())
            .unwrap_or(track.artist.trim())
            == lead_artist.as_str()
    }) {
        lead_artist
    } else {
        qbz_i18n::t("Various Artists")
    };
    let best = selected
        .iter()
        .max_by_key(|track| local_quality_rank(track))
        .unwrap_or(first);
    // Keep the raw purchase spelling when it has its own source mark; every
    // other physical source uses the same folded vocabulary as SourceIcon.
    let raw_source = crate::local_rows::badge_source_raw(best.source.as_deref());
    let source = if raw_source.is_empty() {
        crate::local_rows::badge_source(best.source.as_deref())
    } else {
        raw_source
    };
    let rows = selected
        .iter()
        .enumerate()
        .map(|(index, track)| QuickTrack {
            id: track.id.to_string(),
            // The compact table is flat even for a box set, so its visible
            // ordinal stays continuous across disc boundaries.
            number: (index + 1).to_string(),
            title: track.title.clone(),
            duration: crate::local_rows::mmss(track.duration_secs),
            // Local/server reachability is preflighted by the playback seam;
            // the library row itself carries no catalog-rights withdrawal.
            available: true,
        })
        .collect();

    Some((
        QuickViewDoc {
            open: true,
            requested_id: requested_id.clone(),
            album_id: requested_id,
            title,
            artist,
            quality_tier: crate::local_rows::tier_of(
                &best.format,
                best.bit_depth,
                best.sample_rate,
            )
            .to_string(),
            quality_detail: crate::local_rows::detail_of(
                &best.format,
                best.bit_depth,
                best.sample_rate,
            ),
            source,
            is_local: true,
            tracks: rows,
            ..Default::default()
        },
        selected,
    ))
}

fn clear_local_context() {
    if let Ok(mut context) = LOCAL_CONTEXT.lock() {
        *context = None;
    }
}

fn store_local_context_if_current(generation: u64, tracks: Vec<LocalTrack>) -> bool {
    if GENERATION.load(Ordering::SeqCst) != generation {
        return false;
    }
    let Ok(mut context) = LOCAL_CONTEXT.lock() else {
        return false;
    };
    if GENERATION.load(Ordering::SeqCst) != generation {
        return false;
    }
    *context = Some(LocalContext { generation, tracks });
    true
}

/// Open immediately in loading state, then replace it with the compact album
/// document. This never records navigation and never touches AlbumView's
/// progressive document/stash.
pub(crate) fn open(album_id: String) {
    let album_id = album_id.trim().to_string();
    if album_id.is_empty() {
        return;
    }

    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    clear_local_context();
    publish(loading_doc(&album_id));

    if crate::library_qt::is_local_album_key(&album_id) {
        crate::spawn(async move {
            let requested_id = album_id.clone();
            let loaded = tokio::task::spawn_blocking(move || {
                let tracks = crate::local_albums::fetch_album_tracks_blocking(&requested_id);
                map_local_album(requested_id, tracks)
            })
            .await
            .ok()
            .flatten();
            match loaded {
                Some((doc, tracks)) => {
                    if store_local_context_if_current(generation, tracks) {
                        publish_if_current(generation, doc);
                    }
                }
                None => {
                    log::warn!("[qbz-qt] local album quick view load failed for {album_id}");
                    publish_if_current(generation, failed_doc(album_id));
                }
            }
        });
        return;
    }

    let runtime = crate::app();
    crate::spawn(async move {
        match runtime.core().get_album(&album_id).await {
            Ok(album) => publish_if_current(generation, map_album(album_id, album)),
            Err(error) => {
                log::warn!("[qbz-qt] album quick view load failed for {album_id}: {error}");
                publish_if_current(generation, failed_doc(album_id));
            }
        }
    });
}

/// Run an action over the exact local/media-server version published in the
/// preview. The generation check prevents a delayed click from acting on the
/// card that occupied the modal previously.
pub(crate) fn local_action(action: String, track_id: String) {
    let generation = GENERATION.load(Ordering::SeqCst);
    let context = LOCAL_CONTEXT.lock().ok().and_then(|context| {
        context
            .as_ref()
            .filter(|context| context.generation == generation)
            .cloned()
    });
    let Some(context) = context else {
        log::debug!("[qbz-qt] ignored stale local Quick View action '{action}'");
        return;
    };
    let requested_track = !track_id.trim().is_empty();
    let selected_id = track_id.trim().parse::<i64>().ok();
    let selected_position =
        selected_id.and_then(|id| context.tracks.iter().position(|track| track.id == id));
    if requested_track && selected_position.is_none() {
        log::debug!("[qbz-qt] ignored local Quick View action for unknown track '{track_id}'");
        return;
    }
    let start = selected_position.unwrap_or(0);
    let rows = if matches!(action.as_str(), "next" | "later" | "queue" | "playlist") {
        match selected_id {
            Some(id) => context
                .tracks
                .into_iter()
                .find(|track| track.id == id)
                .into_iter()
                .collect(),
            None => context.tracks,
        }
    } else {
        context.tracks
    };
    if rows.is_empty() {
        return;
    }

    // "playlist" never touches the queue: source-aware refs, then the
    // app-wide picker in LOCAL MODE (the local_bulk "add-to-playlist" tail).
    // Sync on purpose — the QML fires this BEFORE closeQuickView, and the
    // rows are already snapshotted above.
    if action == "playlist" {
        crate::local_album_actions::open_picker_for_rows(&rows);
        return;
    }

    let runtime = crate::app();
    crate::spawn(async move {
        match action.as_str() {
            "play" | "shuffle" => {
                crate::local_playback::play_rows(&runtime, rows, start, action == "shuffle").await;
            }
            "next" | "later" | "queue" => {
                crate::local_playback::enqueue_rows(&runtime, rows, action.clone()).await;
            }
            other => log::debug!("[qbz-qt] unknown local Quick View action '{other}'"),
        }
    });
}

/// Close and invalidate the request that may still be in flight.
pub(crate) fn close() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    clear_local_context();
    publish(QuickViewDoc::default());
}
