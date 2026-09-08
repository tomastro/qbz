//! Kiosk profile — resolution, the live toggle, and the boot decisions.
//!
//! Port of three Slint blocks (2026-08-02 kiosk-port contract §8):
//!
//! * `qbz-nix/crates/qbz/src/main.rs:7855-7876` — `kiosk_profile_active()`
//!   and `resolved_shell_screen()`.
//! * `:8586-8618` — kiosk entry: forced reduce-motion, and fullscreen ONLY
//!   via `QBZ_KIOSK_FULLSCREEN`.
//! * `:11767-11780` — the live Kiosk <-> Desktop toggle.
//!
//! The `profile` key is shared with the shipping Slint app
//! (`qbz-nix/crates/qbz/src/ui_prefs.rs:634-635`, default `"desktop"`), so it
//! is read and written through `settings_qt`'s additive single-key patch and
//! never by rewriting the file.

use std::sync::atomic::{AtomicBool, Ordering};

use cxx_qt_lib::QString;

use crate::{session_bridge, settings_qt, shell_bridge};

/// The persisted key, shared with Slint.
const PREF_KEY: &str = "profile";
const KIOSK: &str = "kiosk";
const DESKTOP: &str = "desktop";

/// Rust-side mirror of the live profile.
///
/// Two readers need it and neither can reach a Qt property: the boot path
/// decides a screen string before any QObject exists, and [`toggle`] runs off
/// the Qt thread (`nav_qt.rs:63-66` documents that constraint and why an
/// accessor exists rather than a QML round trip). Same shape as
/// `shell_bridge.rs:579`'s `QUEUE_OPEN`.
static KIOSK_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Is the kiosk profile on right now?
pub fn active() -> bool {
    KIOSK_ACTIVE.load(Ordering::SeqCst)
}

/// Resolve the profile from `QBZ_PROFILE` (env wins) else the persisted key.
///
/// 1:1 with `main.rs:7855-7867`, including the two details that make the env
/// override usable: it is trimmed and compared case-insensitively, and an
/// EMPTY value falls through to the pref rather than reading as "not kiosk" —
/// so `QBZ_PROFILE=` behaves like an unset variable, and `QBZ_PROFILE=desktop`
/// can force the desktop shell back on a kiosk image.
pub fn resolve() -> bool {
    if let Ok(v) = std::env::var("QBZ_PROFILE") {
        let v = v.trim();
        if !v.is_empty() {
            return v.eq_ignore_ascii_case(KIOSK);
        }
    }
    settings_qt::pref_str(PREF_KEY, DESKTOP)
        .trim()
        .eq_ignore_ascii_case(KIOSK)
}

/// Called once at boot, before the first screen is published: resolve the
/// profile, latch it, and report the two things it decides.
///
/// Returns whether the kiosk profile is on.
pub fn init_at_boot() -> bool {
    let kiosk = resolve();
    KIOSK_ACTIVE.store(kiosk, Ordering::SeqCst);
    if kiosk {
        // `main.rs:8607-8617`. Fullscreen at boot is OPT-IN and stays that
        // way: a user who toggles kiosk on the desktop and restarts would be
        // TRAPPED, because the kiosk shell has no titlebar control and
        // neither Esc nor F11 leave fullscreen (incident 2026-07-11).
        if fullscreen_at_boot() {
            log::info!(
                "[qbz-qt] kiosk profile active -> fullscreen-at-boot \
                 (QBZ_KIOSK_FULLSCREEN) + forced reduce-motion"
            );
        } else {
            log::info!(
                "[qbz-qt] kiosk profile active (reduce-motion on); windowed \
                 — set QBZ_KIOSK_FULLSCREEN=1 for appliance boot"
            );
        }
    }
    kiosk
}

/// `QBZ_KIOSK_FULLSCREEN` is presence-checked, not parsed — `main.rs:8608`
/// uses `env::var_os(..).is_some()`, so any value including an empty one
/// arms it.
pub fn fullscreen_at_boot() -> bool {
    std::env::var_os("QBZ_KIOSK_FULLSCREEN").is_some()
}

/// The screen id the post-login mount should use — the Qt spelling of
/// `resolved_shell_screen()` (`main.rs:7869-7876`).
///
/// Every `set_screen("shell")` site goes through this, so a kiosk-profile
/// session lands in the kiosk shell whether it arrived by login, by silent
/// session restore, or by the offline path.
pub fn shell_screen() -> &'static str {
    if active() {
        KIOSK
    } else {
        "shell"
    }
}

/// The live toggle (`main.rs:11767-11780`).
///
/// The order is load-bearing and matches the reference: persist, then flip the
/// flag, then swap the screen. The flag before the screen means the shell that
/// mounts next reads a correct `QbzShell.kioskProfile` on its FIRST frame
/// rather than one turn later.
///
/// The decision reads the CURRENT state, not [`resolve`] — the reference's
/// comment at `:11764-11765` is explicit about why: resolving would let a
/// `QBZ_PROFILE` env override pin the reading, and a user on a kiosk image
/// could then never toggle out.
pub fn toggle() {
    let to_kiosk = !active();
    KIOSK_ACTIVE.store(to_kiosk, Ordering::SeqCst);

    let value = if to_kiosk { KIOSK } else { DESKTOP };
    settings_qt::save_pref(PREF_KEY, serde_json::json!(value));
    log::info!("[qbz-qt] profile -> {value}");

    // Forced reduce-motion follows the profile (`main.rs:8596-8598`): the
    // reference computes `kiosk_profile || !use_gpu_renderer`. BOTH halves are
    // honest now (2026-08-11) — `renderer_qt` latches the tier from the QML
    // probe — so this composes them instead of assigning the kiosk half alone.
    // It used to write `to_kiosk` bare, which meant leaving kiosk on a
    // software-tier box turned reduce-motion OFF and handed that box full-rate
    // animations, the exact thing the tier half exists to prevent.
    shell_bridge::ui(move |mut b| {
        b.as_mut().set_kiosk_profile(to_kiosk);
        b.as_mut()
            .set_reduce_motion(crate::renderer_qt::reduce_motion(to_kiosk));
    });
    session_bridge::ui(move |mut b| {
        b.as_mut()
            .set_screen(QString::from(if to_kiosk { KIOSK } else { "shell" }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `resolve()` reads a process-global env var, so these run serially
    /// behind one lock rather than racing each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_profile_env<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("QBZ_PROFILE");
        match value {
            Some(v) => std::env::set_var("QBZ_PROFILE", v),
            None => std::env::remove_var("QBZ_PROFILE"),
        }
        let out = f();
        match previous {
            Some(v) => std::env::set_var("QBZ_PROFILE", v),
            None => std::env::remove_var("QBZ_PROFILE"),
        }
        out
    }

    #[test]
    fn env_wins_and_is_case_insensitive() {
        assert!(with_profile_env(Some("kiosk"), resolve));
        assert!(with_profile_env(Some("KIOSK"), resolve));
        assert!(with_profile_env(Some("  Kiosk  "), resolve));
    }

    #[test]
    fn env_can_force_desktop_back_on() {
        assert!(!with_profile_env(Some("desktop"), resolve));
        assert!(
            !with_profile_env(Some("anything-else"), resolve),
            "anything but kiosk is the desktop profile"
        );
    }

    #[test]
    fn an_empty_env_value_falls_through_to_the_pref() {
        // Both spellings behave as "unset": whatever they resolve to, they
        // resolve to the SAME thing, which is the pref-backed answer.
        let empty = with_profile_env(Some(""), resolve);
        let blank = with_profile_env(Some("   "), resolve);
        let unset = with_profile_env(None, resolve);
        assert_eq!(empty, unset);
        assert_eq!(blank, unset);
    }

    #[test]
    fn the_screen_id_follows_the_latch() {
        let restore = active();
        KIOSK_ACTIVE.store(true, Ordering::SeqCst);
        assert_eq!(shell_screen(), "kiosk");
        KIOSK_ACTIVE.store(false, Ordering::SeqCst);
        assert_eq!(shell_screen(), "shell");
        KIOSK_ACTIVE.store(restore, Ordering::SeqCst);
    }
}
