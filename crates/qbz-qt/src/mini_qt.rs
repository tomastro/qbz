//! Miniplayer controller — the Slint-free half of `crates/qbz/src/miniplayer.rs`
//! that is pure data: the navigable queue projection, the prefs seeds and the
//! two pieces of arithmetic the window's verbs need (the resize guards and the
//! queue-row -> upcoming-index mapping).
//!
//! Port per the 2026-08-03 miniplayer/tray contract (§4.4.1 / §4.7, rows A-17
//! and A-18). **Plain module, NOT a bridge** — it declares no
//! `#[cxx_qt::bridge]`, so it must NOT appear in `build.rs`'s `rust_files`.
//! The QML-facing object is `QbzMini` in `mini_bridge.rs`.
//!
//! Two things this file deliberately does NOT do:
//!
//! - It never fetches queue state. `mini_queue_doc` is a pure projection of the
//!   `QueueState` the ONE existing funnel (`queue_qt::publish`) already read —
//!   contract §7-M1, "no second poll loop and no second queue fetch".
//! - It never re-implements a row helper. Titles go through
//!   `queue_qt::display_title` and durations through `queue_qt::fmt_duration`
//!   (both widened to `pub(crate)` by contract row A-36); a private copy here
//!   would be the third-copy failure `settings_qt.rs`'s accessor block exists
//!   to prevent.

use qbz_models::{QueueState, QueueTrack};
use serde::Serialize;

// --- ui_prefs.json keys (co-owned with the shipping Slint app; the typed
// struct that declares them is `crates/qbz/src/ui_prefs.rs:552-565`). Every
// write goes through `settings_qt::save_pref`'s additive single-key patch —
// a read-serialize-write of a typed struct would drop every key the Qt side
// does not know (§15 trap 16).
const KEY_SURFACE: &str = "mini_surface";
const KEY_WIDTH: &str = "mini_width";
const KEY_HEIGHT: &str = "mini_height";
const KEY_BACKGROUND_BLUR: &str = "mini_background_blur";
const KEY_DEFAULT_VIEW: &str = "mini_default_view";

/// Defaults, from `crates/qbz/src/ui_prefs.rs:677-688`.
const DEFAULT_SURFACE: i32 = 2;
const DEFAULT_WIDTH: f32 = 380.0;
const DEFAULT_HEIGHT: f32 = 540.0;

/// Resize-persistence guards and floors (`miniplayer.rs:455`, `:459`, `:461-462`).
/// **Note the two different numbers**: 320 is the mid-transition-frame filter,
/// 420 is the stored floor. Both are behavioural (§15 trap 17).
const EXPANDED_SURFACE_FLOOR: i32 = 2;
const TRANSITION_HEIGHT_FILTER: f32 = 320.0;
const MIN_PERSISTED_WIDTH: f32 = 340.0;
const MIN_PERSISTED_HEIGHT: f32 = 420.0;

/// Full-shape default for `QbzMini.miniQueueJson` — never `"{}"`, because QML
/// reads `.rows.length` in the pre-publish frame (trap 15; the coverflow
/// precedent is `queue_bridge.rs`'s `{"index":0,"tracks":[]}`).
pub(crate) const MINI_QUEUE_EMPTY: &str = r#"{"currentId":"","rows":[]}"#;

// ---------------------------------------------------------------------------
// The queue projection (contract §4.4.1, row A-17)
// ---------------------------------------------------------------------------

/// One mini queue row. Deliberately NO artwork field: the reference passes an
/// empty prior-image map (`crates/qbz/src/queue.rs:1032-1033`) and the rows
/// render a POSITION NUMBER instead — "the mini must not have MORE than the
/// main" (§15 trap 11).
#[derive(Serialize)]
struct MiniQueueRow {
    /// Decimal string, like every other projection in this crate
    /// (`QueueRow.id`, `CoverflowTrack.id`) — QML compares it against
    /// `currentId` as a string.
    id: String,
    title: String,
    artist: String,
    /// Pre-formatted `m:ss` (`queue_qt::fmt_duration`).
    duration: String,
    /// The model field is `parental_warning`; the document key is `explicit`,
    /// mirroring `QueueRow.explicit` and the reference's `item.explicit`.
    explicit: bool,
}

#[derive(Serialize)]
struct MiniQueueDoc {
    /// The row-highlight key: the reference compares `item.id` against
    /// `MiniPlayerState.current-track-id`, NOT a per-row `playing` flag
    /// (`MiniQueueSurface.slint:42`). `""` when there is no current track.
    #[serde(rename = "currentId")]
    current_id: String,
    rows: Vec<MiniQueueRow>,
}

fn row_from(track: &QueueTrack) -> MiniQueueRow {
    MiniQueueRow {
        id: track.id.to_string(),
        title: crate::queue_qt::display_title(track),
        artist: track.artist.clone(),
        duration: crate::queue_qt::fmt_duration(track.duration_secs),
        explicit: track.parental_warning,
    }
}

/// Build the miniplayer's self-contained NAVIGABLE queue document: the current
/// track first (only if there IS one), then the FULL upcoming list — not capped
/// at 20, not paginated, not de-duplicated.
///
/// 1:1 with `crate::queue::mini_queue_items` (`crates/qbz/src/queue.rs:1031-1042`).
/// The reference's own doc comment claims a de-duplication it does not perform
/// (`state.slint:4509-4510`); the port copies the CODE, not the comment
/// (contract §12-P8).
pub(crate) fn mini_queue_doc(state: &QueueState) -> String {
    let mut rows: Vec<MiniQueueRow> = Vec::with_capacity(1 + state.upcoming.len());
    if let Some(t) = state.current_track.as_ref() {
        rows.push(row_from(t));
    }
    for t in state.upcoming.iter() {
        rows.push(row_from(t));
    }
    let doc = MiniQueueDoc {
        current_id: state
            .current_track
            .as_ref()
            .map(|t| t.id.to_string())
            .unwrap_or_default(),
        rows,
    };
    serde_json::to_string(&doc).unwrap_or_else(|_| MINI_QUEUE_EMPTY.to_string())
}

// ---------------------------------------------------------------------------
// Row activation arithmetic (contract §4.4.3 — the off-by-one this port FIXES)
// ---------------------------------------------------------------------------

/// What a mini queue row activation resolves to, once the remote-first ladder
/// has declined it. Kept separate from the invokable so the mapping is
/// testable without a runtime (UT-2 is the lock on the §4.4.3 fix).
///
/// Consumed by `QbzMini.queue_play` (contract row A-13), which landed with the
/// queue surface in block B4.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MiniRowTarget {
    /// Row 0 WITH a current track: resume if paused, never restart
    /// (`miniplayer.rs:379-388`).
    Resume,
    /// A 0-based index into the UNFILTERED `upcoming` list — the domain
    /// `queue_qt::play_upcoming_flat` takes (`queue_bridge.rs:75-78`).
    /// NEVER the page-local `queue_play_upcoming` (§15 trap 10).
    Upcoming(i32),
}

/// Map a mini queue row index onto its action.
///
/// **The reference is wrong here and this port fixes it (§4.4.3).**
/// `mini_queue_items` pushes the current track only `if let Some(t)`, but the
/// reference's activation treats `idx == 0` as the current track
/// unconditionally — so with no current track, clicking row 0 (which IS
/// `upcoming[0]`) merely toggles play. Here the shift is driven by whether the
/// document actually carries a current row.
///
/// Same consumer as `MiniRowTarget` (contract row A-13).
pub(crate) fn queue_row_target(has_current: bool, row: i32) -> MiniRowTarget {
    if has_current {
        if row == 0 {
            MiniRowTarget::Resume
        } else {
            MiniRowTarget::Upcoming(row - 1)
        }
    } else {
        MiniRowTarget::Upcoming(row)
    }
}

// ---------------------------------------------------------------------------
// Geometry (contract §4.7)
// ---------------------------------------------------------------------------

/// The resize-persist decision, as a pure function so the two guards and the
/// two floors are testable without a window (UT-3).
///
/// `None` = do not persist. `Some((w, h))` = the clamped pair to store.
/// Port of `miniplayer.rs:455-463`: persist only while an EXPANDED surface is
/// shown (micro/compact sizes are constants), ignore sub-320 heights (a
/// mid-transition frame), and clamp what is stored to 340 x 420.
///
/// Consumed by `QbzMini.save_geometry` (contract row A-14), which arrived with
/// block B3's 400 ms resize debounce in `MiniWindow.qml`.
pub(crate) fn geometry_to_persist(surface: i32, width: f32, height: f32) -> Option<(f32, f32)> {
    if surface < EXPANDED_SURFACE_FLOOR || height < TRANSITION_HEIGHT_FILTER {
        return None;
    }
    Some((
        width.max(MIN_PERSISTED_WIDTH),
        height.max(MIN_PERSISTED_HEIGHT),
    ))
}

// ---------------------------------------------------------------------------
// Prefs (contract §4.7) — the seeds `QbzMiniRust::Default` reads and the
// writes the invokables make. All through the `settings_qt` accessors.
// ---------------------------------------------------------------------------

/// The surface to open on: the per-user default-view pref, or the last
/// persisted surface when that pref is `"remember"`. Pure half of
/// `resolve_initial_surface` (`miniplayer.rs:110-119`), split out so the
/// mapping is testable without touching the filesystem.
fn surface_for_default_view(default_view: &str, last_surface: i32) -> i32 {
    match default_view {
        "micro" => 0,
        "compact" => 1,
        "artwork" => 2,
        "queue" => 3,
        "lyrics" => 4,
        // "remember" / unknown
        _ => last_surface,
    }
}

/// The opening surface, UNCLAMPED — every one of the five renders.
///
/// This used to run through a `clamp_to_implemented` guard, because
/// `mini_surface` and `mini_default_view` live in the ui_prefs.json co-owned
/// with the shipping Slint app: a user whose Slint miniplayer was last on
/// Queue would have opened the Qt mini onto a blank card. B3 landed the micro
/// footer and B4 the queue and lyrics surfaces, so the clamp had no entries
/// left and went with them — as its own doc required of the last block to
/// remove one. **Do not reintroduce it**: a fallback here would now silently
/// override the user's persisted surface.
pub(crate) fn initial_surface() -> i32 {
    surface_for_default_view(
        &crate::settings_qt::pref_str(KEY_DEFAULT_VIEW, "remember"),
        crate::settings_qt::pref_i32(KEY_SURFACE, DEFAULT_SURFACE),
    )
}

pub(crate) fn initial_background_blur() -> bool {
    crate::settings_qt::pref_bool(KEY_BACKGROUND_BLUR, false)
}

pub(crate) fn initial_width() -> f32 {
    crate::settings_qt::pref_f32(KEY_WIDTH, DEFAULT_WIDTH)
}

pub(crate) fn initial_height() -> f32 {
    crate::settings_qt::pref_f32(KEY_HEIGHT, DEFAULT_HEIGHT)
}

/// Persist the last surface. A JSON NUMBER, not a string: `ui_prefs.json` is
/// co-owned with the Slint app, whose typed struct declares `mini_surface` as
/// i32 (`ui_prefs.rs:553-554`) — a string write would poison its whole-document
/// load.
pub(crate) fn save_surface(surface: i32) {
    crate::settings_qt::save_pref(KEY_SURFACE, serde_json::Value::Number(surface.into()));
}

/// Persist the card backdrop toggle (`QbzMini.toggle_background_blur`, A-10).
pub(crate) fn save_background_blur(value: bool) {
    crate::settings_qt::save_pref(KEY_BACKGROUND_BLUR, serde_json::Value::Bool(value));
}

/// Persist the expanded geometry, ALREADY clamped by `geometry_to_persist`.
///
/// Two single-key patches, not one struct write: `ui_prefs.json` is co-owned
/// with the shipping Slint app and `save_pref` is the additive path (§15 trap
/// 16). The reference writes the same two keys from its resize handler
/// (`miniplayer.rs:460-463`) — with a whole-file load+save per event, which is
/// exactly what the 400 ms debounce in `MiniWindow.qml` exists to keep off the
/// Qt GUI thread (§7-M10, §13-D14).
pub(crate) fn save_geometry(width: f32, height: f32) {
    crate::settings_qt::save_pref(KEY_WIDTH, serde_json::json!(width));
    crate::settings_qt::save_pref(KEY_HEIGHT, serde_json::json!(height));
}

#[cfg(test)]
mod tests {
    use super::*;
    use qbz_models::RepeatMode;

    /// QueueTrack has no Default impl — every field is spelled out
    /// (`queue_qt.rs`'s own test helper carries the same note).
    fn track(id: u64, title: &str, version: Option<&str>, secs: u64, explicit: bool) -> QueueTrack {
        QueueTrack {
            id,
            title: title.to_string(),
            version: version.map(str::to_string),
            artist: format!("artist-{id}"),
            album: String::new(),
            album_version: None,
            duration_secs: secs,
            artwork_url: None,
            hires: false,
            bit_depth: None,
            sample_rate: None,
            is_local: false,
            album_id: None,
            artist_id: None,
            streamable: true,
            source: None,
            parental_warning: explicit,
            source_item_id_hint: None,
            context_kind: None,
            context_id: None,
            isrc: None,
            recording_mbid: None,
        }
    }

    /// QueueState has no Default impl either.
    fn state(current: Option<QueueTrack>, upcoming: Vec<QueueTrack>) -> QueueState {
        QueueState {
            current_index: current.as_ref().map(|_| 0),
            total_tracks: upcoming.len() + usize::from(current.is_some()),
            current_track: current,
            upcoming,
            history: Vec::new(),
            shuffle: false,
            repeat: RepeatMode::Off,
            stop_after_track_id: None,
            manual_next_count: 0,
        }
    }

    /// UT-1 (contract §11).
    #[test]
    fn mini_queue_doc_is_current_first_then_full_upcoming() {
        // 1 current + 60 upcoming: no 40-row cap (PAGE_SIZE is the panel's, not
        // the mini's), current first, currentId set.
        let upcoming: Vec<QueueTrack> = (1..=60)
            .map(|i| track(100 + i, &format!("up-{i}"), None, 187, false))
            .collect();
        let doc: serde_json::Value = serde_json::from_str(&mini_queue_doc(&state(
            Some(track(7, "Now", Some("Remastered"), 187, true)),
            upcoming.clone(),
        )))
        .unwrap();

        let rows = doc["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 61);
        assert_eq!(rows[0]["id"], "7");
        assert_eq!(doc["currentId"], "7");
        // Titles go through display_title (version suffix).
        assert_eq!(rows[0]["title"], "Now (Remastered)");
        // Durations go through fmt_duration.
        assert_eq!(rows[0]["duration"], "3:07");
        // explicit is carried (from QueueTrack::parental_warning).
        assert_eq!(rows[0]["explicit"], true);
        assert_eq!(rows[1]["id"], "101");
        assert_eq!(rows[1]["explicit"], false);

        // No current track: 60 rows and an EMPTY currentId.
        let doc: serde_json::Value =
            serde_json::from_str(&mini_queue_doc(&state(None, upcoming))).unwrap();
        assert_eq!(doc["rows"].as_array().unwrap().len(), 60);
        assert_eq!(doc["currentId"], "");
    }

    #[test]
    fn mini_queue_empty_default_is_full_shape() {
        // QML reads `.rows.length` in the pre-publish frame — "{}" would throw.
        let doc: serde_json::Value = serde_json::from_str(MINI_QUEUE_EMPTY).unwrap();
        assert!(doc.get("rows").is_some_and(|r| r.is_array()));
        assert_eq!(doc["currentId"], "");
        // An empty queue serializes to exactly the default document.
        assert_eq!(mini_queue_doc(&state(None, Vec::new())), MINI_QUEUE_EMPTY);
    }

    /// UT-2 (contract §11) — the lock on the §4.4.3 off-by-one fix.
    #[test]
    fn mini_queue_row_maps_to_upcoming_index() {
        // With a current track: row 0 resumes, row n plays upcoming[n - 1].
        assert_eq!(queue_row_target(true, 0), MiniRowTarget::Resume);
        assert_eq!(queue_row_target(true, 1), MiniRowTarget::Upcoming(0));
        assert_eq!(queue_row_target(true, 7), MiniRowTarget::Upcoming(6));
        // WITHOUT one, row 0 IS upcoming[0] — the reference toggles play here.
        assert_eq!(queue_row_target(false, 0), MiniRowTarget::Upcoming(0));
        assert_eq!(queue_row_target(false, 1), MiniRowTarget::Upcoming(1));
        assert_eq!(queue_row_target(false, 7), MiniRowTarget::Upcoming(7));
    }

    /// UT-3 (contract §11).
    #[test]
    fn mini_geometry_guards_and_floors() {
        // Micro and compact are constant sizes — never persisted.
        assert_eq!(geometry_to_persist(0, 380.0, 57.0), None);
        assert_eq!(geometry_to_persist(1, 380.0, 178.0), None);
        assert_eq!(geometry_to_persist(0, 900.0, 900.0), None);
        // Expanded, but a mid-transition frame: 319 is filtered, 320 is not.
        assert_eq!(geometry_to_persist(2, 400.0, 319.0), None);
        assert_eq!(geometry_to_persist(2, 400.0, 320.0), Some((400.0, 420.0)));
        // The stored pair is clamped to (max(w, 340), max(h, 420)) — note the
        // floor (420) is NOT the transition filter (320).
        assert_eq!(geometry_to_persist(2, 300.0, 400.0), Some((340.0, 420.0)));
        assert_eq!(geometry_to_persist(4, 500.0, 700.0), Some((500.0, 700.0)));
    }

    #[test]
    fn default_view_pref_maps_to_the_slint_surface_table() {
        // miniplayer.rs:110-119 — "remember"/unknown fall back to the last
        // persisted surface; every pinned key wins over it.
        assert_eq!(surface_for_default_view("micro", 3), 0);
        assert_eq!(surface_for_default_view("compact", 3), 1);
        assert_eq!(surface_for_default_view("artwork", 3), 2);
        assert_eq!(surface_for_default_view("queue", 0), 3);
        assert_eq!(surface_for_default_view("lyrics", 0), 4);
        assert_eq!(surface_for_default_view("remember", 1), 1);
        assert_eq!(surface_for_default_view("", 4), 4);
    }
}
