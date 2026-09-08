// crates/qbzd/src/cli/queue.rs — the `queue list/add/remove/clear` verbs
// (02-cli-and-api.md §2.2). Each is exactly one HTTP request (§1.1); the pure
// index-translation and rendering helpers below are unit-tested without a
// running daemon.
//
// INDEX CONVENTION (normative, cross-doc-fixed, §3.3.13/§3.3.15/§2.2 queue
// remove note): the wire is 0-based everywhere (the same space as
// `GET /api/queue`'s `current_index`). This file is the ONLY place a 1-based
// position exists — `queue list`'s `#` column and `queue remove <INDEX>`'s
// argument. The translation happens exactly at this boundary:
//   - display: `cli_position(api_index_0based) -> 1-based` (used by the
//     `queue list` table renderer).
//   - input: `cli_index_to_api(cli_position_1based) -> 0-based, Result` (used
//     by `queue remove`; position 0 is a usage error — there is no "0th"
//     track).
use serde_json::Value;

use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

// ============================ list ============================

/// `qbzd queue list [--json]` — renders `GET /api/queue` (§2.2/§3.3.13).
/// Exit: 0 · 1 · 3 (no needs_auth in `list`'s Errors column, §3.3.13).
pub async fn list(host: Option<String>, json: bool, roots: &ProfileRoots) -> i32 {
    let client = ApiClient::new(host, roots);
    match client.get("/api/queue").await {
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&v).unwrap_or_default());
            } else {
                print!("{}", render_queue_list(&v));
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// The §2.2 table header, verbatim (column starts: `#`@4, `track`@7,
/// `artist`@47, `len` right-aligned in the trailing 4-wide field).
const HEADER: &str = "    #  track                                   artist            len";

/// How many played rows render above the current track. A render cap only —
/// the wire `history` field is the full list (the §2.2 example shows one
/// played row; three keeps a long session's table from being mostly past).
const HISTORY_RENDER_CAP: usize = 3;

/// The §2.2 `queue list` table: up to [`HISTORY_RENDER_CAP`] played rows
/// (from the response's additive `history` field, recent-first on the wire,
/// rendered oldest-first), the current track marked `->`, then `upcoming` —
/// all numbered with the 1-based display position via `cli_position`.
///
/// History-row numbering is the linear-play reconstruction: positions count
/// back from the current track (`current - 1`, `current - 2`, …), which is
/// exact in the §2.2 example's sequential case. Under shuffle or after
/// index jumps a played track's TRUE absolute position is not derivable
/// from the wire shape (history entries carry no index), so those rows'
/// numbers are best-effort context — `queue remove` against them stays
/// safe: the daemon re-validates every index server-side (§3.3.15).
///
/// When nothing is current (`current_index: null`), `upcoming` already
/// holds the ENTIRE queue (`QueueManager::get_state_full`,
/// qbz-player/src/queue.rs:1036-1082), numbering starts at 1, and no
/// history rows render (there is no current row to anchor them to).
fn render_queue_list(v: &Value) -> String {
    let current_index = v.get("current_index").and_then(|i| i.as_u64()).map(|i| i as usize);
    let total = v.get("total_tracks").and_then(|t| t.as_u64()).unwrap_or(0);
    let shuffle = v.get("shuffle").and_then(|s| s.as_bool()).unwrap_or(false);
    let repeat = v.get("repeat").and_then(|r| r.as_str()).unwrap_or("off");

    let mut out = String::new();
    out.push_str(HEADER);
    out.push('\n');

    let current_track = v.get("current_track").filter(|t| !t.is_null());
    if let (Some(idx), Some(track)) = (current_index, current_track) {
        let cur_pos = cli_position(idx);
        // Played rows: the `take` most recent history entries, printed
        // oldest-first so the most recent lands directly above the current
        // row, numbered backwards from it (never below position 1).
        if let Some(history) = v.get("history").and_then(|h| h.as_array()) {
            let take = HISTORY_RENDER_CAP.min(cur_pos.saturating_sub(1)).min(history.len());
            for (i, played) in history[..take].iter().rev().enumerate() {
                out.push_str(&render_row(false, cur_pos - take + i, played));
            }
        }
        out.push_str(&render_row(true, cur_pos, track));
    }

    let upcoming_start = current_index.map(|i| cli_position(i) + 1).unwrap_or(1);
    if let Some(upcoming) = v.get("upcoming").and_then(|u| u.as_array()) {
        for (i, track) in upcoming.iter().enumerate() {
            out.push_str(&render_row(false, upcoming_start + i, track));
        }
    }

    out.push_str(&format!(
        "{total} tracks · shuffle {} · repeat {repeat}\n",
        if shuffle { "on" } else { "off" }
    ));
    out
}

fn render_row(is_current: bool, position: usize, track: &Value) -> String {
    let marker = if is_current { "->" } else { "  " };
    let title = track.get("title").and_then(|t| t.as_str()).unwrap_or("");
    let artist = track.get("artist").and_then(|a| a.as_str()).unwrap_or("");
    let dur = track.get("duration_secs").and_then(|d| d.as_u64()).unwrap_or(0);
    format!(
        "{marker}{position:>3}  {title:<40}{artist:<17}{len:>4}\n",
        len = fmt_mmss(dur)
    )
}

// ============================ add ============================

/// `qbzd queue add <TRACK_ID> [--next]` -> `POST /api/queue/add` (§2.2/
/// §3.3.14). Exit: 0 · 1 · 3 · 4 (needs_auth) · 6 (unknown id).
pub async fn add(host: Option<String>, roots: &ProfileRoots, track_id: u64, next: bool) -> i32 {
    let client = ApiClient::new(host, roots);
    let position = if next { "next" } else { "end" };
    let body = serde_json::json!({"track_ids": [track_id], "position": position});
    match client.post("/api/queue/add", body).await {
        Ok(v) => {
            println!("{}", render_added(&v, next));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// `added: Spain – Chick Corea (next)` (§2.2, verbatim). The daemon
/// additively returns the materialized `tracks` (api/queue.rs's `add` doc
/// comment explains why: the documented `{"added","total_tracks"}` sketch
/// alone has no title/artist, and §1.1 forbids a second request to fetch
/// them). A response without `tracks` (an older daemon, or the batch path
/// with `track_ids.len() > 1`, which this CLI never sends) falls back to the
/// bare count.
fn render_added(v: &Value, next: bool) -> String {
    let suffix = if next { " (next)" } else { "" };
    let track = v
        .get("tracks")
        .and_then(|t| t.as_array())
        .filter(|a| a.len() == 1)
        .and_then(|a| a.first());
    match track {
        Some(t) => {
            let title = t.get("title").and_then(|x| x.as_str()).unwrap_or("");
            let artist = t.get("artist").and_then(|x| x.as_str()).unwrap_or("");
            format!("added: {title} – {artist}{suffix}")
        }
        None => {
            let added = v.get("added").and_then(|a| a.as_u64()).unwrap_or(0);
            format!("added: {added} track(s){suffix}")
        }
    }
}

// ============================ remove ============================

/// `qbzd queue remove <INDEX>` -> `POST /api/queue/remove` (§2.2/§3.3.15).
/// `index` is the 1-based position `queue list` displayed; translated to the
/// API's 0-based index at this boundary. Exit: 0 · 1 (removing the playing
/// track — server `bad_request`, not one of `error_from_envelope`'s
/// special-cased codes, §3.1.3) · 2 (position 0, local usage error) · 3 ·
/// 6 (out of range — server `not_found`).
pub async fn remove(host: Option<String>, roots: &ProfileRoots, index: usize) -> i32 {
    let api_index = match cli_index_to_api(index) {
        Ok(i) => i,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let client = ApiClient::new(host, roots);
    let body = serde_json::json!({"index": api_index});
    match client.post("/api/queue/remove", body).await {
        Ok(v) => {
            println!("{}", render_removed(&v));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// 1-based CLI position -> 0-based API index. Position 0 is a usage error
/// (there is no "0th" track) — the ONLY local validation this verb does;
/// everything else (out of range, playing track) is a server-side 404/400
/// the daemon is authoritative on.
pub fn cli_index_to_api(position: usize) -> Result<usize, String> {
    position.checked_sub(1).ok_or_else(|| {
        format!("invalid queue position '{position}' — positions start at 1 (see: qbzd queue list)")
    })
}

/// 0-based API index -> 1-based CLI display position (the inverse of
/// [`cli_index_to_api`] — used by `queue list`'s row numbering).
fn cli_position(api_index: usize) -> usize {
    api_index + 1
}

fn render_removed(v: &Value) -> String {
    let id = v.get("removed").and_then(|r| r.as_u64()).unwrap_or(0);
    let total = v.get("total_tracks").and_then(|t| t.as_u64()).unwrap_or(0);
    format!("removed: track {id} · {total} left")
}

// ============================ clear ============================

/// `qbzd queue clear [--keep-current]` -> `POST /api/queue/clear` (§2.2/
/// §3.3.16). The flag is a plain bool (clap default `false` when absent):
/// bare `queue clear` sends `keep_current: false` (a full reset — the
/// harshest reading of "clear"); `--keep-current` sends `true` (preserve the
/// now-playing track). The API's own default (`true` when the field is
/// OMITTED, §3.3.16) never applies here — this CLI always sends the field
/// explicitly. Exit: 0 · 1 · 3.
pub async fn clear(host: Option<String>, roots: &ProfileRoots, keep_current: bool) -> i32 {
    let client = ApiClient::new(host, roots);
    let body = serde_json::json!({"keep_current": keep_current});
    match client.post("/api/queue/clear", body).await {
        Ok(v) => {
            let total = v.get("total_tracks").and_then(|t| t.as_u64()).unwrap_or(0);
            println!("queue cleared · {total} left");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

// ============================ move / jump / stop-after ============================

/// `qbzd queue move <FROM> <TO>` -> `POST /api/queue/move`. FROM/TO are 1-based
/// positions (the same convention as `queue remove`), translated to 0-based at
/// this boundary. Exit: 0 · 1 · 2 (position 0) · 3 · 6 (out of range).
pub async fn move_(host: Option<String>, roots: &ProfileRoots, from: usize, to: usize) -> i32 {
    let from_i = match cli_index_to_api(from) {
        Ok(i) => i,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let to_i = match cli_index_to_api(to) {
        Ok(i) => i,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let client = ApiClient::new(host, roots);
    match client.post("/api/queue/move", serde_json::json!({"from": from_i, "to": to_i})).await {
        Ok(_) => {
            println!("moved {from} -> {to}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// `qbzd queue jump <POS>` -> `POST /api/queue/jump`. POS is 1-based; jumping
/// starts audio (a click-to-play-row). Exit: 0 · 1 · 2 · 3 · 4 · 6.
pub async fn jump(host: Option<String>, roots: &ProfileRoots, position: usize) -> i32 {
    let index = match cli_index_to_api(position) {
        Ok(i) => i,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let client = ApiClient::new(host, roots);
    match client.post("/api/queue/jump", serde_json::json!({"index": index})).await {
        Ok(v) => {
            let title = v.get("track").and_then(|t| t.get("title")).and_then(|x| x.as_str()).unwrap_or("");
            let artist = v.get("track").and_then(|t| t.get("artist")).and_then(|x| x.as_str()).unwrap_or("");
            if title.is_empty() {
                println!("jumped to {position}");
            } else {
                println!("jumped: {position}  \"{title}\" — {artist}");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

/// `qbzd queue stop-after [current|off]` (bare = current) -> `POST
/// /api/queue/stop-after`. Stops playback after the current track, or clears
/// the gate. Exit: 0 · 1 · 2 · 3.
pub async fn stop_after(host: Option<String>, roots: &ProfileRoots, arg: Option<String>) -> i32 {
    let body = match arg.as_deref() {
        None | Some("current") => serde_json::json!({"current": true}),
        Some("off") => serde_json::json!({"off": true}),
        Some(other) => {
            eprintln!("error: unknown argument '{other}'");
            eprintln!("  → usage: qbzd queue stop-after [current|off]");
            return 2;
        }
    };
    let client = ApiClient::new(host, roots);
    match client.post("/api/queue/stop-after", body).await {
        Ok(v) => {
            match v.get("stop_after_track_id").and_then(|x| x.as_u64()) {
                Some(id) => println!("stop-after set (track {id})"),
                None => println!("stop-after cleared"),
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}

// ============================ shared rendering ============================

fn fmt_mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------- index translation (both directions) --------------------

    #[test]
    fn cli_index_to_api_shifts_1based_to_0based() {
        // 02 §3.3.15's own example state: current_index=1 (0-based) IS
        // position 2 (1-based) in the §2.2 table — this is the inverse.
        assert_eq!(cli_index_to_api(1), Ok(0));
        assert_eq!(cli_index_to_api(2), Ok(1));
        assert_eq!(cli_index_to_api(14), Ok(13));
    }

    #[test]
    fn cli_index_to_api_rejects_position_zero() {
        assert!(cli_index_to_api(0).is_err());
    }

    #[test]
    fn cli_position_shifts_0based_to_1based() {
        assert_eq!(cli_position(0), 1);
        assert_eq!(cli_position(1), 2);
        assert_eq!(cli_position(13), 14);
    }

    #[test]
    fn index_translation_round_trips() {
        for api_index in 0..20usize {
            assert_eq!(cli_index_to_api(cli_position(api_index)), Ok(api_index));
        }
    }

    // -------------------------- queue list rendering --------------------------

    #[test]
    fn render_queue_list_reproduces_the_documented_example_byte_exact() {
        // 02 §2.2's documented state, verbatim: current_index=1 (0-based) ->
        // "Spain" at row #2 with the `->` marker; the played "Captain
        // Marvel" (from the additive `history` field, recent-first) at row
        // #1 above it; "500 Miles High" (the sole `upcoming` entry) at #3.
        // Every byte, including column padding, is the spec's.
        let v = serde_json::json!({
            "current_track": {"id": 176544871, "title": "Spain", "artist": "Chick Corea", "duration_secs": 581},
            "current_index": 1,
            "upcoming": [
                {"id": 176544872, "title": "500 Miles High", "artist": "Chick Corea", "duration_secs": 547}
            ],
            "history": [
                {"id": 176544870, "title": "Captain Marvel", "artist": "Chick Corea", "duration_secs": 293}
            ],
            "history_len": 1, "shuffle": false, "repeat": "off",
            "total_tracks": 14, "stop_after_track_id": null, "offset": 0, "limit": 100
        });
        let expected = concat!(
            "    #  track                                   artist            len\n",
            "    1  Captain Marvel                          Chick Corea      4:53\n",
            "->  2  Spain                                   Chick Corea      9:41\n",
            "    3  500 Miles High                          Chick Corea      9:07\n",
            "14 tracks · shuffle off · repeat off\n",
        );
        assert_eq!(render_queue_list(&v), expected);
    }

    #[test]
    fn render_queue_list_caps_history_rows_and_never_goes_below_position_one() {
        // 5 history entries, current at position 6 -> only the 3 most recent
        // render (positions 3, 4, 5 — oldest of the three first).
        let history: Vec<_> = (0..5)
            .map(|i| serde_json::json!({"id": i, "title": format!("H{i}"), "artist": "X", "duration_secs": 60}))
            .collect();
        let v = serde_json::json!({
            "current_track": {"id": 99, "title": "Cur", "artist": "X", "duration_secs": 60},
            "current_index": 5,
            "upcoming": [],
            "history": history,
            "history_len": 5, "shuffle": false, "repeat": "off",
            "total_tracks": 6, "stop_after_track_id": null, "offset": 0, "limit": 100
        });
        let rendered = render_queue_list(&v);
        // history is recent-first: H0 is the most recent -> directly above
        // the current row at position 5; H2 the oldest rendered at 3.
        assert!(rendered.contains("    3  H2"), "{rendered}");
        assert!(rendered.contains("    4  H1"), "{rendered}");
        assert!(rendered.contains("    5  H0"), "{rendered}");
        assert!(!rendered.contains("H3"), "{rendered}");
        assert!(!rendered.contains("H4"), "{rendered}");
        assert!(rendered.contains("->  6  Cur"), "{rendered}");

        // Current at position 2 with 3 history entries -> only 1 row fits
        // above it (positions never go below 1).
        let clipped = serde_json::json!({
            "current_track": {"id": 99, "title": "Cur", "artist": "X", "duration_secs": 60},
            "current_index": 1,
            "upcoming": [],
            "history": [
                {"id": 0, "title": "H0", "artist": "X", "duration_secs": 60},
                {"id": 1, "title": "H1", "artist": "X", "duration_secs": 60},
                {"id": 2, "title": "H2", "artist": "X", "duration_secs": 60}
            ],
            "history_len": 3, "shuffle": false, "repeat": "off",
            "total_tracks": 2, "stop_after_track_id": null, "offset": 0, "limit": 100
        });
        let rendered = render_queue_list(&clipped);
        assert!(rendered.contains("    1  H0"), "{rendered}");
        assert!(!rendered.contains("H1"), "{rendered}");
        assert!(rendered.contains("->  2  Cur"), "{rendered}");
    }

    #[test]
    fn render_queue_list_numbers_from_one_when_nothing_is_current() {
        let v = serde_json::json!({
            "current_track": null, "current_index": null,
            "upcoming": [
                {"id": 1, "title": "A", "artist": "X", "duration_secs": 60},
                {"id": 2, "title": "B", "artist": "Y", "duration_secs": 60}
            ],
            "history_len": 0, "shuffle": false, "repeat": "off",
            "total_tracks": 2, "stop_after_track_id": null, "offset": 0, "limit": 100
        });
        let rendered = render_queue_list(&v);
        assert!(rendered.contains("    1  A"), "{rendered}");
        assert!(rendered.contains("    2  B"), "{rendered}");
        assert!(!rendered.contains("->"), "{rendered}");
    }

    #[test]
    fn render_queue_list_empty_queue_shows_zero_tracks() {
        let v = serde_json::json!({
            "current_track": null, "current_index": null, "upcoming": [],
            "history_len": 0, "shuffle": false, "repeat": "off",
            "total_tracks": 0, "stop_after_track_id": null, "offset": 0, "limit": 100
        });
        let rendered = render_queue_list(&v);
        assert_eq!(rendered, format!("{HEADER}\n0 tracks · shuffle off · repeat off\n"));
    }

    // ------------------------------ add rendering ------------------------------

    #[test]
    fn render_added_uses_the_materialized_track_and_next_suffix() {
        let v = serde_json::json!({
            "added": 1, "total_tracks": 15,
            "tracks": [{"id": 176544872, "title": "Spain", "artist": "Chick Corea"}]
        });
        assert_eq!(render_added(&v, true), "added: Spain – Chick Corea (next)");
        assert_eq!(render_added(&v, false), "added: Spain – Chick Corea");
    }

    #[test]
    fn render_added_falls_back_to_the_bare_count_without_tracks() {
        let v = serde_json::json!({"added": 1, "total_tracks": 15});
        assert_eq!(render_added(&v, false), "added: 1 track(s)");
    }

    // ---------------------------- remove rendering ----------------------------

    #[test]
    fn render_removed_shows_id_and_remaining_count() {
        let v = serde_json::json!({"removed": 176544872, "total_tracks": 14});
        assert_eq!(render_removed(&v), "removed: track 176544872 · 14 left");
    }

    #[test]
    fn fmt_mmss_pads_seconds() {
        assert_eq!(fmt_mmss(581), "9:41");
        assert_eq!(fmt_mmss(65), "1:05");
    }
}
