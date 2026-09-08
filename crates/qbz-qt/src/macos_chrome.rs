//! macOS custom chrome: keep the NATIVE traffic lights and centre them in
//! QBZ's own header bar.
//!
//! Port of `crates/qbz/src/macos_chrome.rs`. The reference's framing, which
//! is also the owner's ask (2026-07-21, "centre them, like the official Qobuz
//! app does"): with the overlay window attributes the native lights float
//! over QBZ's header, but AppKit parks them at the STANDARD titlebar height
//! (~28pt) while the header is 42px tall, so they sit ~7pt above the header
//! controls' vertical centre.
//!
//! WHY THIS EXISTS AT ALL — the Qt port's divergence, fixed here:
//! `Main.qml` used to add `Qt.FramelessWindowHint` on every platform when the
//! custom title bar was on. On macOS that removes the traffic lights
//! outright, so the port drew its OWN min/max/close cluster there — the one
//! thing the reference explicitly never does
//! (`qbz-ui/ui/shell/WindowControls.slint:1-2`: "Linux only — macOS keeps the
//! native traffic lights"). The window is now left framed on macOS and made
//! transparent/full-size instead, which is the AppKit idiom and what winit
//! does for the Slint build.
//!
//! **NOT COMPILED OR TESTED ON LINUX.** Every objc2 name below was read out
//! of objc2-app-kit-0.2.2's own generated sources before being written (the
//! feature gates are listed in `Cargo.toml`), which narrows the blind spot to
//! runtime behaviour — it does not remove it. The 2026-08-04 Hide-Dock round
//! is the precedent: careful reading caught one of three errors, the platform
//! compiler caught the other two.

/// Vertical centre target in AppKit points, measured from the window's top
/// edge: half of the header height (42px — Qt logical px == AppKit points on
/// macOS). Keep in sync with `qml/theme/QbzTheme.qml`'s `headerHeight` and
/// with the reference's `Layout.header-height`.
#[cfg(target_os = "macos")]
const HEADER_CENTRE_PT: f64 = 21.0;

/// Apply the overlay attributes and centre the lights.
///
/// **Returns whether the work was actually done**, and the caller must retry
/// while it is `false`. That signature is the fix for the 2026-08-05 failure:
/// `Main.qml` latched "applied" BEFORE calling, so the single early attempt —
/// fired on the first rendered frame, when AppKit has not yet made the window
/// `main` — permanently disabled the chrome. The symptom was a stock macOS
/// title bar sitting above the QBZ header, with the 78px traffic-light inset
/// reserved in a header the lights never reached. The log said so exactly
/// once: `[macos-chrome] skipped: no main window yet`.
///
/// Idempotent, so retrying is free: it returns early when the buttons are
/// already centred, and the three window attributes are set to the same
/// values every time.
///
/// No-op (returning `true`, since there is nothing to wait for) with the
/// system title bar, where AppKit owns the layout. **Main-thread only**: its
/// caller is a `#[qinvokable]`, which runs on the Qt GUI thread, and on macOS
/// that IS the AppKit main thread.
#[cfg(target_os = "macos")]
pub(crate) fn apply_and_center() -> bool {
    use objc2_app_kit::{NSApplication, NSView, NSWindowButton, NSWindowTitleVisibility};
    use objc2_foundation::MainThreadMarker;

    if crate::settings_qt::use_system_title_bar() {
        return true;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("[macos-chrome] skipped: not on the AppKit main thread");
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    // `mainWindow` is nil until AppKit has ordered the window front AND made
    // it main, which is LATER than the first rendered frame — that gap is the
    // whole bug this function's return value exists to survive. `keyWindow`
    // covers the focused-but-not-yet-main moment; anything still unresolved is
    // a `false` and the caller tries again.
    //
    // Deliberately NOT falling back to the `windows` array: iterating an
    // NSArray adds objc2 API surface that cannot be compile-checked from
    // Linux, and the retry already covers the timing this would.
    //
    // SAFETY: both are plain accessors; the marker above proves we are on the
    // main thread, which is the only requirement AppKit places on them.
    let Some(ns_window) = (unsafe { app.mainWindow().or_else(|| app.keyWindow()) }) else {
        log::warn!("[macos-chrome] no main/key window yet — will retry");
        return false;
    };

    // --- DIAGNOSTIC: what did Qt actually do to this window? --------------
    //
    // Read-only. It exists because everything above this line is now correct
    // ON PAPER — Qt 6.9's `windowStyleMask()` sets FullSizeContentView from
    // `ExpandedClientAreaHint`, `setWindowFlags()` sets
    // `titlebarAppearsTransparent` from `NoTitleBarBackgroundHint`, both
    // enums resolve in QML (measured: 4194304 / 8388608), the binding is
    // declarative and nothing overwrites it — and the owner still sees a
    // stock title bar. One of those "shoulds" is false, and this says which
    // one instead of the next guess.
    //
    // The decisive number is the LAST pair: with FullSizeContentView active
    // the content view is as tall as the window frame. If it is ~28pt short,
    // the bit never landed no matter what the mask claims.
    {
        let mask = ns_window.styleMask().0;
        let content_h = ns_window
            .contentView()
            .map(|v| v.frame().size.height)
            .unwrap_or(-1.0);
        log::info!(
            "[macos-chrome] DIAG styleMask=0x{mask:x} fullSizeContentView={} \
             titlebarTransparent={} windowH={:.1} contentH={:.1}",
            (mask & (1 << 15)) != 0,
            unsafe { ns_window.titlebarAppearsTransparent() },
            ns_window.frame().size.height,
            content_h,
        );
    }

    // NO STYLE-MASK POKING HERE. Expanding the client area and dropping the
    // titlebar background is `Main.qml`'s job now, through the Qt 6.9 window
    // flags `ExpandedClientAreaHint | NoTitleBarBackgroundHint`.
    //
    // Doing it from here was unwinnable: `QCocoaWindow::windowStyleMask()`
    // recomputes the mask from Qt's flags and reassigns it on
    // `setWindowFlags()`, on fullscreen transitions and via `setWindowState()`,
    // preserving only the fullscreen and unified-toolbar bits — so
    // `FullSizeContentView` was erased moments after being set, by Qt's own
    // `visibility` handling. The symptom was maddening precisely because the
    // OTHER two attributes (transparent titlebar, hidden title) survived: the
    // log reported success, the lights moved, and the bar stayed.
    //
    // What is left for this module is what Qt does NOT do.
    //
    // 1. Hide the title TEXT. Neither hint touches `titleVisibility` — the
    //    reference gets it from winit's `with_title_hidden` — so without this
    //    the window title draws over the expanded content. It is a plain
    //    NSWindow property, NOT a style-mask bit, which is why Qt's mask
    //    recomputation cannot erase it (the trap that sank the first attempt).
    ns_window.setTitleVisibility(NSWindowTitleVisibility::NSWindowTitleHidden);

    // 2. Centre the lights: AppKit parks them for a ~28pt titlebar, and the
    //    QBZ header is 42px.

    // --- Centre the lights.
    //
    // They are three NSButtons inside ONE container view; shifting the
    // container moves all three and preserves their spacing.
    let Some(close) = ns_window.standardWindowButton(NSWindowButton::NSWindowCloseButton) else {
        // The attributes above DID apply; only the centring has nothing to
        // work with, so this counts as done — retrying would not help.
        log::warn!("[macos-chrome] no close button: attributes applied, centring skipped");
        return true;
    };
    let win_h = ns_window.frame().size.height;

    // Close-button centre in window base coordinates (origin bottom-left, +y
    // up), measured from the button's own BOUNDS — its frame lives in the
    // superview's space and double-counts the in-container offset. The
    // reference records a 2026-07-22 regression from exactly that mistake:
    // measuring `frame` and assuming a bottom-left superview shifted the
    // lights UP instead of down.
    let measure_centre_from_top = |btn: &NSView| -> f64 {
        let r = btn.convertRect_toView(btn.bounds(), None);
        win_h - (r.origin.y + r.size.height / 2.0)
    };

    let before = measure_centre_from_top(&close);
    let move_down = HEADER_CENTRE_PT - before; // visual pts, +ve = down
    if move_down.abs() < 0.5 {
        return true; // already centred (idempotent re-entry)
    }
    // SAFETY: main-thread view geometry on a live window, as above.
    let Some(container) = (unsafe { close.superview() }) else {
        return true;
    };
    // The container's frame is in ITS superview's coordinate space, and
    // AppKit flips the titlebar hierarchy (+y down when flipped).
    let parent_flipped = unsafe { container.superview() }
        .map(|sv| sv.isFlipped())
        .unwrap_or(false);
    let original = container.frame();
    let mut frame = original;
    frame.origin.y += if parent_flipped {
        move_down
    } else {
        -move_down
    };
    unsafe { container.setFrame(frame) };

    // Re-measure: if the shift landed FURTHER from target than where it
    // started (a coordinate-model surprise), undo it. Stock placement is an
    // acceptable fallback; a wrongly shifted one is not.
    let after = measure_centre_from_top(&close);
    if (after - HEADER_CENTRE_PT).abs() > (before - HEADER_CENTRE_PT).abs() {
        unsafe { container.setFrame(original) };
        log::warn!(
            "[macos-chrome] traffic-light shift reverted: centre {before:.1}pt -> {after:.1}pt from top (target {HEADER_CENTRE_PT}pt, parent flipped: {parent_flipped})"
        );
        return true;
    }
    log::info!(
        "[macos-chrome] traffic lights centred: {before:.1}pt -> {after:.1}pt from top (target {HEADER_CENTRE_PT}pt, parent flipped: {parent_flipped})"
    );
    true
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn apply_and_center() -> bool {
    true
}
