// crates/qbzd/src/api/play.rs — POST /api/play (02-cli-and-api.md §2.3/§3.4
// row 23, P1). The headline "originate playback" verb: qbzd resolves a piece
// of content — a track, album, playlist, artist, or a Qobuz URL — materializes
// it into the queue server-side, and starts audio through the SHIPPED driver
// ritual. This is what turns the daemon from a receiver into a source.
//
// Materialization is server-side and never trusts a client-built QueueTrack
// (same discipline as queue::add): each Track comes from the core
// (get_album / get_playlist / get_artist_tracks / get_tracks_batch) and is
// mapped by the shared queue::track_to_queue_track. Audio ALWAYS starts through
// core.play_track_resolved + save_session_now — the exact cold-start tail
// playback::cold_start uses — never a bare cursor move (control-surface §2.2);
// the protected qbz-player/qbz-audio crates are untouched.
//
// Body: one of {"track_id": N} | {"album_id": "..."} | {"playlist_id": N} |
// {"artist_id": N} | {"url": "https://open.qobuz.com/..."}, plus optional
// {"index": N} (0-based start position within the resolved list). A URL wins
// over the id fields and is resolved to one of the id kinds first. Errors:
// 409 needs_auth, 400 bad_request (no selector / bad URL), 404 not_found
// (unknown id / empty resolution), 503 audio_unavailable (start failed).
use std::io::Cursor;

use serde_json::Value;
use tiny_http::Response;

use qbz_models::{QueueTrack, Track};
use qbz_qobuz::link_resolver::{resolve_link, ResolvedLink};

use crate::state::AuthState;

use super::queue::track_to_queue_track;
use super::{err_json, json, owner_action_gate, owner_action_lease, ApiState};

/// Top-tracks cap when playing a whole artist — a sane "play this artist" set,
/// not their entire catalogue.
const ARTIST_TOP_LIMIT: u32 = 50;

pub fn play(state: &ApiState, body: &Value) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = owner_action_gate(state) {
        return resp;
    }
    if let Some(resp) = auth_gate(state) {
        return resp;
    }

    let selector = match parse_selector(body) {
        Ok(s) => s,
        Err((message, hint)) => return err_json(400, "bad_request", &message, &hint),
    };

    let (tracks, context) = match fetch_tracks(state, &selector) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    start_resolved(state, tracks, context, body.get("index").and_then(|v| v.as_u64()))
}

/// Materialize resolved catalog `tracks` into the queue and cold-start the
/// chosen one through the SHIPPED ritual (set_queue → play_track_resolved →
/// save_session_now) — never a bare cursor move (control-surface §2.2). The
/// protected qbz-player/qbz-audio crates are untouched. `context` stamps
/// "playing from" provenance (context_kind ∈ album|artist|playlist —
/// qbz-models/src/playback.rs:60-71); `start_index` is the 0-based start
/// position. Shared by `play` (selector-resolved) and `radio` (seed-generated).
pub(crate) fn start_resolved(
    state: &ApiState,
    tracks: Vec<Track>,
    context: Option<(&'static str, String)>,
    start_index: Option<u64>,
) -> Response<Cursor<Vec<u8>>> {
    if let Some(resp) = owner_action_gate(state) {
        return resp;
    }
    if tracks.is_empty() {
        return err_json(404, "not_found", "nothing to play", "check the id: qbzd search <QUERY>");
    }

    let mut queue_tracks: Vec<QueueTrack> = tracks.iter().map(track_to_queue_track).collect();
    if let Some((kind, id)) = &context {
        for qt in &mut queue_tracks {
            qt.context_kind = Some((*kind).to_string());
            qt.context_id = Some(id.clone());
        }
    }

    let total = queue_tracks.len();
    let start = clamp_index(start_index, total);
    let start_track_id = queue_tracks[start].id;
    let start_summary = summary(&queue_tracks[start]);

    // `play` and `radio` both perform owner catalog I/O before reaching this
    // point. A handoff may commit during that I/O, so fence again immediately
    // before replacing the queue.
    let owner_lease = match owner_action_lease(state) {
        Ok(lease) => lease,
        Err(response) => return response,
    };

    state
        .rt
        .block_on(state.runtime.core().set_queue(queue_tracks, Some(start)));

    let quality = super::playback::resolve_quality(state);
    // Spawn-and-ack (see `playback::advance`): set_queue is cheap and stays
    // synchronous; the resolve+play leg runs on the tokio runtime so a slow
    // fetch can't starve the single-threaded API. A load failure latches into
    // `last_errors.stream` (visible via `qbzd status`) instead of a 503.
    let runtime = std::sync::Arc::clone(&state.runtime);
    let shared = std::sync::Arc::clone(&state.shared);
    state.rt.spawn(async move {
        let _owner_lease = owner_lease;
        let played = runtime
            .core()
            .play_track_resolved(start_track_id, quality, None, None, 0)
            .await;
        if let Err(err) = played {
            log::error!("[api] play start of {start_track_id} failed: {err}");
            if let Ok(mut s) = shared.lock() {
                s.last_errors.stream = Some(format!("play: {err}"));
            }
            return;
        }
        qbz_app::playback_driver::save_session_now(runtime.as_ref()).await;
    });

    json(
        200,
        serde_json::json!({
            "queued": total,
            "started": true,
            "index": start,
            "track": start_summary,
        }),
    )
}

// ============================ internals ============================

/// What to play, after a URL (if any) has been resolved to an id kind.
enum Selector {
    Track(u64),
    Album(String),
    Playlist(u64),
    Artist(u64),
}

/// A URL wins over the id fields (§3.4 row 23: "URL resolved server-side"),
/// resolved via the pure `resolve_link` (qbz-qobuz/src/link_resolver.rs:50).
/// Otherwise the first present id field is used.
fn parse_selector(body: &Value) -> Result<Selector, (String, String)> {
    let hint = "body: {\"album_id\":\"...\"} | {\"track_id\":N} | {\"playlist_id\":N} | \
                {\"artist_id\":N} | {\"url\":\"https://open.qobuz.com/...\"}"
        .to_string();

    if let Some(url) = body.get("url").and_then(|v| v.as_str()) {
        return match resolve_link(url) {
            Ok(ResolvedLink::OpenAlbum(id)) => Ok(Selector::Album(id)),
            Ok(ResolvedLink::OpenTrack(id)) => Ok(Selector::Track(id)),
            Ok(ResolvedLink::OpenArtist(id)) => Ok(Selector::Artist(id)),
            Ok(ResolvedLink::OpenPlaylist(id)) => Ok(Selector::Playlist(id)),
            Err(_) => Err((
                format!("unrecognized Qobuz URL: {url}"),
                "expected an open.qobuz.com album/track/artist/playlist link".into(),
            )),
        };
    }
    if let Some(id) = body.get("track_id").and_then(|v| v.as_u64()) {
        return Ok(Selector::Track(id));
    }
    if let Some(id) = body.get("album_id").and_then(|v| v.as_str()) {
        return Ok(Selector::Album(id.to_string()));
    }
    if let Some(id) = body.get("playlist_id").and_then(|v| v.as_u64()) {
        return Ok(Selector::Playlist(id));
    }
    if let Some(id) = body.get("artist_id").and_then(|v| v.as_u64()) {
        return Ok(Selector::Artist(id));
    }
    Err(("play requires a content selector".into(), hint))
}

/// Resolve the selector to catalog tracks + optional (context_kind, context_id)
/// provenance. A single track carries no container provenance (None).
#[allow(clippy::type_complexity)]
fn fetch_tracks(
    state: &ApiState,
    selector: &Selector,
) -> Result<(Vec<Track>, Option<(&'static str, String)>), Response<Cursor<Vec<u8>>>> {
    match selector {
        Selector::Track(id) => match state.rt.block_on(state.runtime.core().get_tracks_batch(&[*id])) {
            Ok(tracks) => Ok((tracks, None)),
            Err(_) => Err(not_found("track", &id.to_string())),
        },
        Selector::Album(id) => match state.rt.block_on(state.runtime.core().get_album(id)) {
            Ok(album) => {
                let items = album.tracks.map(|t| t.items).unwrap_or_default();
                Ok((items, Some(("album", id.clone()))))
            }
            Err(_) => Err(not_found("album", id)),
        },
        Selector::Playlist(id) => match state.rt.block_on(state.runtime.core().get_playlist(*id)) {
            Ok(pl) => {
                let items = pl.tracks.map(|t| t.items).unwrap_or_default();
                Ok((items, Some(("playlist", id.to_string()))))
            }
            Err(_) => Err(not_found("playlist", &id.to_string())),
        },
        Selector::Artist(id) => {
            match state.rt.block_on(state.runtime.core().get_artist_tracks(*id, ARTIST_TOP_LIMIT, 0)) {
                Ok(tc) => Ok((tc.items, Some(("artist", id.to_string())))),
                Err(_) => Err(not_found("artist", &id.to_string())),
            }
        }
    }
}

fn not_found(kind: &str, id: &str) -> Response<Cursor<Vec<u8>>> {
    err_json(
        404,
        "not_found",
        &format!("{kind} {id} not found"),
        "check the id: qbzd search <QUERY>",
    )
}

/// Clamp the requested 0-based start index into the resolved list; default 0.
fn clamp_index(idx: Option<u64>, len: usize) -> usize {
    match idx {
        Some(i) => (i as usize).min(len.saturating_sub(1)),
        None => 0,
    }
}

/// A compact now-playing summary for the response (title/artist for the CLI's
/// human line; the full state is one `qbzd now` away — §1.1 one-request rule
/// is satisfied because this rides the same response).
fn summary(qt: &QueueTrack) -> Value {
    serde_json::json!({
        "id": qt.id,
        "title": qt.title,
        "artist": qt.artist,
        "album": qt.album,
    })
}

/// 409 `needs_auth` — play needs a live Qobuz session (materialization calls
/// the client). Self-contained per-file helper (this crate's convention).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_selector_reads_each_id_field() {
        assert!(matches!(
            parse_selector(&serde_json::json!({"track_id": 42})).unwrap(),
            Selector::Track(42)
        ));
        assert!(matches!(
            parse_selector(&serde_json::json!({"album_id": "abc"})).unwrap(),
            Selector::Album(ref s) if s == "abc"
        ));
        assert!(matches!(
            parse_selector(&serde_json::json!({"playlist_id": 7})).unwrap(),
            Selector::Playlist(7)
        ));
        assert!(matches!(
            parse_selector(&serde_json::json!({"artist_id": 9})).unwrap(),
            Selector::Artist(9)
        ));
    }

    #[test]
    fn parse_selector_resolves_a_qobuz_url_and_it_wins_over_ids() {
        // URL resolves to a kind, and it takes precedence over a stray id field.
        let body = serde_json::json!({
            "url": "https://open.qobuz.com/album/0060254728933",
            "track_id": 42
        });
        assert!(matches!(
            parse_selector(&body).unwrap(),
            Selector::Album(ref s) if s == "0060254728933"
        ));
    }

    #[test]
    fn parse_selector_rejects_a_bad_url_and_a_missing_selector() {
        assert!(parse_selector(&serde_json::json!({"url": "https://example.com/x"})).is_err());
        assert!(parse_selector(&serde_json::json!({})).is_err());
    }

    #[test]
    fn clamp_index_defaults_zero_and_clamps_into_range() {
        assert_eq!(clamp_index(None, 5), 0);
        assert_eq!(clamp_index(Some(2), 5), 2);
        assert_eq!(clamp_index(Some(99), 5), 4);
        assert_eq!(clamp_index(Some(0), 0), 0);
    }
}
