// crates/qbzd/src/api/playback.rs — routes 4-12 (02-cli-and-api.md §3.3.4-12):
// GET /api/now-playing + the 8 POST /api/playback/* transport routes.
//
// 409 needs_auth (01-architecture.md §6.2) is gated per-route by reading the
// per-route Errors column in 02 §3.3, not blanket-applied: `/api/now-playing`
// gates unconditionally (§3.3.4); `play`/`toggle` gate ONLY the cold-start
// branch (§3.3.5-8 "cold-start needs a session"); `next`/`previous` gate
// unconditionally before running the advance ritual (§3.3.9-10); `pause`/
// `stop`/`seek`/`volume` never cold-start and are NOT listed with needs_auth
// in their own Errors columns (§3.3.11-12), so they act on whatever is
// already loaded regardless of auth state.
//
// DSD-direct guard: volume (including the `mute` body form) remains fixed for
// bit-perfect output. Seek is supported by replacing the demuxed DSD source
// while the direct stream remains open, so it is intentionally not guarded.
//
// Mute is daemon-owned state in `DaemonShared.{muted, premute_volume}` (T2
// seam), NOT the desktop's process statics (`crates/qbz/src/playback.rs:
// 3907-3930`) — same semantics (stash-then-zero / restore), different owner.
// The reported `playback.volume` is always the NOMINAL (pre-mute) level, both
// muted and unmuted: `premute_volume` when muted, the live player volume
// otherwise. This is what makes `now`'s and `mute`'s human lines ("vol 80%",
// "muted (was 80%)") trivial reads of one JSON field, and mirrors the
// desktop's PREMUTE_VOLUME/MUTED pair exactly, just relocated.
use std::io::Cursor;
use std::sync::Arc;

use serde_json::Value;
use tiny_http::Response;

use crate::state::AuthState;

use super::{
    canon_volume, err_json, json, owner_action_gate, owner_action_lease, transport_action_lease,
    ApiState,
};

/// `GET /api/now-playing` (02 §3.3.4). `playback` is the serialized
/// `PlaybackEvent` (qbz-player/src/player/mod.rs:925) with `shuffle`/`repeat`
/// filled in from the queue (the player itself leaves them `None` — "Set by
/// caller with access to queue state") plus the daemon-owned `muted` field,
/// plus an ADDITIVE `queue_len` (02 §3.1.4 allows additive fields within
/// api_version 1; needed so `qbzd now`'s stopped-state render, "stopped ·
/// queue 14 tracks", has a count — the documented playing-state example has
/// no queue count because nothing needs one while a track is loaded).
/// `track` is the current `QueueTrack`, or `null` when nothing is loaded.
pub fn now_playing(state: &ApiState) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = auth_gate(state) {
        return resp;
    }

    let player = state.runtime.core().player();
    let mut ev = player.get_playback_event();
    let queue = state.rt.block_on(state.runtime.core().get_queue_state());

    ev.shuffle = Some(queue.shuffle);
    ev.repeat = Some(repeat_str(queue.repeat));

    let (muted, nominal_volume) = nominal_volume(state, ev.volume);
    ev.volume = nominal_volume;

    let mut playback = serde_json::to_value(&ev).unwrap_or_else(|_| serde_json::json!({}));
    if let Value::Object(map) = &mut playback {
        map.insert("muted".into(), serde_json::json!(muted));
        map.insert("queue_len".into(), serde_json::json!(queue.total_tracks));
        // Overwrite the f32→f64-widened `volume` serde_json::to_value produced
        // above with the canonical (3-decimal) form — see `canon_volume`.
        map.insert("volume".into(), canon_volume(nominal_volume));
    }

    let track = queue
        .current_track
        .as_ref()
        .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);

    json(
        200,
        serde_json::json!({"playback": playback, "track": track}),
    )
}

/// `POST /api/playback/play` (02 §3.3.5). Resume if paused; cold-start the
/// current queue track when `!has_loaded_audio()` (the desktop's
/// `toggle_play_pause` cold-start branch, `crates/qbz/src/playback.rs:3837-3860`).
pub fn play(state: &ApiState) -> Response<Cursor<Vec<u8>>> {
    let transport_lease = match transport_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    if state.runtime.core().player().has_loaded_audio() {
        return match state.runtime.core().resume() {
            Ok(()) => json(200, serde_json::json!({"state": "playing"})),
            Err(e) => runtime_error(&e.to_string()),
        };
    }
    // Cold start selects/resolves the owner queue and therefore needs the
    // stricter owner lease. Drop the transport permit first; `cold_start`
    // revalidates authority before its first mutation.
    drop(transport_lease);
    // The cold-start load is spawn-and-ack: report "loading" (honest ack);
    // `qbzd now` / SSE show the transition to playing.
    match cold_start(state) {
        Ok(()) => json(200, serde_json::json!({"state": "loading"})),
        Err(resp) => resp,
    }
}

/// `POST /api/playback/pause` (02 §3.3.6). Never cold-starts; exit set is
/// 0 · 1 · 3 (no 5, §2.2) so a `Player::pause` channel failure is
/// [`runtime_error`], not `audio_unavailable`.
pub fn pause(state: &ApiState) -> Response<Cursor<Vec<u8>>> {
    let _transport_lease = match transport_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    match state.runtime.core().pause() {
        Ok(()) => json(200, serde_json::json!({"state": "paused"})),
        Err(e) => runtime_error(&e.to_string()),
    }
}

/// `POST /api/playback/toggle` (02 §3.3.7). Mirrors the desktop's
/// `toggle_play_pause`: playing -> pause; paused-with-loaded-audio -> resume;
/// nothing loaded -> cold-start (same gate as `play`). The cold-start branch
/// can return `audio_unavailable`; an accepted load reports later stream
/// failures through daemon state. The pause/resume branches use
/// [`runtime_error`] like plain `pause`/`stop`.
pub fn toggle(state: &ApiState) -> Response<Cursor<Vec<u8>>> {
    let transport_lease = match transport_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let player = state.runtime.core().player();
    let ev = player.get_playback_event();
    if ev.is_playing {
        return match state.runtime.core().pause() {
            Ok(()) => json(200, serde_json::json!({"state": "paused"})),
            Err(e) => runtime_error(&e.to_string()),
        };
    }
    if player.has_loaded_audio() {
        return match state.runtime.core().resume() {
            Ok(()) => json(200, serde_json::json!({"state": "playing"})),
            Err(e) => runtime_error(&e.to_string()),
        };
    }
    // The remaining branch is owner queue selection/stream resolution, not a
    // primitive transport command. Re-admit it under owner authority.
    drop(transport_lease);
    // The cold-start load is spawn-and-ack: report "loading" (honest ack);
    // `qbzd now` / SSE show the transition to playing.
    match cold_start(state) {
        Ok(()) => json(200, serde_json::json!({"state": "loading"})),
        Err(resp) => resp,
    }
}

/// `POST /api/playback/stop` (02 §3.3.8). Never cold-starts; same exit-set
/// reasoning as `pause`.
pub fn stop(state: &ApiState) -> Response<Cursor<Vec<u8>>> {
    let _transport_lease = match transport_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    match state.runtime.core().stop() {
        Ok(()) => json(200, serde_json::json!({"state": "stopped"})),
        Err(e) => runtime_error(&e.to_string()),
    }
}

/// `POST /api/playback/next` (02 §3.3.9).
pub fn next(state: &ApiState) -> Response<Cursor<Vec<u8>>> {
    advance(state, true)
}

/// `POST /api/playback/previous` (02 §3.3.10).
pub fn previous(state: &ApiState) -> Response<Cursor<Vec<u8>>> {
    advance(state, false)
}

/// `POST /api/playback/seek` (02 §3.3.11). Body `{"position": N}` (absolute)
/// or `{"delta": N}` (additive seconds). Returns the CLAMPED target — the
/// value `Player::seek` will settle on (`qbz-player/src/player/mod.rs:5134`
/// clamps to duration) — rather than a live re-read, since `seek` only sends
/// an async command to the audio thread; the clamp is deterministic so the
/// "post-state" is knowable synchronously.
pub fn seek(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    let _transport_lease = match transport_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let player = state.runtime.core().player();
    let ev = player.get_playback_event();
    let target: u64 = if let Some(pos) = body.get("position").and_then(|v| v.as_u64()) {
        pos
    } else if let Some(delta) = body.get("delta").and_then(|v| v.as_i64()) {
        (ev.position as i64 + delta).max(0) as u64
    } else {
        return err_json(
            400,
            "bad_request",
            "seek requires a 'position' or 'delta' field",
            "body: {\"position\": 90} or {\"delta\": -10}",
        );
    };
    let clamped = if ev.duration > 0 {
        target.min(ev.duration)
    } else {
        target
    };
    if let Err(e) = state.runtime.core().seek(clamped) {
        return runtime_error(&e.to_string());
    }
    json(
        200,
        serde_json::json!({"position": clamped, "duration": ev.duration}),
    )
}

/// `POST /api/playback/volume` (02 §3.3.12). One of three body forms:
/// `{"volume": F}` (absolute 0.0-1.0), `{"delta": F}` (additive), or
/// `{"mute": "on"|"off"|"toggle"}` (also `qbzd mute`'s route — no dedicated
/// route, §2.2). All three are gated by the same DSD-direct guard as `seek`.
pub fn volume(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    let _transport_lease = match transport_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let player = state.runtime.core().player();
    if player.is_dsd_direct_active() {
        return err_json(
            409,
            "volume_fixed_dsd",
            "volume is fixed in DSD-direct mode (bit-perfect passthrough)",
            "set DSD mode to \"convert\": qbzd setup (Audio screen)",
        );
    }
    let live = player.get_playback_event().volume;

    if let Some(mute_arg) = body.get("mute").and_then(|v| v.as_str()) {
        return apply_mute(state, live, mute_arg);
    }

    let (muted_before, nominal_before) = nominal_volume(state, live);
    let target = if let Some(v) = body.get("volume").and_then(|v| v.as_f64()) {
        (v as f32).clamp(0.0, 1.0)
    } else if let Some(d) = body.get("delta").and_then(|v| v.as_f64()) {
        (nominal_before + d as f32).clamp(0.0, 1.0)
    } else {
        return err_json(
            400,
            "bad_request",
            "volume requires a 'volume', 'delta' or 'mute' field",
            "body: {\"volume\": 0.75}",
        );
    };

    // An explicit non-zero target clears an active mute (desktop parity:
    // `crates/qbz/src/playback.rs:3921-3924` — "a non-zero level clears any
    // active mute").
    let mut muted_after = muted_before;
    if target > 0.0 && muted_before {
        if let Ok(mut s) = state.shared.lock() {
            s.muted = false;
        }
        muted_after = false;
    }
    if let Err(e) = state.runtime.core().set_volume(target) {
        return runtime_error(&e.to_string());
    }
    json(
        200,
        serde_json::json!({"volume": canon_volume(target), "muted": muted_after}),
    )
}

/// `POST /api/playback/shuffle` (CONSOLE). Body `{"mode": "on"|"off"|"toggle"}`
/// (default `toggle`). Returns the resulting shuffle state. No auth — a
/// queue-mode toggle touches no Qobuz session; state also surfaces in
/// `/api/status` and `/api/now-playing`.
pub fn shuffle(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = owner_action_gate(state) {
        return resp;
    }
    let mode = body
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("toggle");
    let _owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let enabled = match mode {
        "on" => {
            state.rt.block_on(state.runtime.core().set_shuffle(true));
            true
        }
        "off" => {
            state.rt.block_on(state.runtime.core().set_shuffle(false));
            false
        }
        "toggle" => state.rt.block_on(state.runtime.core().toggle_shuffle()),
        other => {
            return err_json(
                400,
                "bad_request",
                &format!("invalid mode '{other}'"),
                "mode: on | off | toggle",
            )
        }
    };
    json(200, serde_json::json!({"shuffle": enabled}))
}

/// `POST /api/playback/repeat` (CONSOLE). Body `{"mode": "off"|"all"|"one"}`.
/// No auth. Returns the applied mode.
pub fn repeat(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = owner_action_gate(state) {
        return resp;
    }
    let mode = body.get("mode").and_then(|v| v.as_str()).unwrap_or("");
    let rm = match mode {
        "off" => qbz_models::RepeatMode::Off,
        "all" => qbz_models::RepeatMode::All,
        "one" => qbz_models::RepeatMode::One,
        other => {
            return err_json(
                400,
                "bad_request",
                &format!("invalid repeat mode '{other}'"),
                "mode: off | all | one",
            )
        }
    };
    let _owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    state.rt.block_on(state.runtime.core().set_repeat_mode(rm));
    json(200, serde_json::json!({"repeat": mode}))
}

// ============================ internals ============================

/// `{"mute": "on"|"off"|"toggle"}` — stash-then-zero / restore, mirroring the
/// desktop's `toggle_mute` (`crates/qbz/src/playback.rs:3936-3961`) but
/// against `DaemonShared` instead of process statics. `live` is the player's
/// volume BEFORE this call (the value to stash on a fresh mute).
fn apply_mute(state: &ApiState, live: f32, arg: &str) -> Response<Cursor<Vec<u8>>> {
    let mute_on = match arg {
        "on" => true,
        "off" => false,
        "toggle" => !state.shared.lock().map(|s| s.muted).unwrap_or(false),
        other => {
            return err_json(
                400,
                "bad_request",
                &format!("invalid mute state '{other}' — use on, off, or toggle"),
                "body: {\"mute\": \"toggle\"}",
            )
        }
    };

    let mut guard = match state.shared.lock() {
        Ok(g) => g,
        Err(_) => {
            return err_json(
                500,
                "internal",
                "daemon state lock poisoned",
                "restart qbzd",
            )
        }
    };

    // Desktop fallback for a never-set / zero premute level (playback.rs:3944,
    // 3956): 0.7, so a mute taken at volume 0 still restores to something
    // audible on unmute.
    let (nominal, set_result) = if mute_on && !guard.muted {
        let stash = if live > 0.0 { live } else { 0.7 };
        guard.premute_volume = stash;
        guard.muted = true;
        (stash, state.runtime.core().set_volume(0.0))
    } else if !mute_on && guard.muted {
        let restored = if guard.premute_volume > 0.0 {
            guard.premute_volume
        } else {
            0.7
        };
        guard.muted = false;
        (restored, state.runtime.core().set_volume(restored))
    } else {
        // Already in the requested state — a no-op that still reports the
        // current nominal level.
        let nominal = if guard.muted {
            guard.premute_volume
        } else {
            live
        };
        (nominal, Ok(()))
    };
    let muted_now = guard.muted;
    drop(guard);

    if let Err(e) = set_result {
        return runtime_error(&e.to_string());
    }
    json(
        200,
        serde_json::json!({"volume": canon_volume(nominal), "muted": muted_now}),
    )
}

/// `next`/`previous` (02 §3.3.9-10): gate on NeedsAuth BEFORE running the
/// ritual (unconditional per those two rows' Errors column, unlike
/// play/toggle's cold-start-only gate), then SPAWN
/// `qbz_app::playback_driver::advance_and_play` — the FULL ritual (skip-walk →
/// play → prefetch → persist), never a bare cursor move (02 §2.2 trap).
///
/// Spawn-and-ack: the API serve loop is single-threaded, and the ritual's
/// load leg (resolve+fetch) can block for many seconds on a slow link — an
/// inline `block_on` starved every other route ("daemon not reachable" while
/// the action DID execute). The ritual runs on the tokio runtime; a failure
/// latches into `last_errors.stream` (visible via `qbzd status`) and the log.
/// The landing track is no longer reported synchronously — follow it via
/// `qbzd now` / SSE.
fn advance(state: &ApiState, forward: bool) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = owner_action_gate(state) {
        return resp;
    }
    if let Some(resp) = auth_gate(state) {
        return resp;
    }
    let owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let quality = resolve_quality(state);
    let runtime = std::sync::Arc::clone(&state.runtime);
    let shared = Arc::clone(&state.shared);
    state.rt.spawn(async move {
        let _owner_lease = owner_lease;
        if let Err(err) =
            qbz_app::playback_driver::advance_and_play(runtime.as_ref(), quality, forward).await
        {
            log::error!("[api] advance(forward={forward}) failed: {err}");
            if let Ok(mut s) = shared.lock() {
                s.last_errors.stream = Some(format!("advance: {err}"));
            }
        }
    });
    json(
        200,
        serde_json::json!({"queued": true, "direction": if forward { "next" } else { "previous" }}),
    )
}

/// `play`/`toggle`'s cold-start branch: gate on NeedsAuth, resolve the
/// current queue track, then SPAWN resolve+play+persist — the same ritual
/// tail `advance_and_play` runs, minus the cursor-move (we're playing the
/// CURRENT track, not advancing to a new one) and the gapless prefetch (the
/// running driver's tick-based `ArmGapless` picks that up on a later tick
/// once playback is underway).
///
/// Spawn-and-ack (see `advance`): the gates (auth, empty queue) stay
/// synchronous so their documented errors are immediate; the network-bound
/// load leg runs on the tokio runtime and latches failures into
/// `last_errors.stream` instead of a 5xx. Ok(()) means "load queued" — the
/// callers answer `{"state": "loading"}`.
fn cold_start(state: &ApiState) -> Result<(), Response<Cursor<Vec<u8>>>> {
    let owner_lease = owner_action_lease(state)?;
    if let Some(resp) = auth_gate(state) {
        return Err(resp);
    }
    let queue = state.rt.block_on(state.runtime.core().get_queue_state());
    let Some(track) = queue.current_track else {
        // No documented error code fits "empty queue" exactly; audio_unavailable
        // (503, exit 5) is the closest frozen taxonomy match — "can't produce
        // audio because there is nothing queued" — and the hint names the fix.
        return Err(err_json(
            503,
            "audio_unavailable",
            "queue is empty, nothing to play",
            "queue a track first: qbzd queue add <TRACK_ID>",
        ));
    };
    let track_id = track.id;
    let quality = resolve_quality(state);
    let runtime = std::sync::Arc::clone(&state.runtime);
    let shared = Arc::clone(&state.shared);
    state.rt.spawn(async move {
        let _owner_lease = owner_lease;
        let played = runtime
            .core()
            .play_track_resolved(track_id, quality, None, None, 0)
            .await;
        if let Err(err) = played {
            log::error!("[api] cold-start play of {track_id} failed: {err}");
            if let Ok(mut s) = shared.lock() {
                s.last_errors.stream = Some(format!("play: {err}"));
            }
            return;
        }
        qbz_app::playback_driver::save_session_now(runtime.as_ref()).await;
    });
    Ok(())
}

/// Streaming quality for a play-time resolve, from the daemon's persisted
/// prefs — the SAME key contract `daemon.rs` uses to seed the driver's
/// `DriverDeps.quality` closure at boot (01 §10.3), so a cold-start play and
/// the next auto-advance never pick different tiers.
pub(crate) fn resolve_quality(state: &ApiState) -> qbz_models::Quality {
    let prefs = qbz_app::settings::daemon_prefs::load_at(&state.roots.data);
    qbz_app::playback_driver::quality_from_key(&prefs.streaming_quality)
}

/// 409 `needs_auth` (01 §6.2 / 02 §3.1.3 example envelope, verbatim).
fn auth_gate(state: &ApiState) -> Option<Response<Cursor<Vec<u8>>>> {
    let needs_auth = state
        .shared
        .lock()
        .map(|s| s.auth == AuthState::NeedsAuth)
        .unwrap_or(false);
    if needs_auth {
        Some(err_json(
            409,
            "needs_auth",
            "not logged in to Qobuz",
            "run: qbzd login",
        ))
    } else {
        None
    }
}

/// A generic runtime failure, exit 1 (02 §1.3's catch-all) — e.g. the
/// player's command channel is dead. `code` "internal" is NOT one of
/// `error_from_envelope`'s special-cased codes, so it falls to
/// `CliError::Runtime` client-side.
fn runtime_error(message: &str) -> Response<Cursor<Vec<u8>>> {
    err_json(500, "internal", message, "check: qbzd status")
}

/// The NOMINAL volume (what `now`/`volume`/`mute` all report): the live
/// player volume, EXCEPT while muted, where it's the stashed `premute_volume`
/// — the player's real output is 0.0 while muted, but the reported level
/// stays at what the user set it to, so `vol 80%` keeps reading `80%` through
/// a mute/unmute cycle. Returns `(muted, nominal)`.
fn nominal_volume(state: &ApiState, live: f32) -> (bool, f32) {
    match state.shared.lock() {
        Ok(s) => (s.muted, if s.muted { s.premute_volume } else { live }),
        Err(_) => (false, live),
    }
}

fn repeat_str(mode: qbz_models::RepeatMode) -> String {
    match mode {
        qbz_models::RepeatMode::Off => "off",
        qbz_models::RepeatMode::All => "all",
        qbz_models::RepeatMode::One => "one",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_str_matches_contract_lowercase() {
        assert_eq!(repeat_str(qbz_models::RepeatMode::Off), "off");
        assert_eq!(repeat_str(qbz_models::RepeatMode::All), "all");
        assert_eq!(repeat_str(qbz_models::RepeatMode::One), "one");
    }

    /// A minimal `PlaybackEvent` for the volume-serialization tests below —
    /// same shape `player.get_playback_event()` returns, just hand-built so
    /// the test does not need a live `ApiState`/runtime.
    fn sample_event(volume: f32) -> qbz_player::player::PlaybackEvent {
        qbz_player::player::PlaybackEvent {
            is_playing: true,
            position: 10,
            duration: 200,
            track_id: 42,
            volume,
            hardware_volume_active: false,
            sample_rate: None,
            bit_depth: None,
            shuffle: Some(false),
            repeat: Some("off".into()),
            normalization_gain: None,
            gapless_ready: false,
            gapless_next_track_id: 0,
            bit_perfect_mode: None,
            buffer_progress: None,
            buffer_state: qbz_player::player::PlaybackBufferState::Ready,
            buffer_track_id: 42,
            engine_empty_generation: 0,
            engine_empty_track_id: 0,
            source_failure_generation: 0,
            source_failure_track_id: 0,
        }
    }

    #[test]
    fn now_playing_map_path_serializes_canonical_volume() {
        // Pins the exact `now_playing` sequence: `to_value(&ev)` (which
        // widens f32 volume via `Number::from_f32`), then the map overwrite
        // that replaces it with `canon_volume`. 0.8f32 must land as `0.8`,
        // never `0.800000011920929`.
        let ev = sample_event(0.8f32);
        let mut playback = serde_json::to_value(&ev).unwrap();
        if let Value::Object(map) = &mut playback {
            map.insert("muted".into(), serde_json::json!(false));
            map.insert("queue_len".into(), serde_json::json!(3));
            map.insert("volume".into(), canon_volume(ev.volume));
        }
        let rendered = serde_json::to_string(&playback).unwrap();
        assert!(rendered.contains("\"volume\":0.8"), "got: {rendered}");
        assert!(!rendered.contains("0.80000"), "got: {rendered}");
    }

    #[test]
    fn volume_post_response_serializes_canonical_volume() {
        // Pins the `/api/playback/volume` and `mute` response builders'
        // `json!({"volume": canon_volume(_), ...})` shape.
        let target = 0.8f32;
        let body = serde_json::json!({"volume": canon_volume(target), "muted": false});
        let rendered = serde_json::to_string(&body).unwrap();
        assert!(rendered.contains("\"volume\":0.8"), "got: {rendered}");
        assert!(!rendered.contains("0.80000"), "got: {rendered}");
    }
}
