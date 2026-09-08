//! Memory-pressure watchdog.
//!
//! Every 15 s, reads the host's memory pressure from `qbz-core`'s
//! `system_capabilities` and reacts:
//!
//! * `is_critical` (< 5 % available) — evict the L1 in-memory audio cache
//!   (the L2 disk files stay; disk is not the scarce resource) and latch
//!   [`prefetch_halted`] so no new prefetch downloads start.
//! * `is_low` (< 15 % available) — pause HiRes prefetch for that tick via
//!   [`hires_prefetch_paused`]; a normal-lossless prefetch is ~4x smaller.
//! * healthy again — clear both latches and log the recovery at debug.
//!   There is deliberately no fancier unlatching logic.
//!
//! Both frontends (`qbz`, `qbzd`) spawn the loop once at startup with the
//! shared [`Player`] handle; the prefetch path in
//! [`crate::playback_driver`] consults the latches before starting work.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use qbz_player::Player;

/// How often the watchdog samples /proc/meminfo.
const TICK_SECS: u64 = 15;

/// Latched on critical pressure: no new prefetch downloads may start.
static PREFETCH_HALTED: AtomicBool = AtomicBool::new(false);

/// Set on low/critical pressure ticks: prefetch downgrades HiRes to
/// Lossless. Follows the latest tick rather than latching.
static HIRES_PREFETCH_PAUSED: AtomicBool = AtomicBool::new(false);

/// True while the watchdog has halted prefetch downloads (critical
/// pressure). Checked by the prefetch path before starting any download.
pub fn prefetch_halted() -> bool {
    PREFETCH_HALTED.load(Ordering::Relaxed)
}

/// True when the latest watchdog tick saw low (or critical) memory
/// pressure — prefetch should stay at Lossless this round.
pub fn hires_prefetch_paused() -> bool {
    HIRES_PREFETCH_PAUSED.load(Ordering::Relaxed)
}

/// Spawn the watchdog loop on the current tokio runtime. Runs for the
/// process lifetime; the returned handle lets a caller with an ordered
/// shutdown abort it explicitly.
pub fn spawn(player: Arc<Player>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECS));
        loop {
            interval.tick().await;
            // None where no memory probe exists (/proc/meminfo on unix,
            // GlobalMemoryStatusEx on Windows) — nothing to do.
            let Some(pressure) = qbz_core::system_capabilities::read_memory_pressure() else {
                continue;
            };
            if pressure.is_critical {
                log::warn!(
                    "[memwatch] critical memory pressure: {:.1}% available ({} MB of {} MB) — evicting L1 audio cache, halting prefetch",
                    pressure.available_pct,
                    pressure.mem_available_kb / 1024,
                    pressure.mem_total_kb / 1024,
                );
                player.evict_l1_audio_cache();
                PREFETCH_HALTED.store(true, Ordering::Relaxed);
                HIRES_PREFETCH_PAUSED.store(true, Ordering::Relaxed);
            } else if pressure.is_low {
                HIRES_PREFETCH_PAUSED.store(true, Ordering::Relaxed);
                log::debug!(
                    "[memwatch] low memory pressure: {:.1}% available — hires prefetch paused this tick",
                    pressure.available_pct,
                );
            } else {
                let was_halted = PREFETCH_HALTED.swap(false, Ordering::Relaxed);
                let was_paused = HIRES_PREFETCH_PAUSED.swap(false, Ordering::Relaxed);
                if was_halted || was_paused {
                    log::debug!(
                        "[memwatch] memory pressure recovered ({:.1}% available) — prefetch re-enabled",
                        pressure.available_pct,
                    );
                }
            }
        }
    })
}
