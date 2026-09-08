// crates/qbzd/src/api/queue.rs — routes 13-16 (02-cli-and-api.md §3.3.13-16):
// GET /api/queue, POST /api/queue/{add,remove,clear}.
//
// INDEX CONVENTION (normative, §3.3.13/§3.3.15, cross-doc-fixed): every index
// on the wire is 0-based, the SAME space as `current_index` in the
// `GET /api/queue` response. The CLI is the only place a 1-based position
// exists (§2.2's `queue list` table, `queue remove <INDEX>`) — the translation
// happens ONLY at the CLI boundary (`cli/queue.rs`), never here. This file
// speaks 0-based exclusively, straight from `QueueState.current_index`
// (`crates/qbz-models/src/playback.rs:94-103`) with no shift.
//
// 409 needs_auth is gated per-route by the §3.3.14-16 Errors columns, same
// discipline as api/playback.rs's header comment: `add` gates (needs a
// session to resolve tracks via `core.get_track`); `list`/`remove`/`clear`
// carry no needs_auth in their Errors column and act on whatever queue state
// already exists regardless of auth.
//
// Server-side materialization (brief, non-negotiable): `add` NEVER accepts a
// client-built `QueueTrack`. It resolves each `track_id` via `core.get_track`
// (the Qobuz catalog `Track`) and maps it to a `QueueTrack` with
// `track_to_queue_track` below — the same shape the desktop's single-track
// play path builds server-side (`crates/qbz/src/playback.rs:2028-2073`,
// off-limits here since `qbz` is the Slint crate; this is an independent,
// Slint-free re-derivation from `qbz_models::Track`, not a copy of that file).
//
// Response shape note on `add`: 02 §3.3.14's sketch is `{"added","total_tracks"}`.
// This handler additively includes `"tracks"`: the materialized `QueueTrack`
// objects, in request order — the same reasoning T7 used for `next`/`previous`
// returning the full landing `QueueTrack` (02 §3.3.9-10) rather than a bare id:
// the CLI's documented human line (`added: Spain – Chick Corea (next)`, §2.2)
// needs title/artist, and §1.1's one-request-per-verb rule forbids a second
// GET to fetch them. §3.1.4 explicitly allows additive fields within
// api_version 1; `total_tracks`/`added` are unchanged and still exactly what
// §3.3.14 documents.
use std::io::Cursor;

use serde_json::Value;
use tiny_http::Response;

use qbz_models::{QueueTrack, RepeatMode, Track};

use crate::state::AuthState;

use super::{err_json, json, owner_action_gate, owner_action_lease, ApiState};

/// `GET /api/queue` (02 §3.3.13). `query` is the raw query string (no leading
/// `?`) — `route()` strips it off the path before dispatch, so it is threaded
/// through separately. Reads the FULL queue state (`get_queue_state_full`,
/// not the UI-capped `get_queue_state`) so `offset`/`limit` paginate over the
/// complete `upcoming` list rather than an already-truncated 20-entry window.
///
/// ADDITIVE `history` field (§3.1.4 allows additive within api_version 1):
/// the full played-track list, recent-first (`QueueManager::get_state_full`'s
/// convention, qbz-player/src/queue.rs:1064-1070). Without it the §2.2
/// `queue list` example — a played row rendered ABOVE the current track — is
/// unreproducible from the response. `history_len` stays exactly as
/// documented (§3.3.13); nothing existing is renamed or removed.
pub fn list(state: &ApiState, query: &str) -> Response<Cursor<Vec<u8>>> {
    let (offset, limit) = parse_offset_limit(query);
    let queue = state.rt.block_on(state.runtime.core().get_queue_state_full());

    let current_track = queue
        .current_track
        .as_ref()
        .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    let upcoming: Vec<Value> = paginate(&queue.upcoming, offset, limit)
        .iter()
        .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
        .collect();
    let history: Vec<Value> = queue
        .history
        .iter()
        .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
        .collect();

    json(
        200,
        serde_json::json!({
            "current_track": current_track,
            "current_index": queue.current_index,
            "upcoming": upcoming,
            "history": history,
            "history_len": queue.history.len(),
            "shuffle": queue.shuffle,
            "repeat": repeat_str(queue.repeat),
            "total_tracks": queue.total_tracks,
            "stop_after_track_id": queue.stop_after_track_id,
            "offset": offset,
            "limit": limit,
        }),
    )
}

/// `POST /api/queue/add` (02 §3.3.14). Body `{"track_ids": [...], "position":
/// "end"|"next"}` (`position` default `"end"`). Errors: 409 `needs_auth`,
/// 404 `not_found` (any unresolvable id — resolution stops at the first
/// failure, nothing partially added), 400 `bad_request` (malformed
/// `track_ids` element or unknown `position` literal — both parses are
/// STRICT and run before any core call, so a rejected body never partially
/// mutates the queue).
pub fn add(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = owner_action_gate(state) {
        return resp;
    }
    if let Some(resp) = auth_gate(state) {
        return resp;
    }

    let track_ids = match parse_track_ids(body) {
        Ok(ids) => ids,
        Err((message, hint)) => return err_json(400, "bad_request", &message, &hint),
    };
    let position = match parse_position(body) {
        Ok(p) => p,
        Err((message, hint)) => return err_json(400, "bad_request", &message, &hint),
    };

    let mut resolved: Vec<QueueTrack> = Vec::with_capacity(track_ids.len());
    for id in &track_ids {
        match state.rt.block_on(state.runtime.core().get_track(*id)) {
            Ok(track) => resolved.push(track_to_queue_track(&track)),
            Err(_) => {
                return err_json(
                    404,
                    "not_found",
                    &format!("track {id} not found"),
                    "check the track id: qbzd search <QUERY>",
                )
            }
        }
    }

    let added = resolved.len();
    let tracks_json: Vec<Value> = resolved
        .iter()
        .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
        .collect();

    // Catalog resolution can outlive an authority handoff. Revalidate before
    // the first owner queue mutation so a guest epoch cannot inherit this
    // request merely because it began under owner authority.
    let _owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };

    if position == AddPosition::Next {
        // `add_track_next` always inserts immediately after the current
        // track, so reverse iteration is what lands multiple tracks in
        // request order (matches the desktop's multi-add-next convention).
        for track in resolved.into_iter().rev() {
            state.rt.block_on(state.runtime.core().add_track_next(track));
        }
    } else {
        state.rt.block_on(state.runtime.core().add_tracks(resolved));
    }

    let total_tracks = state.rt.block_on(state.runtime.core().get_queue_state()).total_tracks;
    json(
        200,
        serde_json::json!({"added": added, "total_tracks": total_tracks, "tracks": tracks_json}),
    )
}

/// `POST /api/queue/remove` (02 §3.3.15). Body `{"index": N}`, 0-based —
/// SAME space as `current_index` (no CLI-boundary shift here). Errors:
/// 404 `not_found` (index out of range), 400 `bad_request` (index is the
/// playing track — the verbatim hint below is quoted §3.3.15; also a
/// missing or non-integer `index` field, with distinct messages).
pub fn remove(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    let _owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let index = match parse_remove_index(body) {
        Ok(i) => i,
        Err((message, hint)) => return err_json(400, "bad_request", &message, &hint),
    };

    let queue = state.rt.block_on(state.runtime.core().get_queue_state_full());
    match check_remove_index(index, queue.total_tracks, queue.current_index) {
        RemoveCheck::OutOfRange => {
            return err_json(
                404,
                "not_found",
                &format!("queue index {index} is out of range"),
                "check: qbzd queue list",
            )
        }
        RemoveCheck::PlayingIndex => {
            return err_json(
                400,
                "bad_request",
                &format!("index {index} is the playing track"),
                "use: qbzd next, or qbzd queue clear",
            )
        }
        RemoveCheck::Ok => {}
    }

    match state.rt.block_on(state.runtime.core().remove_track(index)) {
        Some(track) => {
            let total_tracks =
                state.rt.block_on(state.runtime.core().get_queue_state()).total_tracks;
            json(200, serde_json::json!({"removed": track.id, "total_tracks": total_tracks}))
        }
        // Narrow race, acknowledged: the bounds/playing check above and this
        // mutation are two separate core calls, and while the API serving
        // thread is single-threaded, the playback driver and QConnect tasks
        // mutate the queue independently — an auto-advance (or a remote
        // command) landing between the two calls can shrink the queue or
        // shift `current_index`, so this branch IS reachable, and a remove
        // can land on what just BECAME the playing row. Both degradations
        // are benign (a 404 here; a queue edit the §3.3.15 gate was one tick
        // too late to refuse) — never a panic, never corruption
        // (`QueueManager::remove_track` re-checks bounds under its own lock).
        // TODO(converge): atomic remove-guard primitive in qbz-core (P1).
        None => err_json(
            404,
            "not_found",
            &format!("queue index {index} is out of range"),
            "check: qbzd queue list",
        ),
    }
}

/// `POST /api/queue/clear` (02 §3.3.16). Body `{"keep_current": bool}`
/// (default `true` when the field is absent — the CLI always sends it
/// explicitly, §"queue clear" in cli/queue.rs).
pub fn clear(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    let _owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let keep_current = body.get("keep_current").and_then(|v| v.as_bool()).unwrap_or(true);
    state.rt.block_on(state.runtime.core().clear_queue(keep_current));
    let total_tracks = state.rt.block_on(state.runtime.core().get_queue_state()).total_tracks;
    json(200, serde_json::json!({"total_tracks": total_tracks}))
}

/// `POST /api/queue/move` (CONSOLE). Body `{"from": N, "to": N}` (0-based).
/// GUI drag-reorder (core `move_track`). 404 when either index is out of range.
pub fn reorder(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = owner_action_gate(state) {
        return resp;
    }
    let from = match body.get("from").and_then(|v| v.as_u64()) {
        Some(n) => n as usize,
        None => return err_json(400, "bad_request", "move requires 'from' and 'to'", "body: {\"from\": 7, \"to\": 2}"),
    };
    let to = match body.get("to").and_then(|v| v.as_u64()) {
        Some(n) => n as usize,
        None => return err_json(400, "bad_request", "move requires 'from' and 'to'", "body: {\"from\": 7, \"to\": 2}"),
    };
    let _owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    if state.rt.block_on(state.runtime.core().move_track(from, to)) {
        json(200, serde_json::json!({"from": from, "to": to}))
    } else {
        err_json(404, "not_found", "queue index out of range", "check: qbzd queue list")
    }
}

/// `POST /api/queue/jump` (CONSOLE). Body `{"index": N}` (0-based). A
/// click-to-play-row: moves the cursor (`play_index`) AND starts audio through
/// the shipped ritual — never a bare cursor move (control-surface §2.2).
/// Auth-gated (needs a session to resolve the stream). The cursor move is
/// synchronous (the 404 contract is immediate); the resolve+play leg is
/// spawn-and-ack (see `playback::advance`) so a slow fetch can't starve the
/// single-threaded API — failures latch into `last_errors.stream`.
pub fn jump(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = owner_action_gate(state) {
        return resp;
    }
    if let Some(resp) = auth_gate(state) {
        return resp;
    }
    let index = match body.get("index").and_then(|v| v.as_u64()) {
        Some(n) => n as usize,
        None => return err_json(400, "bad_request", "jump requires an 'index'", "body: {\"index\": 2}"),
    };
    let owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let track = match state.rt.block_on(state.runtime.core().play_index(index)) {
        Some(t) => t,
        None => return err_json(404, "not_found", &format!("queue index {index} is out of range"), "check: qbzd queue list"),
    };
    let track_id = track.id;
    let quality = super::playback::resolve_quality(state);
    let runtime = std::sync::Arc::clone(&state.runtime);
    let shared = std::sync::Arc::clone(&state.shared);
    state.rt.spawn(async move {
        let _owner_lease = owner_lease;
        if let Err(err) = runtime
            .core()
            .play_track_resolved(track_id, quality, None, None, 0)
            .await
        {
            log::error!("[api] queue jump play of {track_id} failed: {err}");
            if let Ok(mut s) = shared.lock() {
                s.last_errors.stream = Some(format!("jump: {err}"));
            }
            return;
        }
        qbz_app::playback_driver::save_session_now(runtime.as_ref()).await;
    });
    json(
        200,
        serde_json::json!({"playing": index, "track": {"id": track.id, "title": track.title, "artist": track.artist}}),
    )
}

/// `POST /api/queue/stop-after` (CONSOLE). Body `{"track_id": N}` |
/// `{"current": true}` | `{"off": true}`. Sets/clears the stop-after gate
/// (core `set_stop_after`/`clear_stop_after`).
pub fn stop_after(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    let _owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    if body.get("off").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.rt.block_on(state.runtime.core().clear_stop_after());
        return json(200, serde_json::json!({"stop_after_track_id": Value::Null}));
    }
    let track_id = if body.get("current").and_then(|v| v.as_bool()).unwrap_or(false) {
        match state.rt.block_on(state.runtime.core().get_queue_state()).current_track {
            Some(t) => t.id,
            None => return err_json(404, "not_found", "nothing is playing", "queue a track first"),
        }
    } else if let Some(id) = body.get("track_id").and_then(|v| v.as_u64()) {
        id
    } else {
        return err_json(
            400,
            "bad_request",
            "stop-after requires track_id, current, or off",
            "body: {\"current\": true} | {\"track_id\": N} | {\"off\": true}",
        );
    };
    state.rt.block_on(state.runtime.core().set_stop_after(track_id));
    json(200, serde_json::json!({"stop_after_track_id": track_id}))
}

// ============================ internals ============================

/// 409 `needs_auth` — only `add` gates (§3.3.14's Errors column; `list`/
/// `remove`/`clear` carry no needs_auth entry and act on whatever queue
/// already exists). Mirrors `playback::auth_gate` exactly; kept local per
/// this file's self-contained-helpers convention (see api/status.rs's own
/// `bitperfect_label`/`backend_label`).
fn auth_gate(state: &ApiState) -> Option<Response<Cursor<Vec<u8>>>> {
    let needs_auth = state
        .shared
        .lock()
        .map(|s| s.auth == AuthState::NeedsAuth)
        .unwrap_or(false);
    if needs_auth {
        Some(err_json(409, "needs_auth", "not logged in to Qobuz", "run: qbzd login"))
    } else {
        None
    }
}

/// Strict `track_ids` parse — ANY non-u64 element is a 400 naming the
/// offending (0-based JSON array) position. The previous `filter_map` parse
/// silently dropped malformed elements: "add 3, enqueue 2" is data loss the
/// caller never sees. Runs BEFORE any core call, so a rejected body leaves
/// the queue untouched.
fn parse_track_ids(body: &Value) -> Result<Vec<u64>, (String, String)> {
    let hint = "body: {\"track_ids\": [176544872]}".to_string();
    let arr = match body.get("track_ids").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Err(("add requires a 'track_ids' array".into(), hint)),
    };
    if arr.is_empty() {
        return Err(("'track_ids' must not be empty".into(), hint));
    }
    let mut ids = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        match v.as_u64() {
            Some(id) => ids.push(id),
            None => {
                return Err((
                    format!("track_ids[{i}] is not an unsigned integer track id"),
                    hint,
                ))
            }
        }
    }
    Ok(ids)
}

/// `position` for `add` — strict literal match (§3.3.14: `"end"|"next"`,
/// default `"end"` when the field is absent). Anything else — an unknown
/// literal, or a non-string — is a 400, never a silent fall-through to
/// "end" (a typo'd `"nxet"` appending to the queue tail instead of playing
/// next would read as broken).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddPosition {
    End,
    Next,
}

fn parse_position(body: &Value) -> Result<AddPosition, (String, String)> {
    let hint = "body: {\"track_ids\": [176544872], \"position\": \"next\"}".to_string();
    match body.get("position") {
        None => Ok(AddPosition::End),
        Some(v) => match v.as_str() {
            Some("end") => Ok(AddPosition::End),
            Some("next") => Ok(AddPosition::Next),
            _ => Err((format!("invalid position {v} — use \"end\" or \"next\""), hint)),
        },
    }
}

/// `index` for `remove` — distinct messages for a MISSING field vs a
/// present-but-wrong-type one (a string or negative `index` is not "you
/// forgot the field"; §1.4 error voice wants the message to name the actual
/// fault).
fn parse_remove_index(body: &Value) -> Result<usize, (String, String)> {
    let hint = "body: {\"index\": 3}".to_string();
    match body.get("index") {
        None => Err(("remove requires an 'index' field".into(), hint)),
        Some(v) => match v.as_u64() {
            Some(i) => Ok(i as usize),
            None => Err(("'index' must be a non-negative integer".into(), hint)),
        },
    }
}

/// The pure remove-index decision (brief step 1: "the remove-playing-index
/// rejection body"). Bounds-check wins over the playing-index check when
/// both would fire (an out-of-range index cannot simultaneously BE the
/// playing index, so order does not matter in practice — bounds first reads
/// more naturally as "does this index exist at all").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoveCheck {
    Ok,
    OutOfRange,
    PlayingIndex,
}

fn check_remove_index(index: usize, total_tracks: usize, current_index: Option<usize>) -> RemoveCheck {
    if index >= total_tracks {
        return RemoveCheck::OutOfRange;
    }
    if current_index == Some(index) {
        return RemoveCheck::PlayingIndex;
    }
    RemoveCheck::Ok
}

/// Pure pagination slicing (brief step 1) — `GET /api/queue`'s `offset`/
/// `limit` applied to the full `upcoming` list.
fn paginate(items: &[QueueTrack], offset: usize, limit: usize) -> Vec<QueueTrack> {
    items.iter().skip(offset).take(limit).cloned().collect()
}

/// `?offset=0&limit=100` (02 §3.3.13). Malformed/missing values fall back to
/// the documented defaults; there is no 400 for a bad query param (a read
/// route degrades gracefully rather than failing the request).
fn parse_offset_limit(query: &str) -> (usize, usize) {
    let mut offset = 0usize;
    let mut limit = 100usize;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let val = kv.next().unwrap_or("");
        match key {
            "offset" => {
                if let Ok(n) = val.parse() {
                    offset = n;
                }
            }
            "limit" => {
                if let Ok(n) = val.parse() {
                    limit = n;
                }
            }
            _ => {}
        }
    }
    (offset, limit)
}

fn repeat_str(mode: RepeatMode) -> String {
    match mode {
        RepeatMode::Off => "off",
        RepeatMode::All => "all",
        RepeatMode::One => "one",
    }
    .to_string()
}

/// Qobuz catalog `Track` (`crates/qbz-models/src/types.rs:301-342`, what
/// `core.get_track` returns) -> `QueueTrack` (`crates/qbz-models/src/
/// playback.rs:15`, what the queue stores). An independent Slint-free
/// re-derivation of the same mapping the desktop's single-track play path
/// performs (`crates/qbz/src/playback.rs:2028-2073`) and `qbz-mixtape`'s
/// `track_to_queue_track_from_api` (`crates/qbz-mixtape/src/enqueue.rs:
/// 430-472`) — `qbzd` cannot depend on `qbz` (Slint) and this task's file
/// list does not add a new workspace dependency, so the ~20-line mapping is
/// duplicated rather than imported. `source_item_id_hint`/`context_kind`/
/// `context_id` are left `None`: those are "playing from" provenance fields
/// with no equivalent in a bare `qbzd queue add <TRACK_ID>` call (no album/
/// playlist/artist container in play).
pub(crate) fn track_to_queue_track(track: &Track) -> QueueTrack {
    let artwork_url = track.album.as_ref().and_then(|a| a.image.best().cloned());
    let artist = track
        .performer
        .as_ref()
        .map(|p| p.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let album = track
        .album
        .as_ref()
        .map(|a| a.title.clone())
        .unwrap_or_else(|| "Unknown Album".to_string());
    let album_id = track.album.as_ref().map(|a| a.id.clone());
    let artist_id = track.performer.as_ref().map(|p| p.id);

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
        album_id,
        artist_id,
        // `QueueTrack.streamable` is a resolved `bool`; absence is interpreted
        // once, here, by `Track::is_streamable` (absent = available).
        streamable: track.is_streamable(),
        source: Some("qobuz".to_string()),
        parental_warning: track.parental_warning,
        source_item_id_hint: None,
        context_kind: None,
        context_id: None,
        isrc: None,
        recording_mbid: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track(id: u64) -> QueueTrack {
        QueueTrack {
            id,
            title: format!("Track {id}"),
            version: None,
            artist: "Chick Corea".into(),
            album: "Light as a Feather".into(),
            album_version: None,
            duration_secs: 300,
            artwork_url: None,
            hires: true,
            bit_depth: Some(24),
            sample_rate: Some(96.0),
            is_local: false,
            album_id: Some("0060253776847".into()),
            artist_id: Some(123206),
            streamable: true,
            source: Some("qobuz".into()),
            parental_warning: false,
            source_item_id_hint: None,
            context_kind: None,
            context_id: None,
            isrc: None,
            recording_mbid: None,
        }
    }

    // ---------------------------- pagination ----------------------------

    #[test]
    fn paginate_slices_offset_and_limit() {
        let items: Vec<QueueTrack> = (0..5).map(sample_track).collect();
        let page = paginate(&items, 1, 2);
        assert_eq!(page.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn paginate_offset_past_end_is_empty() {
        let items: Vec<QueueTrack> = (0..3).map(sample_track).collect();
        assert!(paginate(&items, 10, 5).is_empty());
    }

    #[test]
    fn paginate_limit_larger_than_remaining_returns_the_rest() {
        let items: Vec<QueueTrack> = (0..3).map(sample_track).collect();
        let page = paginate(&items, 1, 100);
        assert_eq!(page.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn parse_offset_limit_defaults_when_absent() {
        assert_eq!(parse_offset_limit(""), (0, 100));
        assert_eq!(parse_offset_limit("foo=bar"), (0, 100));
    }

    #[test]
    fn parse_offset_limit_reads_both_params() {
        assert_eq!(parse_offset_limit("offset=5&limit=10"), (5, 10));
        assert_eq!(parse_offset_limit("limit=10&offset=5"), (5, 10));
    }

    #[test]
    fn parse_offset_limit_ignores_malformed_values() {
        assert_eq!(parse_offset_limit("offset=nope&limit=10"), (0, 10));
    }

    // -------------------------- strict body parsing --------------------------

    #[test]
    fn parse_track_ids_accepts_a_valid_array() {
        let body = serde_json::json!({"track_ids": [176544872, 176544873]});
        assert_eq!(parse_track_ids(&body), Ok(vec![176544872, 176544873]));
    }

    #[test]
    fn parse_track_ids_rejects_a_mixed_valid_body_naming_the_position() {
        // Review fix: `filter_map` silently dropped the malformed element —
        // "add 3, enqueue 2". Now the WHOLE body is refused (400) before any
        // core call, so the queue stays untouched (`add` parses first,
        // resolves/mutates only on Ok).
        let body = serde_json::json!({"track_ids": [176544872, "oops", 176544873]});
        let (message, _hint) = parse_track_ids(&body).unwrap_err();
        assert_eq!(message, "track_ids[1] is not an unsigned integer track id");
    }

    #[test]
    fn parse_track_ids_rejects_negative_and_fractional_ids() {
        let neg = serde_json::json!({"track_ids": [-1]});
        assert!(parse_track_ids(&neg).is_err());
        let frac = serde_json::json!({"track_ids": [1.5]});
        assert!(parse_track_ids(&frac).is_err());
    }

    #[test]
    fn parse_track_ids_rejects_an_empty_array() {
        let body = serde_json::json!({"track_ids": []});
        let (message, _hint) = parse_track_ids(&body).unwrap_err();
        assert_eq!(message, "'track_ids' must not be empty");
    }

    #[test]
    fn parse_track_ids_rejects_a_missing_or_non_array_field() {
        assert!(parse_track_ids(&serde_json::json!({})).is_err());
        assert!(parse_track_ids(&serde_json::json!({"track_ids": 176544872})).is_err());
        assert!(parse_track_ids(&Value::Null).is_err());
    }

    #[test]
    fn parse_position_defaults_end_and_matches_the_two_literals() {
        assert_eq!(parse_position(&serde_json::json!({})), Ok(AddPosition::End));
        assert_eq!(
            parse_position(&serde_json::json!({"position": "end"})),
            Ok(AddPosition::End)
        );
        assert_eq!(
            parse_position(&serde_json::json!({"position": "next"})),
            Ok(AddPosition::Next)
        );
    }

    #[test]
    fn parse_position_rejects_unknown_literals_and_non_strings() {
        // Strict match (review fix): a typo must not silently become "end".
        let (message, _hint) =
            parse_position(&serde_json::json!({"position": "nxet"})).unwrap_err();
        assert_eq!(message, "invalid position \"nxet\" — use \"end\" or \"next\"");
        assert!(parse_position(&serde_json::json!({"position": 3})).is_err());
        assert!(parse_position(&serde_json::json!({"position": null})).is_err());
    }

    #[test]
    fn parse_remove_index_distinguishes_missing_from_wrong_type() {
        // Review fix: a present-but-wrong-type `index` gets its own message,
        // not the missing-field copy.
        let (missing, _) = parse_remove_index(&serde_json::json!({})).unwrap_err();
        assert_eq!(missing, "remove requires an 'index' field");
        let (wrong, _) = parse_remove_index(&serde_json::json!({"index": "3"})).unwrap_err();
        assert_eq!(wrong, "'index' must be a non-negative integer");
        let (neg, _) = parse_remove_index(&serde_json::json!({"index": -1})).unwrap_err();
        assert_eq!(neg, "'index' must be a non-negative integer");
        assert_eq!(parse_remove_index(&serde_json::json!({"index": 3})), Ok(3));
    }

    // -------------------------- remove-index gate --------------------------

    #[test]
    fn check_remove_index_ok_for_a_non_playing_in_range_index() {
        assert_eq!(check_remove_index(2, 14, Some(1)), RemoveCheck::Ok);
    }

    #[test]
    fn check_remove_index_rejects_out_of_range() {
        assert_eq!(check_remove_index(14, 14, Some(1)), RemoveCheck::OutOfRange);
        assert_eq!(check_remove_index(100, 14, None), RemoveCheck::OutOfRange);
    }

    #[test]
    fn check_remove_index_rejects_the_playing_index() {
        // 02 §3.3.15's own example state: current_index=1, removing index 1.
        assert_eq!(check_remove_index(1, 14, Some(1)), RemoveCheck::PlayingIndex);
    }

    #[test]
    fn remove_playing_index_error_body_matches_the_documented_hint() {
        // 02 §3.3.15 verbatim: {"error":{"code":"bad_request",
        // "message":"index 1 is the playing track","hint":"use: qbzd next, or qbzd queue clear"}}
        let body = super::super::error_body(
            "bad_request",
            "index 1 is the playing track",
            "use: qbzd next, or qbzd queue clear",
        );
        assert_eq!(body["error"]["code"], "bad_request");
        assert_eq!(body["error"]["message"], "index 1 is the playing track");
        assert_eq!(body["error"]["hint"], "use: qbzd next, or qbzd queue clear");
    }

    // ------------------------------ misc ------------------------------

    #[test]
    fn repeat_str_matches_contract_lowercase() {
        assert_eq!(repeat_str(RepeatMode::Off), "off");
        assert_eq!(repeat_str(RepeatMode::All), "all");
        assert_eq!(repeat_str(RepeatMode::One), "one");
    }

    #[test]
    fn track_to_queue_track_maps_the_catalog_shape() {
        let mut track = Track {
            id: 176544872,
            title: "500 Miles High".into(),
            duration: 547,
            hires: true,
            maximum_bit_depth: Some(24),
            maximum_sampling_rate: Some(96.0),
            // The catalog shape reports availability explicitly here — that is
            // what `/album/get` sends. `Some(true)`, not a bare `true`, since
            // the model distinguishes "the server said yes" from "the server
            // said nothing".
            streamable: Some(true),
            parental_warning: false,
            ..Default::default()
        };
        track.performer = Some(qbz_models::Artist {
            id: 123206,
            name: "Chick Corea".into(),
            ..Default::default()
        });
        track.album = Some(qbz_models::AlbumSummary {
            id: "0060253776847".into(),
            title: "Light as a Feather".into(),
            image: qbz_models::ImageSet {
                large: Some("https://static.qobuz.com/large.jpg".into()),
                ..Default::default()
            },
            label: None,
            genre: None,
            artist: None,
            version: None,
            streamable: None,
            streamable_at: None,
            release_date_stream: None,
        });

        let qt = track_to_queue_track(&track);
        assert_eq!(qt.id, 176544872);
        assert_eq!(qt.title, "500 Miles High");
        assert_eq!(qt.artist, "Chick Corea");
        assert_eq!(qt.album, "Light as a Feather");
        assert_eq!(qt.duration_secs, 547);
        assert_eq!(qt.album_id.as_deref(), Some("0060253776847"));
        assert_eq!(qt.artist_id, Some(123206));
        assert_eq!(qt.source.as_deref(), Some("qobuz"));
        assert_eq!(qt.artwork_url.as_deref(), Some("https://static.qobuz.com/large.jpg"));
    }

    #[test]
    fn track_to_queue_track_falls_back_when_performer_and_album_are_absent() {
        let track = Track {
            id: 1,
            title: "Untitled".into(),
            ..Default::default()
        };
        let qt = track_to_queue_track(&track);
        assert_eq!(qt.artist, "Unknown Artist");
        assert_eq!(qt.album, "Unknown Album");
        assert!(qt.album_id.is_none());
        assert!(qt.artwork_url.is_none());
    }
}
