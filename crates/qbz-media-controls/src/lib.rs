//! Frontend-agnostic system media controls (ADR-006).
//!
//! - **Linux:** `mpris-server` — publishes the full MPRIS interface INCLUDING
//!   `org.mpris.MediaPlayer2.DesktopEntry = "com.blitzfc.qbz"`, the only way
//!   GNOME Shell resolves the application icon for its media widget. (souvlaki,
//!   the cross-platform crate, never sets it, so GNOME shows no app icon.)
//! - **macOS / Windows:** `souvlaki` — MediaRemote / SMTC, where there is no
//!   DesktopEntry concept (macOS keys the icon off the app bundle).
//!
//! One trait ([`MediaIntegration`]); one factory ([`spawn`]); no winit / Slint
//! / Tauri types — headless/TUI can reuse it.

use std::sync::Arc;

mod types;
pub use types::{xesam_url_for, MediaEvent, MediaIntegration, PlaybackStatus, TrackMeta};

pub mod notify;
pub use notify::{show_track_notification, withdraw_track_notification, NotificationMeta};

#[cfg(target_os = "linux")]
mod inhibit;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform;

/// Spawn the OS media-controls integration. `on_event` receives inbound
/// control events (media keys, the desktop media widget, macOS Now Playing).
/// Returns `None` if the platform backend could not start — the app keeps
/// working without media controls.
///
/// **macOS:** souvlaki's command callbacks fire on the app run loop, so this is
/// safe to call from any thread, but the run loop (Slint's winit loop) must be
/// running for inbound events to arrive.
/// Keep the machine awake while playing. No-op off Windows; see
/// `platform::set_sleep_inhibit` for why it is not a handle method.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use platform::set_sleep_inhibit;

/// Keep the machine awake while playing. No-op on this platform.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn set_sleep_inhibit(_playing: bool) {}

/// The platform window identity the OS integration needs.
///
/// Windows SMTC is registered against a window: souvlaki's backend calls
/// `config.hwnd.expect(..)`, so a missing handle is a PANIC, not a graceful
/// degrade. Linux and macOS key off the D-Bus name and the app bundle
/// respectively and ignore this entirely, which is why `default()` is the
/// right thing to pass there and from headless callers.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeWindow {
    pub hwnd: Option<*mut core::ffi::c_void>,
}

// SAFETY: an HWND is an opaque process-wide identifier, not a pointer that is
// dereferenced here. It is read once by souvlaki on the thread that receives
// it. Windows itself allows a window handle to be named from any thread; what
// is thread-affine is the window's message loop, which this never touches.
unsafe impl Send for NativeWindow {}

pub fn spawn(
    on_event: impl Fn(MediaEvent) + Send + Sync + 'static,
    native: NativeWindow,
) -> Option<Box<dyn MediaIntegration>> {
    let cb: Arc<dyn Fn(MediaEvent) + Send + Sync> = Arc::new(on_event);

    #[cfg(target_os = "linux")]
    {
        let _ = native;
        return linux::spawn(cb).map(|h| Box::new(h) as Box<dyn MediaIntegration>);
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        return platform::spawn(cb, native).map(|h| Box::new(h) as Box<dyn MediaIntegration>);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (cb, native);
        None
    }
}
