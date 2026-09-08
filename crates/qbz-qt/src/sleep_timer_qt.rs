//! Sleep timer (queue footer) — port of `crates/qbz/src/sleep_timer.rs`.
//!
//! A single armed deadline that PAUSES playback when it elapses, plus a 1 Hz
//! countdown pushed onto the queue bridge. The deadline is a monotonic
//! `Instant`, not a wall-clock timestamp, so a laptop suspend or a clock jump
//! cannot fire it early or strand it.
//!
//! Nothing is persisted: an armed timer dies with the process. That is the
//! reference's deliberate choice — a timer that survived a restart would stop
//! playback long after the user forgot about it.
//!
//! A process-wide generation counter invalidates an in-flight task on cancel or
//! re-arm, so a superseded task can never pause playback behind the live one's
//! back. `set` bumps it and keeps its own value; the task exits the moment the
//! two stop matching.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cxx_qt_lib::QString;
use qbz_app::shell::AppRuntime;
use qbz_core::LoggingAdapter;

/// Monotonically increasing token. Each `set`/`cancel` bumps it; a running task
/// keeps the value it was spawned with and exits as soon as it differs.
static GENERATION: AtomicU64 = AtomicU64::new(0);

const MIN_MINUTES: i32 = 1;
const MAX_MINUTES: i32 = 24 * 60; // 1440

fn push_state(active: bool, remaining: String) {
    crate::queue_bridge::ui(move |mut b| {
        b.as_mut().set_sleep_active(active);
        b.as_mut()
            .set_sleep_remaining(QString::from(remaining.as_str()));
    });
}

/// Arm (or re-arm) the sleep timer for `minutes`, clamped to [1, 1440].
/// Replaces any running timer. At expiry it pauses playback (only if something
/// is playing) and returns to idle.
pub fn set(runtime: Arc<AppRuntime<LoggingAdapter>>, minutes: i32) {
    if minutes <= 0 {
        return;
    }
    let minutes = minutes.clamp(MIN_MINUTES, MAX_MINUTES);
    // Bump first: this task owns `my_gen` until the next set/cancel.
    let my_gen = GENERATION.fetch_add(1, Ordering::SeqCst).wrapping_add(1);

    crate::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs((minutes as u64) * 60);
        // Immediate feedback: armed + the initial countdown, before the first
        // tick lands.
        push_state(
            true,
            qbz_text_utils::sleep::format_sleep_remaining((minutes as i64) * 60),
        );

        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await; // fires immediately, then every 1s
            if GENERATION.load(Ordering::SeqCst) != my_gen {
                return; // cancelled or superseded
            }
            let now = Instant::now();
            if now >= deadline {
                // A timer can be armed under owner authority and outlive a
                // QConnect handoff. Re-admit the expiry itself so that stale
                // local automation cannot pause the delegated renderer (and
                // cannot race a transition fence). The timer still returns to
                // idle: its one deadline has elapsed, only the playback side
                // effect is refused.
                let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
                    log::info!("[qbz-qt] sleep-timer: expiry ignored while QConnect owns playback");
                    push_state(false, String::new());
                    return;
                };
                if runtime.core().get_playback_state().is_playing {
                    if let Err(e) = runtime.core().pause() {
                        log::warn!("[qbz-qt] sleep-timer: pause on expiry failed: {e}");
                    }
                    // The transport flag the bar reads is pushed by the poll
                    // loop's own play/pause edge; nothing to mirror here.
                }
                push_state(false, String::new());
                return;
            }
            let remaining = (deadline - now).as_secs() as i64;
            push_state(
                true,
                qbz_text_utils::sleep::format_sleep_remaining(remaining),
            );
        }
    });
}

/// Cancel any armed timer and return to idle.
pub fn cancel() {
    GENERATION.fetch_add(1, Ordering::SeqCst); // invalidate the running task
    push_state(false, String::new());
}
