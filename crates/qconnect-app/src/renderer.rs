//! Frontend-agnostic renderer-side pure helpers (slice 6).
//!
//! Pure protocol/format math used by the renderer orchestration (queue
//! materialize / cursor-align). No engine, no I/O, no Tauri. Relocated here so
//! both the Tauri adapter and the Slint adapter share one definition; the
//! src-tauri side re-exports these. The load-dedup predicates and the
//! audio-quality report helpers move here alongside their orchestration /
//! report consumers in the later slice-6 steps.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use qbz_models::{QueueTrack, RepeatMode, Track};
use qbz_player::PlaybackState;
use tokio::sync::Mutex;

use crate::queue_resolution::{
    dedupe_track_ids, resolve_core_shuffle_order, resolve_remote_start_index,
};
use crate::renderer_engine::QconnectRendererEngine;
use crate::session::quality_from_max_audio_quality;
use crate::{QConnectQueueState, QConnectRendererState, QconnectRemoteSyncState, RendererCommand};

/// QConnect protocol `playing_state` wire values. Single source of truth for the
/// renderer orchestration; the Tauri adapter re-exports these from here.
pub const PLAYING_STATE_UNKNOWN: i32 = 0;
pub const PLAYING_STATE_STOPPED: i32 = 1;
pub const PLAYING_STATE_PLAYING: i32 = 2;
pub const PLAYING_STATE_PAUSED: i32 = 3;

/// Dedup window: an echoed SetState for a track whose load was registered within
/// this window does not re-trigger the load. The audio thread updates
/// `playback_state.track_id` only after the engine appends the source, so a bare
/// `track_id` comparison would re-fire during that buffer/decode gap.
const LOAD_ATTEMPT_DEDUP_WINDOW: Duration = Duration::from_secs(5);

/// Canonical source stamped on tracks materialized from a QConnect cloud queue.
///
/// The wire supplies Qobuz catalog ids. Keeping the old transport-only
/// `qobuz_connect_remote` tag in the core queue leaked QConnect lifecycle state
/// into downstream renderers: after a manual disconnect, Cast no longer chose
/// its Qobuz progressive-stream path. Materialized rows therefore use the same
/// portable provenance as every other Qobuz catalog row.
pub const QCONNECT_REMOTE_QUEUE_SOURCE: &str = "qobuz";

/// Backward-compatible spelling found in persisted queues and in queues
/// materialized by an older runtime before an in-process upgrade/reconnect.
pub const LEGACY_QCONNECT_REMOTE_QUEUE_SOURCE: &str = "qobuz_connect_remote";

/// Whether a queue row can be named by Qobuz Connect without looking the id up.
///
/// Source provenance is the authority: local-library and media-server ids can
/// overlap the numeric Qobuz id space, while offline Qobuz downloads retain the
/// real catalog id and remain resolvable. A missing source is accepted only by
/// the [`QueueTrack`] wrapper below, where the legacy `is_local` discriminator
/// is still available.
pub fn qconnect_source_is_resolvable(track_id: u64, source: Option<&str>) -> bool {
    if track_id == 0 {
        return false;
    }
    match source.map(str::trim).filter(|source| !source.is_empty()) {
        Some(source) => matches!(
            source.to_ascii_lowercase().as_str(),
            QCONNECT_REMOTE_QUEUE_SOURCE
                | "qobuz_download"
                | "qobuz_purchase"
                | "offline"
                | LEGACY_QCONNECT_REMOTE_QUEUE_SOURCE
        ),
        // Callers with only the wire-shaped `(id, source)` pair have no local
        // bit. Every current local producer stamps an explicit source, so the
        // source-less shape is the legacy catalog case.
        None => true,
    }
}

/// Queue-row form of [`qconnect_source_is_resolvable`]. This is the canonical
/// admission predicate for desktop queue filtering and daemon publication.
pub fn qconnect_queue_track_is_resolvable(track: &QueueTrack) -> bool {
    match track
        .source
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        Some(source) => qconnect_source_is_resolvable(track.id, Some(source)),
        None => track.id > 0 && !track.is_local,
    }
}

pub fn qconnect_repeat_mode_from_loop_mode(loop_mode: i32) -> Option<RepeatMode> {
    // QConnect protocol loop mode values: 1 = off, 2 = repeat one, 3 = repeat all.
    match loop_mode {
        0 | 1 => Some(RepeatMode::Off),
        2 => Some(RepeatMode::One),
        3 => Some(RepeatMode::All),
        _ => None,
    }
}

pub fn normalize_volume_to_fraction(volume: i32) -> f32 {
    volume.clamp(0, 100) as f32 / 100.0
}

pub fn model_track_to_core_queue_track(track: &Track) -> QueueTrack {
    let artwork_url = track
        .album
        .as_ref()
        .and_then(|album| album.image.best().cloned());
    let artist = track
        .performer
        .as_ref()
        .map(|performer| performer.name.clone())
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let album = track
        .album
        .as_ref()
        .map(|album| album.title.clone())
        .unwrap_or_else(|| "Unknown Album".to_string());
    let album_id = track.album.as_ref().and_then(|album| {
        let trimmed = album.id.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let artist_id = track.performer.as_ref().map(|performer| performer.id);

    QueueTrack {
        id: track.id,
        title: track.title.clone(),
        version: track.version.clone(),
        artist,
        album,
        album_version: None,
        duration_secs: track.duration as u64,
        artwork_url,
        hires: track.hires,
        bit_depth: track.maximum_bit_depth,
        sample_rate: track.maximum_sampling_rate,
        is_local: false,
        album_id: album_id.clone(),
        artist_id,
        // A queue pushed by a QConnect peer carries real Qobuz tracks, so the
        // flag is worth keeping — but resolved through `is_streamable()`, since
        // a peer's payload is one more endpoint we have never captured and a
        // terse one must not arrive here marked dead. Its source is canonical
        // Qobuz provenance, not a transport-lifecycle tag: this queue remains
        // playable by local and Cast renderers after QConnect disconnects.
        streamable: track.is_streamable(),
        source: Some(QCONNECT_REMOTE_QUEUE_SOURCE.to_string()),
        parental_warning: track.parental_warning,
        source_item_id_hint: album_id,
        context_kind: None,
        context_id: None,
        isrc: None,
        recording_mbid: None,
    }
}

/// Keep intrinsic metadata that was already present in the local queue when a
/// QConnect cloud echo materializes the same catalog track again.
///
/// The queue wire carries ids only. Hydrating those ids through the batch
/// track endpoint gives us enough to play, but that endpoint can omit the
/// track `version` and cannot express the album release `version` at all
/// (`Track::album` is only an `AlbumSummary`). Replacing an album-built queue
/// with that terse projection therefore stripped edition suffixes from every
/// downstream consumer: queue rows, Now Playing, MPRIS, lyrics and scrobbling.
///
/// Only missing intrinsic fields are filled, and only from a non-local track
/// with the same catalog id. The remote source tag, availability answer and
/// context remain authoritative; in particular, an unrelated local-library id
/// collision can never leak file metadata into a Qobuz queue.
fn preserve_existing_catalog_metadata(remote: &mut QueueTrack, existing: &QueueTrack) {
    if remote.id != existing.id || existing.is_local {
        return;
    }

    let missing = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    };
    if missing(&remote.version) && !missing(&existing.version) {
        remote.version = existing.version.clone();
    }
    if missing(&remote.album_version) && !missing(&existing.album_version) {
        remote.album_version = existing.album_version.clone();
    }
    if remote.title.trim().is_empty() && !existing.title.trim().is_empty() {
        remote.title = existing.title.clone();
    }
    if (remote.artist.trim().is_empty() || remote.artist == "Unknown Artist")
        && !existing.artist.trim().is_empty()
    {
        remote.artist = existing.artist.clone();
    }
    if (remote.album.trim().is_empty() || remote.album == "Unknown Album")
        && !existing.album.trim().is_empty()
    {
        remote.album = existing.album.clone();
    }
    if missing(&remote.artwork_url) && !missing(&existing.artwork_url) {
        remote.artwork_url = existing.artwork_url.clone();
    }
    if missing(&remote.album_id) && !missing(&existing.album_id) {
        remote.album_id = existing.album_id.clone();
    }
    if remote.artist_id.is_none() {
        remote.artist_id = existing.artist_id;
    }
    if remote.duration_secs == 0 {
        remote.duration_secs = existing.duration_secs;
    }
    if remote.bit_depth.is_none() {
        remote.bit_depth = existing.bit_depth;
    }
    if remote.sample_rate.is_none() {
        remote.sample_rate = existing.sample_rate;
    }
    remote.hires |= existing.hires;
}

// ===================== Renderer orchestration (slice 6, step 6) =====================
//
// Engine-agnostic: written ONLY against `QconnectRendererEngine` + the shared
// `QconnectRemoteSyncState`. The Tauri/Slint adapters obtain a concrete engine
// (`&CoreBridge` / `&SlintEngine`) — including any "not initialized yet" guard —
// and dispatch here, so the hard-won echo/cursor/materialize/shuffle logic is
// never re-derived per frontend. Ported byte-for-byte from the prior Tauri
// `corebridge.rs` / `track_loading.rs`; only `bridge` -> `engine` and the
// guard-unwrap (which stays adapter-side) changed.

pub fn queue_state_needs_materialization(
    previous: Option<&QConnectQueueState>,
    next: &QConnectQueueState,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };

    previous.version != next.version
        || previous.queue_items != next.queue_items
        || previous.shuffle_mode != next.shuffle_mode
        || previous.shuffle_order != next.shuffle_order
        || previous.autoplay_mode != next.autoplay_mode
        || previous.autoplay_loading != next.autoplay_loading
        || previous.autoplay_items != next.autoplay_items
}

pub fn should_reload_remote_track(playback_state: &PlaybackState, track_id: u64) -> bool {
    // Only reload when the track ID actually changed. The previous
    // !has_loaded_audio gate fired during the buffering window of an
    // initial load (qbz already started fetching but the audio engine
    // hasn't reported the track as loaded yet) — when the cloud echo
    // SetState arrived for the same track, this caused a redundant
    // load that interrupted the in-progress one. That was the residual
    // first-track hiccup.
    playback_state.track_id != track_id
}

/// Returns true if a load attempt for `track_id` was registered within the
/// dedup window (see `LOAD_ATTEMPT_DEDUP_WINDOW`).
fn is_recent_load_attempt(state: &QconnectRemoteSyncState, track_id: u64) -> bool {
    match state.last_load_attempt {
        Some((tid, ts)) => tid == track_id && ts.elapsed() < LOAD_ATTEMPT_DEDUP_WINDOW,
        None => false,
    }
}

/// Load a remote track into the engine, deduped against echoed SetState frames.
/// Records the attempt BEFORE dispatching the load (the audio thread updates
/// `playback_state.track_id` only after the engine appends the source, so the
/// recording must precede the load to close the echo window).
///
/// `start_position_secs` is the position to resume the stream at. For a normal
/// peer track-change the cloud sends position ~0, so this is 0 (a fresh track).
/// On a TAKEBACK whose first load lands here (SetActive arrived before the cloud
/// knew our current_track, so the force-stream couldn't fire), the SetState
/// carries the peer's real position and we resume there instead of streaming
/// from 0 and then trying to seek forward — a forward seek past the buffered
/// watermark is silently ignored by the audio thread, so streaming from 0 left
/// the first takeback playing from the start (bad for audiobooks). The protected
/// bit-perfect seams + the HTTP feeder live behind `start_track_stream`.
pub async fn ensure_remote_track_loaded(
    engine: &impl QconnectRendererEngine,
    sync_state: &Arc<Mutex<QconnectRemoteSyncState>>,
    track_id: u64,
    max_audio_quality: Option<i32>,
    start_position_secs: u64,
) -> Result<bool, String> {
    {
        let state = sync_state.lock().await;
        if is_recent_load_attempt(&state, track_id) {
            return Ok(false);
        }
    }
    let playback_state = engine.get_playback_state();
    if !should_reload_remote_track(&playback_state, track_id) {
        return Ok(false);
    }

    {
        let mut state = sync_state.lock().await;
        state.last_load_attempt = Some((track_id, Instant::now()));
    }

    let quality = quality_from_max_audio_quality(max_audio_quality);
    let duration_secs = engine
        .get_track(track_id)
        .await
        .map(|track| u64::from(track.duration))
        .unwrap_or(0);
    engine
        .start_track_stream(track_id, quality, duration_secs, start_position_secs)
        .await?;
    Ok(true)
}

/// Force a (re)stream of `track_id` at `start_position_secs` when BECOMING the
/// active renderer (takeback). Unlike [`ensure_remote_track_loaded`], this does
/// NOT short-circuit on a matching `playback_state.track_id`: a prior
/// controller->renderer handoff tore the local stream down via `engine.stop()`
/// (audio buffer cleared, `has_loaded_audio` false) while `current_track_id`
/// still reports the old track, so the plain track-id guard would skip the load
/// and the following `resume()` would fail with "no audio data available".
///
/// It DOES skip when the engine is already streaming this exact track with audio
/// loaded (`track_id` matches AND `has_loaded_audio`), so a spurious SetActive
/// during live renderer playback never restarts the current track; and it keeps
/// the dedup window so the SetActive->SetState echo doesn't double-load.
///
/// `start_position_secs` resumes at the handed-off position (the cloud carries
/// the peer's last position in `renderer_state.current_position_ms`), so a long
/// track / audiobook does not restart from 0. Resume is honored by the protected
/// `play_streaming_dynamic` session-resume path behind `start_track_stream`.
pub async fn force_remote_track_stream(
    engine: &impl QconnectRendererEngine,
    sync_state: &Arc<Mutex<QconnectRemoteSyncState>>,
    track_id: u64,
    max_audio_quality: Option<i32>,
    start_position_secs: u64,
) -> Result<bool, String> {
    let playback_state = engine.get_playback_state();
    if playback_state.track_id == track_id && engine.has_loaded_audio() {
        return Ok(false);
    }

    {
        let state = sync_state.lock().await;
        if is_recent_load_attempt(&state, track_id) {
            return Ok(false);
        }
    }
    {
        let mut state = sync_state.lock().await;
        state.last_load_attempt = Some((track_id, Instant::now()));
    }

    let quality = quality_from_max_audio_quality(max_audio_quality);
    let duration_secs = engine
        .get_track(track_id)
        .await
        .map(|track| u64::from(track.duration))
        .unwrap_or(0);
    engine
        .start_track_stream(track_id, quality, duration_secs, start_position_secs)
        .await?;
    Ok(true)
}

/// Resolve the track for a state-only PLAYING command on a cold engine.
/// Official clients can omit `current_track`; the synchronized queue cursor is
/// then the next authority. The id that `stop()` deliberately preserved is
/// accepted only when it still names a resolvable row in that queue; a bare
/// local-library id is never guessed to be Qobuz.
async fn takeback_track_id(
    engine: &impl QconnectRendererEngine,
    renderer_state: &QConnectRendererState,
) -> Option<u64> {
    if let Some(track_id) = renderer_state
        .current_track
        .as_ref()
        .map(|track| track.track_id)
        .filter(|track_id| *track_id > 0)
    {
        return Some(track_id);
    }

    let (tracks, current_index) = engine.get_all_queue_tracks().await;
    if let Some(track_id) = current_index
        .and_then(|index| tracks.get(index))
        .filter(|track| qconnect_queue_track_is_resolvable(track))
        .map(|track| track.id)
        .filter(|track_id| *track_id > 0)
    {
        return Some(track_id);
    }

    let track_id = engine.get_playback_state().track_id;
    tracks
        .iter()
        .find(|track| track.id == track_id && qconnect_queue_track_is_resolvable(track))
        .map(|track| track.id)
}

pub async fn apply_remote_loop_mode(
    engine: &impl QconnectRendererEngine,
    loop_mode: i32,
) -> Result<(), String> {
    let repeat_mode = qconnect_repeat_mode_from_loop_mode(loop_mode)
        .ok_or_else(|| format!("unsupported qconnect loop mode: {loop_mode}"))?;
    engine.set_repeat_mode(repeat_mode).await;
    Ok(())
}

pub async fn apply_renderer_command(
    engine: &impl QconnectRendererEngine,
    sync_state: &Arc<Mutex<QconnectRemoteSyncState>>,
    command: &RendererCommand,
    renderer_state: &QConnectRendererState,
) -> Result<(), String> {
    match command {
        RendererCommand::SetState {
            playing_state,
            current_position_ms,
            current_track,
            next_track,
            ..
        } => {
            let mut loaded_at_reported_position = false;
            let resolved_playing_state = renderer_state.playing_state.or(*playing_state);
            let mut projection_renderer_state = renderer_state.clone();
            if projection_renderer_state.current_track.is_none() {
                projection_renderer_state.current_track = current_track.clone();
            }
            if projection_renderer_state.next_track.is_none() {
                projection_renderer_state.next_track = next_track.clone();
            }
            let resolved_current_track = projection_renderer_state.current_track.as_ref();
            if let Some(projected_track) = resolved_current_track {
                let queue_state = {
                    let state = sync_state.lock().await;
                    state.last_remote_queue_state.clone()
                };
                let projection_applied = if let Some(queue_state) = queue_state.as_ref() {
                    sync_remote_shuffle_projection(
                        engine,
                        sync_state,
                        queue_state,
                        &projection_renderer_state,
                    )
                    .await?
                } else {
                    false
                };

                // Track-manipulation operations (cursor align, force-restart,
                // ensure_remote_track_loaded) only run when the COMMAND
                // explicitly specifies a current_track. The projection's
                // resolved_current_track can be stale: when the cloud sends
                // a state-only update (pause/resume) with command.current_track=null,
                // the projection falls back to renderer_state.current_track,
                // which is the cloud's last-known view of qbz's playback —
                // potentially behind qbz's actual local advance. Using that
                // stale value to align/load causes spurious track switches
                // (e.g., pause from iOS made qbz jump back to a previous
                // track). The outer renderer_state-based projection is still
                // used for shuffle sync above and downstream playing_state /
                // seek operations, which remain safe because they don't
                // change the queue cursor or load tracks.
                let _ = projected_track; // retained for shuffle projection above
                if let Some(command_track) = current_track.as_ref() {
                    if !projection_applied {
                        if let Err(err) = align_queue_cursor(engine, command_track.track_id).await {
                            log::warn!("[QConnect] Failed to align CoreBridge queue cursor: {err}");
                        }
                    }

                    if matches!(
                        resolved_playing_state,
                        Some(PLAYING_STATE_PLAYING | PLAYING_STATE_PAUSED)
                    ) {
                        // Force-restart removed: the cloud routinely re-emits
                        // SetState with current_position_ms=0 for the same
                        // track when only secondary fields change (e.g.,
                        // next_track corrections, queue_item_id refreshes).
                        // Reloading the stream on every echo caused first-
                        // track hiccup on album change and "needs several
                        // taps" on prev/next. Track-change cases are handled
                        // by align_queue_cursor + ensure_remote_track_loaded
                        // below; legitimate seek-to-start from a peer
                        // controller can use the seek path with target>1s.
                        // Resume the load at the cloud's reported position (same
                        // source the seek block below uses). For a normal peer
                        // track-change this is ~0; on a takeback whose first load
                        // lands here it is the peer's position, so we stream from
                        // there instead of from 0 + an ignored forward seek.
                        let start_position_secs = renderer_state
                            .current_position_ms
                            .or(*current_position_ms)
                            .map(|ms| ms / 1000)
                            .unwrap_or(0);
                        match ensure_remote_track_loaded(
                            engine,
                            sync_state,
                            command_track.track_id,
                            projection_renderer_state.max_audio_quality,
                            start_position_secs,
                        )
                        .await
                        {
                            Ok(loaded) => loaded_at_reported_position |= loaded,
                            Err(err) => log::warn!(
                                "[QConnect] Failed to load remote track {}: {err}",
                                command_track.track_id
                            ),
                        }
                    }
                }
            }

            if let Some(value) = resolved_playing_state {
                match value {
                    PLAYING_STATE_PLAYING => {
                        // A state-only resume (current_track = null, e.g. a
                        // mid-track handoff from a peer renderer after the
                        // engine restarted) can land on an engine whose queue
                        // cursor is set but which holds NO loaded audio — the
                        // session store restores the queue paused and
                        // unloaded. A bare resume() then dies in the audio
                        // thread ("cannot resume - no audio data available")
                        // while the cloud keeps reporting paused 0:00 to the
                        // controller forever. Cold-load the cloud's current
                        // track at its position first, exactly like the
                        // SetActive takeback path; the has_loaded_audio gate
                        // keeps echoes and live playback on the plain resume.
                        let cold_engine = !engine.has_loaded_audio();
                        if cold_engine {
                            let Some(track_id) =
                                takeback_track_id(engine, &projection_renderer_state).await
                            else {
                                return Err(
                                    "cold-start resume has no projected, queued, or preserved track"
                                        .to_string(),
                                );
                            };
                            let start_position_secs = renderer_state
                                .current_position_ms
                                .or(*current_position_ms)
                                .map(|ms| ms / 1000)
                                .unwrap_or_else(|| engine.get_playback_state().position);
                            match force_remote_track_stream(
                                engine,
                                sync_state,
                                track_id,
                                projection_renderer_state.max_audio_quality,
                                start_position_secs,
                            )
                            .await
                            {
                                Ok(loaded) => loaded_at_reported_position |= loaded,
                                Err(err) => log::warn!(
                                    "[QConnect] Cold-start load of remote track {track_id} failed: {err}"
                                ),
                            }
                        } else {
                            engine.resume()?;
                        }
                    }
                    PLAYING_STATE_PAUSED => {
                        engine.pause()?;
                    }
                    PLAYING_STATE_STOPPED => {
                        engine.stop()?;
                    }
                    PLAYING_STATE_UNKNOWN => {}
                    _ => {
                        log::debug!("[QConnect] Unknown playing state received: {value}");
                    }
                }
            }

            if let Some(position_ms) = renderer_state.current_position_ms.or(*current_position_ms) {
                let playback_state = engine.get_playback_state();
                let current_pos_secs = playback_state.position;
                let target_secs = position_ms / 1000;
                // Reject echo seeks: when the command targets the same track
                // qbz is already playing AND target<=1s while local is well
                // ahead, this is the cloud re-emitting a stale SetState
                // (frequently fires on next_track corrections and queue_
                // item_id refreshes). A real peer "go to start" intent
                // would target the same track as the local one but the
                // round-trip to qbz is already a few seconds, making this
                // case indistinguishable from echo — favor stability.
                let is_echo_reset = current_track
                    .as_ref()
                    .map(|cmd_track| cmd_track.track_id == playback_state.track_id)
                    .unwrap_or(false)
                    && target_secs <= 1
                    && current_pos_secs > 2;
                // Issue #387: honor seeks regardless of which device is the
                // active renderer. The previous gate (`peer_renderer_active`)
                // skipped seeks entirely when local was the active renderer,
                // breaking the case where a peer controller (e.g. official
                // Qobuz mobile app) sends a real seek to qbz acting as the
                // renderer — the audio thread never moved while the cloud
                // state advanced, so the controller's progress bar locked.
                // The is_echo_reset + abs_diff > 2 gates already filter the
                // cloud-echo case the peer_renderer_active check was added
                // to defend against in commit 147bcbd7. If hiccups return,
                // revert this change and reintroduce a more targeted echo
                // detector (UUID-based) instead of the all-or-nothing gate.
                if !loaded_at_reported_position
                    && !is_echo_reset
                    && current_pos_secs.abs_diff(target_secs) > 2
                {
                    log::info!(
                        "[QConnect] SetState seek: current={}s target={}s",
                        current_pos_secs,
                        target_secs
                    );
                    engine.seek(target_secs)?;
                }
            }
        }
        RendererCommand::SetVolume { volume, .. } => {
            if let Some(resolved) = renderer_state.volume.or(*volume) {
                engine.set_volume(normalize_volume_to_fraction(resolved))?;
            }
        }
        RendererCommand::MuteVolume { value } => {
            if *value {
                engine.set_volume(0.0)?;
            } else if let Some(resolved) = renderer_state.volume {
                engine.set_volume(normalize_volume_to_fraction(resolved))?;
            }
        }
        RendererCommand::SetLoopMode { loop_mode } => {
            let resolved_loop_mode = renderer_state.loop_mode.unwrap_or(*loop_mode);
            let repeat_mode = qconnect_repeat_mode_from_loop_mode(resolved_loop_mode)
                .ok_or_else(|| format!("unsupported qconnect loop mode: {resolved_loop_mode}"))?;
            engine.set_repeat_mode(repeat_mode).await;
        }
        RendererCommand::SetActive { active } => {
            if *active {
                let local_playback = engine.get_playback_state();
                if engine.has_loaded_audio() && local_playback.is_playing {
                    // SESSION_STATE arbitration may have chosen the live local
                    // queue while a stale SetActive from the old cloud snapshot
                    // was already in flight. Activation acknowledges ownership;
                    // it is not authority to replace audible local playback with
                    // an unrelated renderer cursor. An intentional remote-queue
                    // takeover stops local audio before claiming this renderer.
                    log::info!(
                        "[QConnect] SetActive(true) preserved active local playback (track_id={})",
                        local_playback.track_id
                    );
                    return Ok(());
                }
                // Becoming the active renderer (takeback). FORCE a stream of the
                // current track instead of a plain ensure-loaded: a prior
                // controller->renderer transition tore the local stream down via
                // engine.stop() (audio buffer cleared, has_loaded_audio=false)
                // while current_track_id still reports the old track, so the
                // track-id guard in ensure_remote_track_loaded would skip the load
                // and the next SetState's resume() would fail with "no audio data
                // available". Resume at the handed-off position so a long
                // track / audiobook does not restart from 0.
                if let Some(current) = renderer_state.current_track.as_ref() {
                    let start_position_secs = renderer_state
                        .current_position_ms
                        .map(|ms| ms / 1000)
                        .unwrap_or(0);
                    if let Err(err) = force_remote_track_stream(
                        engine,
                        sync_state,
                        current.track_id,
                        renderer_state.max_audio_quality,
                        start_position_secs,
                    )
                    .await
                    {
                        log::warn!("[QConnect] SetActive(true) force-stream failed: {err}");
                    }
                }
            } else {
                // The official receiver ignores SetActive(false). Actual audio
                // detachment belongs to ACTIVE_RENDERER_CHANGED, after session
                // topology already names the peer. Stopping here opens a race
                // where the local poll still sees QBZ as active and reports the
                // synthetic stopped/paused edge to the session, pausing the new
                // renderer during handoff.
                log::debug!("[QConnect] SetActive(false) acknowledged; awaiting topology handoff");
            }
        }
        RendererCommand::SetMaxAudioQuality { max_audio_quality } => {
            // Applied on the next load via renderer_state.max_audio_quality
            // (recorded by the core reducer). No immediate re-fetch.
            log::info!("[QConnect] SetMaxAudioQuality => {max_audio_quality}");
        }
        RendererCommand::SetShuffleMode { shuffle_mode } => {
            let enabled = renderer_state.shuffle_mode.unwrap_or(*shuffle_mode);
            // Renderer commands carry only a flag, never the server seed/order.
            // The queue channel is the sole authority allowed to mutate the
            // local queue. Even a "flag-only" core write is unsafe here because
            // it installs identity order and can race after QueueUpdated.
            log::debug!(
                "[QConnect] SetShuffleMode({enabled}) acknowledged; awaiting queue authority"
            );
        }
    }

    Ok(())
}

async fn sync_remote_shuffle_projection(
    engine: &impl QconnectRendererEngine,
    sync_state: &Arc<Mutex<QconnectRemoteSyncState>>,
    queue_state: &QConnectQueueState,
    renderer_state: &QConnectRendererState,
) -> Result<bool, String> {
    if !queue_state.shuffle_mode || queue_state.queue_items.is_empty() {
        return Ok(false);
    }

    let start_index = resolve_remote_start_index(
        queue_state,
        renderer_state
            .current_track
            .as_ref()
            .map(|item| item.queue_item_id),
        renderer_state
            .current_track
            .as_ref()
            .map(|item| item.track_id),
    );
    let Some(start_index) = start_index else {
        return Ok(false);
    };

    let core_shuffle_order = resolve_core_shuffle_order(queue_state);

    // Do not invent an identity shuffle if no WS-authored order is available.
    if core_shuffle_order.is_none() {
        return Ok(false);
    }

    let should_apply = {
        let state = sync_state.lock().await;
        state.last_materialized_start_index != Some(start_index)
            || state.last_materialized_core_shuffle_order != core_shuffle_order
    };
    if !should_apply {
        return Ok(false);
    }

    let (tracks, _) = engine.get_all_queue_tracks().await;
    if tracks.len() != queue_state.queue_items.len() || tracks.is_empty() {
        return Ok(false);
    }

    engine
        .set_queue_with_order(
            tracks,
            Some(start_index),
            queue_state.shuffle_mode,
            core_shuffle_order.clone(),
        )
        .await;

    let mut state = sync_state.lock().await;
    state.last_materialized_start_index = Some(start_index);
    state.last_materialized_core_shuffle_order = core_shuffle_order;
    Ok(true)
}

pub async fn materialize_remote_queue(
    engine: &impl QconnectRendererEngine,
    sync_state: &Arc<Mutex<QconnectRemoteSyncState>>,
    queue_state: &QConnectQueueState,
) -> Result<bool, String> {
    let (
        renderer_queue_item_id,
        renderer_track_id,
        renderer_next_queue_item_id,
        renderer_next_track_id,
        renderer_playing_state,
        should_skip,
    ) = {
        let mut state = sync_state.lock().await;
        if !queue_state_needs_materialization(state.last_applied_queue_state.as_ref(), queue_state)
        {
            (
                state.last_renderer_queue_item_id,
                state.last_renderer_track_id,
                state.last_renderer_next_queue_item_id,
                state.last_renderer_next_track_id,
                state.last_renderer_playing_state,
                true,
            )
        } else {
            state.last_applied_queue_state = Some(queue_state.clone());
            (
                state.last_renderer_queue_item_id,
                state.last_renderer_track_id,
                state.last_renderer_next_queue_item_id,
                state.last_renderer_next_track_id,
                state.last_renderer_playing_state,
                false,
            )
        }
    };

    if should_skip {
        log::debug!(
            "[QConnect] materialize_remote_queue: skipped (identical snapshot {}.{})",
            queue_state.version.major,
            queue_state.version.minor
        );
        return Ok(false);
    }

    log::info!(
        "[QConnect] materialize_remote_queue: version={}.{} items={} renderer_qid={:?} renderer_tid={:?} renderer_next_qid={:?} renderer_next_tid={:?} playing_state={:?}",
        queue_state.version.major,
        queue_state.version.minor,
        queue_state.queue_items.len(),
        renderer_queue_item_id,
        renderer_track_id,
        renderer_next_queue_item_id,
        renderer_next_track_id,
        renderer_playing_state
    );

    if queue_state.queue_items.is_empty() {
        // Preserve legacy behavior: keep current track on qconnect sync clears.
        engine.clear_queue(true).await;
        engine.set_shuffle(false).await;
        let mut state = sync_state.lock().await;
        state.last_materialized_start_index = None;
        state.last_materialized_core_shuffle_order = None;
        return Ok(true);
    }

    // Preserve the richer QueueTrack built by album/playlist entry points
    // before the id-only cloud echo replaces the core queue. Per-id enrichment
    // is safe across reorder/insert operations because every copied field is
    // intrinsic catalog metadata; queue context and the remote source word are
    // deliberately not copied.
    let (existing_tracks, _) = engine.get_all_queue_tracks().await;
    let existing_by_id: HashMap<u64, QueueTrack> = existing_tracks
        .into_iter()
        .filter(|track| !track.is_local)
        .map(|track| (track.id, track))
        .collect();

    let unique_track_ids = dedupe_track_ids(queue_state);
    let fetched_tracks = engine
        .get_tracks_batch(&unique_track_ids)
        .await
        .map_err(|err| format!("fetch tracks batch for remote queue: {err}"))?;

    let mut tracks_by_id = HashMap::with_capacity(fetched_tracks.len());
    for track in fetched_tracks {
        let mut mapped = model_track_to_core_queue_track(&track);
        if let Some(existing) = existing_by_id.get(&track.id) {
            preserve_existing_catalog_metadata(&mut mapped, existing);
        }
        tracks_by_id.insert(track.id, mapped);
    }

    let mut queue_tracks = Vec::with_capacity(queue_state.queue_items.len());
    for item in &queue_state.queue_items {
        if let Some(queue_track) = tracks_by_id.get(&item.track_id) {
            queue_tracks.push(queue_track.clone());
            continue;
        }

        match engine.get_track(item.track_id).await {
            Ok(track) => {
                let mut mapped = model_track_to_core_queue_track(&track);
                if let Some(existing) = existing_by_id.get(&track.id) {
                    preserve_existing_catalog_metadata(&mut mapped, existing);
                }
                tracks_by_id.insert(item.track_id, mapped.clone());
                queue_tracks.push(mapped);
            }
            Err(err) => {
                log::warn!(
                    "[QConnect] Unable to hydrate remote queue track {}: {}",
                    item.track_id,
                    err
                );
            }
        }
    }

    if queue_tracks.is_empty() {
        return Err("remote queue materialization produced zero playable tracks".to_string());
    }

    // Resolve start index from remote state first, then from the local playback
    // cursor only if that track is still part of the remote queue.
    let current_playback_track_id = match engine.get_playback_state().track_id {
        0 => None,
        track_id => Some(track_id),
    };
    let mut start_index =
        resolve_remote_start_index(queue_state, renderer_queue_item_id, renderer_track_id);
    if start_index.is_none() {
        start_index = resolve_remote_start_index(
            queue_state,
            renderer_next_queue_item_id,
            renderer_next_track_id,
        )
        .map(|index| index.saturating_sub(1));
    }
    if start_index.is_none() {
        start_index = current_playback_track_id.and_then(|track_id| {
            queue_state
                .queue_items
                .iter()
                .position(|item| item.track_id == track_id)
        });
    }
    if start_index.is_none() && !queue_tracks.is_empty() {
        start_index = Some(0);
    }
    let core_shuffle_order = resolve_core_shuffle_order(queue_state);
    // A queue update is usable only once it contains a WS-authored order,
    // either copied from QueueState.shuffled_track_indexes or reproduced from
    // the seed/pivot carried by the incremental queue event.
    let effective_shuffle_enabled = queue_state.shuffle_mode && core_shuffle_order.is_some();
    log::info!(
        "[QConnect] materialize_remote_queue: setting queue with {} tracks, start_index={:?}, local_track_id={:?}, remote_shuffle_mode={}, shuffle_order_present={}, engine_shuffle_enabled={}",
        queue_tracks.len(),
        start_index,
        current_playback_track_id,
        queue_state.shuffle_mode,
        core_shuffle_order.is_some(),
        effective_shuffle_enabled,
    );
    engine
        .set_queue_with_order(
            queue_tracks,
            start_index,
            effective_shuffle_enabled,
            core_shuffle_order.clone(),
        )
        .await;

    {
        let mut state = sync_state.lock().await;
        state.last_materialized_start_index = start_index;
        state.last_materialized_core_shuffle_order = core_shuffle_order;
    }

    let local_track_missing_from_remote = current_playback_track_id
        .map(|track_id| {
            !queue_state
                .queue_items
                .iter()
                .any(|item| item.track_id == track_id)
        })
        .unwrap_or(true);

    if let Some(index) = start_index {
        if local_track_missing_from_remote {
            log::info!(
                "[QConnect] materialize_remote_queue: aligning queue cursor to remote index {}",
                index
            );
            let _ = engine.play_index(index).await;
        }
    }

    if current_playback_track_id.is_some()
        && current_playback_track_id != renderer_track_id
        && local_track_missing_from_remote
        && matches!(
            renderer_playing_state,
            Some(PLAYING_STATE_STOPPED | PLAYING_STATE_UNKNOWN)
        )
    {
        log::info!(
            "[QConnect] materialize_remote_queue: stopping stale local playback track {:?} after remote queue replacement",
            current_playback_track_id
        );
        let _ = engine.stop();
    }

    Ok(true)
}

pub async fn align_queue_cursor(
    engine: &impl QconnectRendererEngine,
    track_id: u64,
) -> Result<(), String> {
    let (tracks, current_index) = engine.get_all_queue_tracks().await;
    log::info!(
        "[QConnect] align_queue_cursor: track_id={track_id} queue_len={} current_index={:?}",
        tracks.len(),
        current_index
    );
    if let Some(target_index) = tracks.iter().position(|track| track.id == track_id) {
        if current_index != Some(target_index) {
            log::info!(
                "[QConnect] align_queue_cursor: moving cursor from {:?} to {target_index}",
                current_index
            );
            let _ = engine.play_index(target_index).await;
        }
        return Ok(());
    }

    log::info!(
        "[QConnect] align_queue_cursor: track {track_id} not in queue, fetching and creating single-track queue"
    );
    let track = engine
        .get_track(track_id)
        .await
        .map_err(|err| format!("fetch current remote track {track_id}: {err}"))?;
    let queue_track = model_track_to_core_queue_track(&track);
    engine.set_queue(vec![queue_track], Some(0)).await;
    Ok(())
}

// ===================== Mock-engine trait tests (slice 6, step 8) =====================
//
// These exercise the renderer orchestration end-to-end against a recording mock
// engine — the hard-won behavior that previously could only be tested through the
// Tauri adapter. A passing test here proves the logic is engine-independent: any
// future Slint regression is a wiring bug in its trait impl, not a re-derivation
// bug in the shared logic.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    use async_trait::async_trait;
    use qbz_models::{Quality, QueueTrack, RepeatMode, Track};
    use qbz_player::PlaybackState;
    use qconnect_core::{QueueItem, QueueVersion};
    use tokio::sync::Mutex;

    use crate::renderer_engine::QconnectRendererEngine;
    use crate::{
        QConnectQueueState, QConnectRendererState, QconnectRemoteSyncState, RendererCommand,
    };

    #[derive(Default)]
    struct MockCalls {
        resumes: u32,
        pauses: u32,
        stops: u32,
        seeks: Vec<u64>,
        set_volumes: Vec<f32>,
        set_repeat_modes: u32,
        set_shuffles: Vec<bool>,
        set_queue_with_order: Vec<(bool, Option<Vec<usize>>)>,
        materialized_tracks: Vec<QueueTrack>,
        set_queues: u32,
        clear_queues: Vec<bool>,
        play_indexes: Vec<usize>,
        get_tracks_batch: u32,
        start_track_streams: Vec<u64>,
        start_positions: Vec<u64>,
    }

    /// Records every engine call; serves canned `PlaybackState` + queue snapshot.
    struct MockEngine {
        calls: Arc<StdMutex<MockCalls>>,
        playback: PlaybackState,
        queue_tracks: Vec<QueueTrack>,
        queue_index: Option<usize>,
        loaded_audio: bool,
    }

    impl MockEngine {
        fn new() -> Self {
            Self {
                calls: Arc::new(StdMutex::new(MockCalls::default())),
                playback: PlaybackState::default(),
                queue_tracks: Vec::new(),
                queue_index: None,
                loaded_audio: false,
            }
        }

        fn calls(&self) -> std::sync::MutexGuard<'_, MockCalls> {
            self.calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl QconnectRendererEngine for MockEngine {
        fn resume(&self) -> Result<(), String> {
            self.calls().resumes += 1;
            Ok(())
        }
        fn pause(&self) -> Result<(), String> {
            self.calls().pauses += 1;
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            self.calls().stops += 1;
            Ok(())
        }
        fn seek(&self, position_secs: u64) -> Result<(), String> {
            self.calls().seeks.push(position_secs);
            Ok(())
        }
        fn set_volume(&self, fraction: f32) -> Result<(), String> {
            self.calls().set_volumes.push(fraction);
            Ok(())
        }
        fn get_playback_state(&self) -> PlaybackState {
            self.playback.clone()
        }
        fn has_loaded_audio(&self) -> bool {
            self.loaded_audio
        }
        async fn set_repeat_mode(&self, _mode: RepeatMode) {
            self.calls().set_repeat_modes += 1;
        }
        async fn set_shuffle(&self, enabled: bool) {
            self.calls().set_shuffles.push(enabled);
        }
        async fn get_all_queue_tracks(&self) -> (Vec<QueueTrack>, Option<usize>) {
            (self.queue_tracks.clone(), self.queue_index)
        }
        async fn set_queue(&self, _tracks: Vec<QueueTrack>, _start_index: Option<usize>) {
            self.calls().set_queues += 1;
        }
        async fn set_queue_with_order(
            &self,
            tracks: Vec<QueueTrack>,
            _start_index: Option<usize>,
            shuffle_enabled: bool,
            shuffle_order: Option<Vec<usize>>,
        ) {
            let mut calls = self.calls();
            calls.materialized_tracks = tracks;
            calls
                .set_queue_with_order
                .push((shuffle_enabled, shuffle_order));
        }
        async fn clear_queue(&self, keep_current: bool) {
            self.calls().clear_queues.push(keep_current);
        }
        async fn play_index(&self, index: usize) -> Option<QueueTrack> {
            self.calls().play_indexes.push(index);
            None
        }
        async fn get_track(&self, track_id: u64) -> Result<Track, String> {
            Ok(mock_track(track_id))
        }
        async fn get_tracks_batch(&self, track_ids: &[u64]) -> Result<Vec<Track>, String> {
            self.calls().get_tracks_batch += 1;
            Ok(track_ids.iter().map(|&id| mock_track(id)).collect())
        }
        async fn start_track_stream(
            &self,
            track_id: u64,
            _quality: Quality,
            _duration_secs: u64,
            start_position_secs: u64,
        ) -> Result<(), String> {
            let mut calls = self.calls();
            calls.start_track_streams.push(track_id);
            calls.start_positions.push(start_position_secs);
            Ok(())
        }
        fn current_output_format(&self) -> Option<(u32, u32)> {
            Some((44_100, 16))
        }
    }

    fn qi(track_id: u64, queue_item_id: u64) -> QueueItem {
        QueueItem {
            track_context_uuid: "ctx".to_string(),
            track_id,
            queue_item_id,
        }
    }

    fn mock_track(id: u64) -> Track {
        serde_json::from_value(serde_json::json!({ "id": id, "title": "t", "duration": 100 }))
            .expect("mock track")
    }

    fn mock_queue_track(id: u64) -> QueueTrack {
        model_track_to_core_queue_track(&mock_track(id))
    }

    #[test]
    fn qconnect_admission_uses_provenance_and_accepts_materialized_rows() {
        for source in [
            "qobuz",
            "qobuz_download",
            "qobuz_purchase",
            "offline",
            LEGACY_QCONNECT_REMOTE_QUEUE_SOURCE,
        ] {
            let mut track = mock_queue_track(7);
            track.source = Some(source.to_string());
            track.is_local = source == "qobuz_download";
            assert!(qconnect_queue_track_is_resolvable(&track), "{source}");
        }

        for source in ["local", "plex", "jellyfin", "subsonic", "navidrome"] {
            let mut track = mock_queue_track(7);
            track.source = Some(source.to_string());
            track.is_local = true;
            assert!(!qconnect_queue_track_is_resolvable(&track), "{source}");
        }
    }

    #[test]
    fn qconnect_admission_legacy_rows_require_non_local_positive_ids() {
        let mut track = mock_queue_track(7);
        track.source = None;
        track.is_local = false;
        assert!(qconnect_queue_track_is_resolvable(&track));

        track.is_local = true;
        assert!(!qconnect_queue_track_is_resolvable(&track));
        track.id = 0;
        track.is_local = false;
        assert!(!qconnect_queue_track_is_resolvable(&track));
    }

    fn queue_state(
        version: QueueVersion,
        items: Vec<QueueItem>,
        shuffle_mode: bool,
        shuffle_order: Option<Vec<usize>>,
    ) -> QConnectQueueState {
        QConnectQueueState {
            version,
            queue_items: items,
            shuffle_mode,
            shuffle_order,
            autoplay_mode: false,
            autoplay_loading: false,
            autoplay_items: Vec::new(),
            updated_at_ms: 0,
            last_server_queue_hash: None,
        }
    }

    fn sync() -> Arc<Mutex<QconnectRemoteSyncState>> {
        Arc::new(Mutex::new(QconnectRemoteSyncState::default()))
    }

    /// A queue created from `/album/get` carries both edition suffixes. The
    /// id-only QConnect echo must not replace them with the terse batch-track
    /// response, because every metadata consumer reads the materialized core
    /// queue after this point.
    #[tokio::test]
    async fn materialize_preserves_rich_metadata_from_matching_local_queue() {
        let mut engine = MockEngine::new();
        let mut rich = mock_queue_track(7);
        rich.title = "What Is Life".to_string();
        rich.version = Some("Backing Track / Bonus Track".to_string());
        rich.artist = "George Harrison".to_string();
        rich.album = "All Things Must Pass".to_string();
        rich.album_version = Some("Remastered 2014".to_string());
        rich.artwork_url = Some("https://example.test/cover.jpg".to_string());
        rich.album_id = Some("album-7".to_string());
        rich.artist_id = Some(42);
        rich.bit_depth = Some(24);
        rich.sample_rate = Some(96_000.0);
        rich.hires = true;
        rich.source = Some("qobuz".to_string());
        rich.context_kind = Some("album".to_string());
        rich.context_id = Some("album-7".to_string());
        engine.queue_tracks = vec![rich];
        engine.queue_index = Some(0);

        materialize_remote_queue(
            &engine,
            &sync(),
            &queue_state(QueueVersion::new(1, 0), vec![qi(7, 0)], false, None),
        )
        .await
        .unwrap();

        let calls = engine.calls();
        let track = calls
            .materialized_tracks
            .first()
            .expect("materialized track");
        assert_eq!(
            track.version.as_deref(),
            Some("Backing Track / Bonus Track")
        );
        assert_eq!(track.album_version.as_deref(), Some("Remastered 2014"));
        assert_eq!(track.artist, "George Harrison");
        assert_eq!(track.album, "All Things Must Pass");
        assert_eq!(track.album_id.as_deref(), Some("album-7"));
        assert_eq!(track.artist_id, Some(42));
        assert_eq!(track.bit_depth, Some(24));
        assert_eq!(track.sample_rate, Some(96_000.0));
        assert!(track.hires);
        assert_eq!(track.source.as_deref(), Some("qobuz"));
        assert!(
            track.context_kind.is_none(),
            "remote context remains authoritative"
        );
        assert!(
            track.context_id.is_none(),
            "remote context remains authoritative"
        );
    }

    /// #2 — two loads for the same track within the dedup window trigger exactly
    /// one `start_track_stream`; the second is swallowed by the 5s window even
    /// though the audio thread hasn't reported the track yet.
    #[tokio::test]
    async fn ensure_remote_track_loaded_dedups_within_window() {
        let engine = MockEngine::new(); // playback track_id 0 != 42 → would reload
        let sync = sync();
        ensure_remote_track_loaded(&engine, &sync, 42, None, 0)
            .await
            .unwrap();
        ensure_remote_track_loaded(&engine, &sync, 42, None, 0)
            .await
            .unwrap();
        assert_eq!(engine.calls().start_track_streams, vec![42]);
    }

    /// #2 — no reload when the audio thread already plays the requested track.
    #[tokio::test]
    async fn ensure_remote_track_loaded_skips_when_track_unchanged() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 42,
            ..Default::default()
        };
        let sync = sync();
        ensure_remote_track_loaded(&engine, &sync, 42, None, 0)
            .await
            .unwrap();
        assert!(engine.calls().start_track_streams.is_empty());
    }

    /// #1 / #387 — a SetState targeting the SAME track at <=1s while local is well
    /// ahead is a cloud echo: the seek is rejected.
    #[tokio::test]
    async fn apply_renderer_command_rejects_echo_seek() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7,
            position: 30,
            ..Default::default()
        };
        engine.queue_tracks = vec![mock_queue_track(7)];
        engine.queue_index = Some(0);
        let sync = sync();
        let cmd = RendererCommand::SetState {
            playing_state: None,
            current_position_ms: Some(0),
            current_track: Some(qi(7, 0)),
            next_track: None,
        };
        apply_renderer_command(&engine, &sync, &cmd, &QConnectRendererState::default())
            .await
            .unwrap();
        assert!(
            engine.calls().seeks.is_empty(),
            "echo seek must be rejected (#387 is_echo_reset)"
        );
    }

    /// #1 / #387 — a genuine peer seek (target far from local) IS honored, even
    /// for the same track (the bug the all-or-nothing peer gate caused).
    #[tokio::test]
    async fn apply_renderer_command_honors_genuine_seek() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7,
            position: 10,
            ..Default::default()
        };
        engine.queue_tracks = vec![mock_queue_track(7)];
        engine.queue_index = Some(0);
        let sync = sync();
        let cmd = RendererCommand::SetState {
            playing_state: None,
            current_position_ms: Some(40_000),
            current_track: Some(qi(7, 0)),
            next_track: None,
        };
        apply_renderer_command(&engine, &sync, &cmd, &QConnectRendererState::default())
            .await
            .unwrap();
        assert_eq!(engine.calls().seeks, vec![40]);
    }

    /// A state-only resume (current_track = null) on a COLD engine — queue
    /// cursor restored but no audio loaded, as after a daemon restart — must
    /// cold-load the cloud's current track at the handed-off position instead
    /// of issuing a bare resume that dies in the audio thread ("cannot resume
    /// - no audio data available") and wedges the controller at paused 0:00.
    #[tokio::test]
    async fn apply_renderer_command_cold_resume_loads_current_track() {
        let mut engine = MockEngine::new();
        // Cursor reports the restored track, but nothing is loaded: the plain
        // track-id guard would skip, which is why the force path is used.
        engine.playback = PlaybackState {
            track_id: 7,
            ..Default::default()
        };
        engine.queue_tracks = vec![mock_queue_track(7)];
        engine.queue_index = Some(0);
        engine.loaded_audio = false;
        let sync = sync();
        let cmd = RendererCommand::SetState {
            playing_state: Some(PLAYING_STATE_PLAYING),
            current_position_ms: None,
            current_track: None,
            next_track: None,
        };
        // The cloud's view carries the session's current track + position.
        let renderer_state = QConnectRendererState {
            current_track: Some(qi(7, 2)),
            current_position_ms: Some(242_491),
            ..Default::default()
        };
        apply_renderer_command(&engine, &sync, &cmd, &renderer_state)
            .await
            .unwrap();
        let calls = engine.calls();
        assert_eq!(
            calls.start_track_streams,
            vec![7],
            "cold resume must load the session's current track"
        );
        assert_eq!(
            calls.start_positions,
            vec![242],
            "load must resume at the handed-off position"
        );
        assert_eq!(calls.resumes, 0, "no bare resume on a cold engine");
    }

    /// The field shape from the owner's 2026-08-31 regression: both the
    /// command and renderer projection omit current_track, but the synchronized
    /// queue still has the authoritative cursor. This must cold-load that row,
    /// never fall through to `resume()` on an empty engine.
    #[tokio::test]
    async fn cold_resume_falls_back_to_queue_cursor_when_projection_omits_track() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7,
            position: 38,
            ..Default::default()
        };
        engine.queue_tracks = vec![mock_queue_track(6), mock_queue_track(7)];
        engine.queue_index = Some(1);
        engine.loaded_audio = false;
        let sync = sync();
        let cmd = RendererCommand::SetState {
            playing_state: Some(PLAYING_STATE_PLAYING),
            current_position_ms: None,
            current_track: None,
            next_track: None,
        };

        apply_renderer_command(&engine, &sync, &cmd, &QConnectRendererState::default())
            .await
            .unwrap();

        let calls = engine.calls();
        assert_eq!(calls.start_track_streams, vec![7]);
        assert_eq!(calls.start_positions, vec![38]);
        assert_eq!(calls.resumes, 0);
    }

    #[tokio::test]
    async fn cold_resume_never_guesses_a_local_queue_id_is_qobuz() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7,
            ..Default::default()
        };
        let mut local = mock_queue_track(7);
        local.source = Some("local".to_string());
        local.is_local = true;
        engine.queue_tracks = vec![local];
        engine.queue_index = Some(0);
        engine.loaded_audio = false;
        let cmd = RendererCommand::SetState {
            playing_state: Some(PLAYING_STATE_PLAYING),
            current_position_ms: None,
            current_track: None,
            next_track: None,
        };

        let error =
            apply_renderer_command(&engine, &sync(), &cmd, &QConnectRendererState::default())
                .await
                .expect_err("local ids must not be sent to Qobuz");

        assert!(error.contains("no projected, queued, or preserved track"));
        let calls = engine.calls();
        assert!(calls.start_track_streams.is_empty());
        assert_eq!(calls.resumes, 0);
    }

    /// The cold-start load never fires while audio is loaded: a resume during
    /// live playback (or a cloud echo) stays a plain resume, no re-stream.
    #[tokio::test]
    async fn apply_renderer_command_warm_resume_stays_plain() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7,
            position: 30,
            ..Default::default()
        };
        engine.queue_tracks = vec![mock_queue_track(7)];
        engine.queue_index = Some(0);
        engine.loaded_audio = true;
        let sync = sync();
        let cmd = RendererCommand::SetState {
            playing_state: Some(PLAYING_STATE_PLAYING),
            current_position_ms: None,
            current_track: None,
            next_track: None,
        };
        let renderer_state = QConnectRendererState {
            current_track: Some(qi(7, 2)),
            ..Default::default()
        };
        apply_renderer_command(&engine, &sync, &cmd, &renderer_state)
            .await
            .unwrap();
        let calls = engine.calls();
        assert!(
            calls.start_track_streams.is_empty(),
            "no re-stream while audio is loaded"
        );
        assert_eq!(calls.resumes, 1, "plain resume on a warm engine");
    }

    /// WS-authoritative shuffle: a standalone renderer command has no seed/order
    /// and therefore must not mutate the core queue at all. The queue event is
    /// the sole authority.
    #[tokio::test]
    async fn apply_renderer_command_setshufflemode_does_not_touch_local_queue() {
        let mut engine = MockEngine::new();
        engine.queue_tracks = vec![
            mock_queue_track(1),
            mock_queue_track(2),
            mock_queue_track(3),
        ];
        engine.queue_index = Some(0);
        let sync = sync();
        let cmd = RendererCommand::SetShuffleMode { shuffle_mode: true };
        apply_renderer_command(&engine, &sync, &cmd, &QConnectRendererState::default())
            .await
            .unwrap();
        let calls = engine.calls();
        assert!(
            calls.set_shuffles.is_empty(),
            "renderer command must not call local shuffle"
        );
        assert!(
            calls.set_queue_with_order.is_empty(),
            "renderer command must not apply an order without queue authority"
        );
    }

    /// #3 — a state-only SetState (command.current_track = None) must NOT align the
    /// cursor or load a track from the renderer_state's stale current_track (the
    /// iOS-pause-jumped-back fix); the playing_state is still applied.
    #[tokio::test]
    async fn apply_renderer_command_skips_track_ops_on_state_only_update() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7,
            position: 5,
            ..Default::default()
        };
        let sync = sync();
        let cmd = RendererCommand::SetState {
            playing_state: Some(PLAYING_STATE_PAUSED),
            current_position_ms: None,
            current_track: None,
            next_track: None,
        };
        // renderer_state carries a STALE current_track the projection would fall
        // back to — it must not drive a load/align.
        let renderer_state = QConnectRendererState {
            current_track: Some(qi(99, 0)),
            ..Default::default()
        };
        apply_renderer_command(&engine, &sync, &cmd, &renderer_state)
            .await
            .unwrap();
        let calls = engine.calls();
        assert!(
            calls.start_track_streams.is_empty(),
            "no load on state-only update"
        );
        assert!(
            calls.play_indexes.is_empty(),
            "no cursor align on state-only update"
        );
        assert_eq!(calls.pauses, 1, "pause still applied");
    }

    /// #5 — shuffle deferral: the first event (shuffle_mode=true, order=None)
    /// materializes with shuffle_enabled=false (no invented identity order); the
    /// second event (authoritative order present) enables shuffle.
    #[tokio::test]
    async fn materialize_defers_shuffle_until_authoritative_order() {
        let engine = MockEngine::new();
        let sync = sync();
        let items = vec![qi(10, 0), qi(11, 1)];

        let q1 = queue_state(QueueVersion::new(1, 0), items.clone(), true, None);
        materialize_remote_queue(&engine, &sync, &q1).await.unwrap();
        {
            let calls = engine.calls();
            assert_eq!(calls.set_queue_with_order.len(), 1);
            assert!(
                !calls.set_queue_with_order[0].0,
                "shuffle deferred while order absent"
            );
        }

        let q2 = queue_state(QueueVersion::new(1, 1), items, true, Some(vec![1, 0]));
        materialize_remote_queue(&engine, &sync, &q2).await.unwrap();
        {
            let calls = engine.calls();
            assert_eq!(calls.set_queue_with_order.len(), 2);
            assert!(
                calls.set_queue_with_order[1].0,
                "shuffle enabled once authoritative order present"
            );
        }
    }

    /// #1 (takeback) — becoming the active renderer FORCE-streams the current
    /// track even though `playback_state.track_id` still matches: the prior
    /// controller->renderer stop() cleared the audio buffer but left the stale
    /// track id, so the plain track-id guard would skip the load and the next
    /// SetState's resume() would fail with "no audio data available". Also
    /// resumes at the handed-off position, not 0.
    #[tokio::test]
    async fn set_active_force_streams_on_takeback_when_audio_torn_down() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7, // stale id left by stop(); audio is gone
            ..Default::default()
        };
        engine.loaded_audio = false;
        let sync = sync();
        let cmd = RendererCommand::SetActive { active: true };
        let renderer_state = QConnectRendererState {
            current_track: Some(qi(7, 0)),
            current_position_ms: Some(45_000),
            ..Default::default()
        };
        apply_renderer_command(&engine, &sync, &cmd, &renderer_state)
            .await
            .unwrap();
        let calls = engine.calls();
        assert_eq!(
            calls.start_track_streams,
            vec![7],
            "takeback must force a stream even when the track id matches"
        );
        assert_eq!(
            calls.start_positions,
            vec![45],
            "takeback must resume at the handed-off position (45s), not 0"
        );
    }

    #[tokio::test]
    async fn set_active_false_waits_for_topology_before_stopping_audio() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7,
            is_playing: true,
            ..Default::default()
        };
        engine.loaded_audio = true;

        apply_renderer_command(
            &engine,
            &sync(),
            &RendererCommand::SetActive { active: false },
            &QConnectRendererState::default(),
        )
        .await
        .unwrap();

        assert_eq!(
            engine.calls().stops,
            0,
            "SetActive(false) must not synthesize a paused edge before ACTIVE_RENDERER_CHANGED"
        );
    }

    /// #1 (no-interrupt) — a SetActive(true) while the renderer is ALREADY
    /// streaming this exact track with audio loaded must NOT restart it (guards
    /// against a spurious activation tearing down live playback).
    #[tokio::test]
    async fn set_active_does_not_restart_when_already_streaming() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7,
            ..Default::default()
        };
        engine.loaded_audio = true; // live playback in progress
        let sync = sync();
        let cmd = RendererCommand::SetActive { active: true };
        let renderer_state = QConnectRendererState {
            current_track: Some(qi(7, 0)),
            current_position_ms: Some(45_000),
            ..Default::default()
        };
        apply_renderer_command(&engine, &sync, &cmd, &renderer_state)
            .await
            .unwrap();
        assert!(
            engine.calls().start_track_streams.is_empty(),
            "must not restart an already-streaming track on a spurious SetActive"
        );
    }

    /// Regression (2026-09-01): local track 7 was actively playing when a
    /// stale SetActive carried remote track 99. The command created a one-track
    /// queue and destroyed the local session before its queue upload settled.
    #[tokio::test]
    async fn set_active_does_not_replace_different_active_local_track() {
        let mut engine = MockEngine::new();
        engine.playback = PlaybackState {
            track_id: 7,
            is_playing: true,
            ..Default::default()
        };
        engine.loaded_audio = true;
        engine.queue_tracks = vec![mock_queue_track(7), mock_queue_track(8)];
        engine.queue_index = Some(0);

        let renderer_state = QConnectRendererState {
            current_track: Some(qi(99, 9)),
            current_position_ms: Some(71_000),
            ..Default::default()
        };
        apply_renderer_command(
            &engine,
            &sync(),
            &RendererCommand::SetActive { active: true },
            &renderer_state,
        )
        .await
        .unwrap();

        let calls = engine.calls();
        assert!(calls.start_track_streams.is_empty());
        assert_eq!(calls.set_queues, 0);
        assert!(calls.play_indexes.is_empty());
    }

    /// #1 (takeback first-load via SetState) — when the FIRST load on a takeback
    /// lands in the SetState path (SetActive arrived before current_track was
    /// known, so the force-stream couldn't fire), the load must stream at the
    /// cloud's reported position, not 0 — so a mid-track takeback resumes where
    /// the peer was instead of restarting (a forward seek past the buffered
    /// watermark is silently ignored, so streaming from 0 stuck at the start).
    #[tokio::test]
    async fn apply_renderer_command_setstate_streams_at_reported_position() {
        let engine = MockEngine::new(); // playback track_id 0 → fresh load
        let sync = sync();
        let cmd = RendererCommand::SetState {
            playing_state: Some(PLAYING_STATE_PLAYING),
            current_position_ms: Some(118_000),
            current_track: Some(qi(7, 1)),
            next_track: None,
        };
        apply_renderer_command(&engine, &sync, &cmd, &QConnectRendererState::default())
            .await
            .unwrap();
        let calls = engine.calls();
        assert_eq!(calls.start_track_streams, vec![7], "fresh takeback load");
        assert_eq!(
            calls.start_positions,
            vec![118],
            "takeback load must resume at the cloud position (118s), not 0"
        );
        assert_eq!(
            calls.seeks,
            Vec::<u64>::new(),
            "the load already starts at 118s; the same SetState must not seek again"
        );
    }
}
