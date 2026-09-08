// crates/qbzd/src/api/radio.rs — POST /api/radio (02-cli-and-api.md §3.4 row
// 25, P1). Seed-and-go: from an artist, track, or album seed, generate a Qobuz
// radio track list and start playing it.
//
// qobuz mode only in this slice (get_radio_artist/track/album — zero local
// state, ships first). The smart local-pool engine (create_smart_artist_radio)
// is gated on threading the daemon's data root into RadioDb::open_at (a shared-
// core change, deferred — memo D2). Reuses play::start_resolved for the
// queue-replace + cold-start ritual, so the protected audio path is untouched
// exactly as `play` leaves it.
use std::io::Cursor;

use serde_json::Value;
use tiny_http::Response;

use qbz_models::Track;

use crate::state::AuthState;

use super::{err_json, owner_action_gate, ApiState};

/// `POST /api/radio`. Body: one of `{"artist_id": N}` | `{"track_id": N}` |
/// `{"album_id": "..."}`. Errors: 409 needs_auth, 400 bad_request (no seed),
/// 502 radio_failed (upstream), 404/503 from `start_resolved` (empty / start).
pub fn radio(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = owner_action_gate(state) {
        return resp;
    }
    if let Some(resp) = auth_gate(state) {
        return resp;
    }
    let tracks = match fetch_radio(state, body) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    // Radio has no container-provenance kind in the album|artist|playlist set,
    // so no context stamp; always starts from the top of the generated list.
    super::play::start_resolved(state, tracks, None, None)
}

// ============================ internals ============================

fn fetch_radio(state: &ApiState, body: &Value) -> Result<Vec<Track>, Response<Cursor<Vec<u8>>>> {
    let radio = if let Some(id) = body.get("artist_id").and_then(|v| v.as_u64()) {
        state.rt.block_on(state.runtime.core().get_radio_artist(&id.to_string()))
    } else if let Some(id) = body.get("track_id").and_then(|v| v.as_u64()) {
        state.rt.block_on(state.runtime.core().get_radio_track(&id.to_string()))
    } else if let Some(id) = body.get("album_id").and_then(|v| v.as_str()) {
        state.rt.block_on(state.runtime.core().get_radio_album(id))
    } else {
        return Err(err_json(
            400,
            "bad_request",
            "radio requires a seed",
            "body: {\"artist_id\":N} | {\"track_id\":N} | {\"album_id\":\"...\"}",
        ));
    };
    match radio {
        Ok(r) => Ok(r.tracks.items),
        Err(_) => Err(err_json(
            502,
            "radio_failed",
            "radio request to Qobuz failed",
            "try again in a moment; check: qbzd status",
        )),
    }
}

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
