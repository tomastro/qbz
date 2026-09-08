//! Bulk actions over a selection of Qobuz catalog track ids — the shared
//! engine every track-listing multi-select bar calls (Playlist view, Artist
//! Popular Tracks, Label popular tracks; the album page keeps its own
//! album-ordered variant in album_qt.rs because it resolves against the
//! open album, not the catalog).
//!
//! The selection lives in QML; select-all/clear never reach Rust. Ids
//! arrive in VISIBLE order and stay in it (Slint's selection order rule).

use qbz_models::Track;

/// Resolve the selection against the catalog in ONE call, preserving the
/// selection's order (get_tracks_batch answers in arbitrary order).
async fn resolve(ids: &[String]) -> Vec<Track> {
    let raw: Vec<u64> = ids.iter().filter_map(|s| s.parse::<u64>().ok()).collect();
    if raw.is_empty() {
        return Vec::new();
    }
    let runtime = crate::app();
    let mut tracks = match runtime.core().get_tracks_batch(&raw).await {
        Ok(t) => t,
        Err(e) => {
            log::warn!("[qbz-qt] bulk tracks resolve failed: {e}");
            return Vec::new();
        }
    };
    let mut ordered: Vec<Track> = Vec::new();
    for id in &raw {
        if let Some(pos) = tracks.iter().position(|t| t.id == *id) {
            ordered.push(tracks.swap_remove(pos));
        }
    }
    ordered
}

/// One QueueTrack per catalog track (the field mapping of
/// `playback_qt::catalog_queue_track`, factored for batch use).
fn queue_track(track: &Track) -> qbz_models::QueueTrack {
    let (album_id, album_title, album_artwork) = match track.album.as_ref() {
        Some(album) => (
            album.id.clone(),
            album.title.clone(),
            // Queue row / np bar feed: full variant (best()) — the thumbnail
            // down-tier was reverted after the 2026-08-15 owner smoke (contract 04 §3).
            album.image.best().cloned().unwrap_or_default(),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    // Register the full variant set for the immersive large-art feed (D2).
    if let Some(album) = track.album.as_ref() {
        if !album.id.is_empty() {
            crate::artwork_qt::note_np_variants(&album.id, &album.image);
        }
    }
    let album_key = if album_id.is_empty() {
        None
    } else {
        Some(album_id)
    };
    qbz_models::QueueTrack {
        id: track.id,
        title: track.title.clone(),
        version: track.version.clone(),
        artist: track
            .performer
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_default(),
        album: album_title,
        album_version: None,
        duration_secs: track.duration as u64,
        artwork_url: if album_artwork.is_empty() {
            None
        } else {
            Some(album_artwork)
        },
        hires: track.hires,
        bit_depth: track.maximum_bit_depth,
        sample_rate: track.maximum_sampling_rate,
        is_local: false,
        album_id: album_key.clone(),
        artist_id: track.performer.as_ref().map(|p| p.id),
        // §3.1: absence is resolved in ONE place. `is_streamable()` reads a
        // missing key as AVAILABLE — only an explicit `false` marks a pull.
        streamable: track.is_streamable(),
        source: Some("qobuz".to_string()),
        parental_warning: track.parental_warning,
        source_item_id_hint: album_key,
        context_kind: None,
        context_id: None,
        isrc: track.isrc.clone(),
        recording_mbid: None,
    }
}

/// `context_kind`/`context_id` stamp the queue's play context ("playlist",
/// "artist", "label"); empty strings stamp nothing.
pub fn run(ids_json: String, action: String, context_kind: String, context_id: String) {
    let ids: Vec<String> = serde_json::from_str(&ids_json).unwrap_or_default();
    if ids.is_empty() {
        log::debug!("[qbz-qt] bulk tracks {action}: empty selection, ignored");
        return;
    }
    match action.as_str() {
        "queue" | "play-next" | "play-later" => {
            crate::spawn(async move {
                let tracks = resolve(&ids).await;
                if tracks.is_empty() {
                    return;
                }
                let mut queue: Vec<_> = tracks.iter().map(queue_track).collect();
                let mode = match action.as_str() {
                    "play-next" => "next",
                    "play-later" => "later",
                    _ => "queue",
                };
                // "next" inserts at the cursor — feed REVERSED so the block
                // keeps its order (playback_qt::enqueue_album's rule).
                if mode == "next" {
                    queue.reverse();
                }
                let queue = crate::playback_qt::stamped(
                    queue,
                    crate::playback_qt::PlayContext::new(&context_kind, &context_id),
                );
                if queue.is_empty() {
                    return;
                }
                let runtime = crate::app();
                if let Err(e) =
                    crate::playback_qt::enqueue_track_list_mode(&runtime, queue, mode).await
                {
                    log::error!("[qbz-qt] bulk tracks {mode} failed: {e}");
                }
            });
        }
        "add-to-playlist" => {
            crate::playlist_picker_qt::open_for_ids(&crate::app(), ids);
        }
        "add-to-mixtape" => {
            crate::spawn(async move {
                let items: Vec<_> = resolve(&ids)
                    .await
                    .iter()
                    .map(|t| crate::myqbz_add_qt::AddItem {
                        item_type: "track".into(),
                        source: "qobuz".into(),
                        source_item_id: t.id.to_string(),
                        title: t.title.clone(),
                        subtitle: t.performer.as_ref().map(|p| p.name.clone()),
                        // `best()` — see the note in album_qt.rs: thumbnail and
                        // small are both nullable, and the stored artwork_url is a
                        // snapshot the mosaics never re-derive.
                        artwork_url: t.album.as_ref().and_then(|a| a.image.best().cloned()),
                        year: None,
                        track_count: None,
                    })
                    .collect();
                if !items.is_empty() {
                    crate::myqbz_add_qt::open_items(items);
                }
            });
        }
        "add-to-favorites" => {
            crate::spawn(async move {
                let runtime = crate::app();
                for id in &ids {
                    if let Err(e) = runtime.core().add_favorite("track", id).await {
                        log::error!("[qbz-qt] bulk favorite {id} failed: {e}");
                    }
                    crate::fav_cache_qt::set("track", id, true);
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
                crate::toast_qt::success(qbz_i18n::t("Added to Library"));
            });
        }
        "make-offline" => {
            crate::spawn(async move {
                let tracks = resolve(&ids).await;
                if !tracks.is_empty() {
                    crate::offline_cache_qt::cache_tracks(tracks);
                }
            });
        }
        other => log::warn!("[qbz-qt] bulk tracks: unknown action {other}"),
    }
}
