//! Listen log glue for the Qt shell — the per-user [`ListenLogger`] binding
//! and the four hooks the playback poll loop calls.
//!
//! WIRED (all in `playback_qt::start_poll_loop`, next to the other
//! track-edge consumers):
//! - the DE-DUPED track edge → [`on_track_edge`] (opens a row; closes the
//!   previous one as `skip` if it was still open);
//! - every tick with a live track → [`on_tick`] (the accumulator; a tick
//!   with `track_id == 0` is the stop detector);
//! - the natural end-of-track edge → [`on_natural_end`];
//! - `main.rs` after the event loop → [`shutdown_blocking`].
//!
//! Nothing here reads the log back. The Settings toggle / clear are the only
//! other callers ([`set_enabled`], [`clear`]).

use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use qbz_app::listen_log::{meta_from_queue_track, ListenLogger, Origin};
use qbz_models::QueueTrack;
use qbz_player::PlaybackEvent;

use crate::playback_qt::OwnerActionLease;

static LOGGER: OnceLock<RwLock<Option<Arc<ListenLogger>>>> = OnceLock::new();

/// A `track_id == 0` tick is normally the sub-second gap while the engine
/// swaps streams; only this many CONSECUTIVE empty ticks (1 Hz) mean the
/// user actually stopped. Below that a following edge closes the row as a
/// skip instead, which is what it was.
const STOP_AFTER_EMPTY_TICKS: u32 = 3;

static EMPTY_TICKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn slot() -> &'static RwLock<Option<Arc<ListenLogger>>> {
    LOGGER.get_or_init(|| RwLock::new(None))
}

fn logger() -> Option<Arc<ListenLogger>> {
    slot().read().ok()?.clone()
}

/// Bind the store for the user that just logged in (`auth_qt`, beside the
/// other per-user stores). Opening is async; until it completes the hooks
/// are no-ops, which at worst misses the first seconds of the first track.
pub fn init_for_user(user_dir: &Path) {
    let dir = user_dir.to_path_buf();
    crate::spawn(async move {
        match ListenLogger::open(dir, Origin::Install).await {
            Ok(logger) => {
                if let Ok(mut g) = slot().write() {
                    *g = Some(logger);
                }
            }
            Err(e) => log::warn!("[qbz-qt] listen log open failed: {e}"),
        }
    });
}

/// Logout: close the row in flight (as `stop`) and drop the binding so the
/// next account never writes into this one's file.
pub async fn teardown() {
    let taken = slot().write().ok().and_then(|mut g| g.take());
    if let Some(logger) = taken {
        logger.stopped(false).await;
    }
}

/// Playback authority moved from the signed-in owner to a delegated renderer.
///
/// The delegation transition calls this only after owner actions have drained,
/// so no admitted owner hook can still be writing the row. Delegated playback
/// never opens a replacement row: every hot-path hook below requires the
/// unforgeable owner lease captured with its playback snapshot.
pub async fn handoff() {
    EMPTY_TICKS.store(0, std::sync::atomic::Ordering::Relaxed);
    if let Some(logger) = logger() {
        logger.handoff().await;
    }
}

/// Orderly exit: close the row in flight as `shutdown`. Synchronous —
/// called from `main.rs` after the event loop, behind the hard-exit
/// watchdog like the session flush next to it.
pub fn shutdown_blocking() {
    if let Some(logger) = logger() {
        logger.shutdown_blocking();
    }
}

/// The de-duped track edge. `event` is the poll's own `PlaybackEvent`, which
/// carries the ACTUAL stream facts (bit depth / sample rate) — the queue row
/// only knows what the catalogue claimed.
pub async fn on_track_edge(
    _owner_action: &OwnerActionLease,
    track: &QueueTrack,
    event: &PlaybackEvent,
) {
    // Ephemeral rows (CD / ad-hoc folder) never reach any history — owner
    // rule, same guard as `recently_qt::record_queue_track`.
    if crate::local_ephemeral::is_ephemeral_id(track.id as i64) {
        return;
    }
    let Some(logger) = logger() else {
        return;
    };
    EMPTY_TICKS.store(0, std::sync::atomic::Ordering::Relaxed);
    let backend = event
        .bit_perfect_mode
        .map(|m| format!("{m:?}").to_ascii_lowercase());
    let meta = meta_from_queue_track(track, event.bit_depth, event.sample_rate, backend);
    // `infer_end = true`: under GAPLESS the engine hands off to the next
    // track without ever reporting a not-playing tick, so the poll loop's
    // `track_ended` predicate never fires and the previous row would close
    // as a skip although it played to its last second (smoke 2026-08-28:
    // 229 s of 230 s recorded as skip). The tracker's 2 s window on the last
    // observed position tells the two apart; when the explicit natural-end
    // edge DID fire first, no row is open and the flag is moot.
    logger.track_started(meta, true).await;
}

/// One poll tick. `position_secs` is the engine's whole-second position;
/// the accumulator only credits monotonic deltas of at most 5 s while
/// `is_playing`, so pauses and seeks add nothing.
pub async fn on_tick(
    _owner_action: &OwnerActionLease,
    track_id: u64,
    position_secs: u64,
    is_playing: bool,
) {
    let Some(logger) = logger() else {
        return;
    };
    if track_id == 0 {
        if !logger.has_open_row() {
            return;
        }
        let n = EMPTY_TICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n >= STOP_AFTER_EMPTY_TICKS {
            EMPTY_TICKS.store(0, std::sync::atomic::Ordering::Relaxed);
            logger.stopped(false).await;
        }
        return;
    }
    EMPTY_TICKS.store(0, std::sync::atomic::Ordering::Relaxed);
    logger.tick(position_secs * 1_000, is_playing).await;
}

/// The poll loop's natural end-of-track edge (the `track_ended` predicate),
/// BEFORE anything advances the cursor.
pub async fn on_natural_end(_owner_action: &OwnerActionLease) {
    if let Some(logger) = logger() {
        logger.ended_naturally().await;
    }
}

/// A stream failure on the current track.
pub async fn on_error(_owner_action: &OwnerActionLease) {
    if let Some(logger) = logger() {
        logger.errored().await;
    }
}

// ----- Settings -------------------------------------------------------------

/// The "Listening history" toggle state for the Settings snapshot. `true`
/// while no store is bound (the default is ON, and a fresh user has no
/// `paused` flag yet).
pub fn is_enabled() -> bool {
    logger().map(|l| !l.is_paused()).unwrap_or(true)
}

pub async fn set_enabled(value: bool) -> Result<(), String> {
    match logger() {
        Some(logger) => logger.set_paused(!value).await,
        None => Err("listening history store is not open".to_string()),
    }
}

/// "Clear listening history" — DELETE + VACUUM; the row in flight goes too.
pub async fn clear() -> Result<(), String> {
    match logger() {
        Some(logger) => logger.clear().await,
        None => Err("listening history store is not open".to_string()),
    }
}
