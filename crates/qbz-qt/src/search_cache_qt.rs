//! Instant cached paint for the cortinilla — the R1/R6 experiment.
//!
//! ## What this is, and why it is not "restoring" something that was rejected
//!
//! The reference deliberately shows a SKELETON while loading and never paints
//! cached rows first. Its comments present that as the design. It is not: the
//! instant paint existed during development and was removed because it
//! GLITCHED — results visibly jumped between the cached paint and the fresh
//! one. The owner's reading (2026-08-03) is that the glitch was a Slint
//! rendering problem, not something inherent to stale-while-revalidate, and
//! asked for it back here as a REVERSIBLE experiment.
//!
//! So this ships **default ON with a runtime opt-out** (`ui_prefs`
//! `cortinilla_instant_paint`), and the skeleton path stays fully implemented
//! as both the opt-out target and the cold-miss path. Flipping the pref
//! requires no rebuild, which is the point: the two behaviours can be compared
//! back to back in one session.
//!
//! ## Why it should not glitch here
//!
//! The glitch is "the rows moved under me". Four properties are what prevent
//! it, and each is a real constraint on the code, not a hope:
//!
//! 1. **Same shape in both paints.** The entry is written AFTER the local
//!    sections are appended, so the local fold is already inside the cached
//!    payload. There is no mid-apply reflow left to see.
//! 2. **Equality-gated replacement.** `CortinillaData` derives `PartialEq`, so
//!    the fresh payload replaces the shown one only if it actually differs. An
//!    unchanged repeat query produces ZERO second paint.
//! 3. **The selection survives.** The publish path never writes
//!    `cortinillaSelectedIndex`.
//! 4. **The scroll survives.** The overlay latches and restores `contentY`
//!    across a republish. Without that, the second paint yanks the user to the
//!    top — which is the glitch by another name.
//!
//! ## Invalidation
//!
//! Process lifetime is the TTL (see `payload_cache`). Within a session the
//! cache must be cleared whenever its content could have gone stale:
//!
//! - a user switch or logout — a different library and a different account;
//! - any mutation of the LOCAL library, because a cached payload embeds local
//!   rows and their artwork paths.
//!
//! There is no "library changed" signal on `QbzLocal` to subscribe to, so the
//! mutating entry points clear it directly. That is an honest bound rather
//! than a guess: if a future path mutates the library without clearing, its
//! staleness window is one query key until the next keystroke, and the fix is
//! one line at that call site.

use std::sync::Mutex;

use qbz_app::settings::payload_cache::PayloadCache;

use crate::search_qt::CortinillaData;

static CACHE: Mutex<Option<PayloadCache<CortinillaData>>> = Mutex::new(None);

/// Is the instant paint enabled? DEFAULT ON (owner ruling R6).
///
/// Read from `ui_prefs` on every call rather than mirrored in memory, and that
/// is deliberate: the whole purpose of the flag is that the owner can flip it
/// and compare both behaviours WITHOUT a rebuild, which a cached mirror would
/// defeat. The read happens on a tokio worker (`live()` is spawned), never on
/// the Qt GUI thread.
pub fn instant_paint_enabled() -> bool {
    crate::settings_qt::pref_bool("cortinilla_instant_paint", true)
}

/// Bind a fresh cache for a session. Called next to the other per-user stores.
pub fn init() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(PayloadCache::default());
    }
}

/// Drop it on logout — a cached payload holds the previous account's library.
pub fn teardown() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = None;
    }
}

/// Clear every entry without unbinding the store.
///
/// Call this from ANY path that mutates the local library: a scan, an import,
/// a delete, a Plex sync. A cached payload embeds local rows and their artwork
/// paths, so after a mutation those rows may name files that no longer exist.
pub fn invalidate() {
    if let Ok(mut guard) = CACHE.lock() {
        if let Some(c) = guard.as_mut() {
            c.clear();
        }
    }
}

/// The cached payload for `query`, or `None` on a miss / when the flag is off
/// / before a session is bound.
pub fn get(query: &str) -> Option<CortinillaData> {
    if !instant_paint_enabled() {
        return None;
    }
    CACHE.lock().ok()?.as_ref()?.get(query).cloned()
}

/// Store a FINISHED payload.
///
/// "Finished" is load-bearing and there are exactly TWO call sites, both in
/// `search_qt::live`: after pass-1 artwork resolution, and again at the pass-2
/// republish once the downloaded covers have landed. Writing any earlier
/// caches rows with empty artwork paths, and since the QML draws `artPath`
/// only, a hit would paint cover-less rows AND the equality gate would fire on
/// every single hit — the exact repaint this cache exists to avoid.
pub fn put(query: &str, data: &CortinillaData) {
    if !instant_paint_enabled() {
        return;
    }
    if let Ok(mut guard) = CACHE.lock() {
        if let Some(c) = guard.as_mut() {
            c.put(query, data.clone());
        }
    }
}
