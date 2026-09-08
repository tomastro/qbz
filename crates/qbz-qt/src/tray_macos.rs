//! macOS menu-bar tray (`NSStatusItem`), hand-rolled on objc2 0.5.
//!
//! Port of `crates/qbz/src/tray/macos.rs`. Until 2026-08-05 this port had no
//! macOS tray at all — `tray_qt::init`'s non-Linux arm was a single log line —
//! and the owner's smoke found the two symptoms that come from that ONE cause:
//! the "Enable tray icon" setting was on and no icon appeared, and
//! "Close to tray" was on and closing quit the app. The second is not a
//! separate bug: `should_hide_on_close` is `close_to_tray && handle().is_some()`,
//! and with no tray the handle is `None`, so closing correctly falls through to
//! quit. One tray, two symptoms.
//!
//! ## Why hand-rolled instead of `tray-icon`
//!
//! The `Cargo.toml` note that deferred this said macOS "would be a NEW
//! implementation, not a port", on the grounds that the reference only
//! hand-rolls because Slint's winit backend bundles `muda`. That reads the
//! reference backwards: its `macos.rs` exists precisely to AVOID muda, so it
//! depends on nothing winit provides and ports here almost verbatim. The
//! premise was true (Qt has no muda conflict) and the conclusion did not
//! follow.
//!
//! ## What changed vs the reference
//!
//! It got SIMPLER. The Slint version carries a process-global `CTX` holding a
//! `Runtime`, a `slint::Weak<AppWindow>` and a `tokio::runtime::Handle`,
//! because its dispatch helpers need all three. This port's helpers
//! (`tray_qt::toggle_window`, `quit`, `dispatch_*`) are free functions that
//! marshal themselves, so the whole `CTX` disappears and `dispatch_tag` is a
//! plain match.
//!
//! Everything here is main-thread only: the `NSStatusItem`, the menu and the
//! `QbzTrayMenuTarget` instance are `!Send` (`thread_local!`). [`create`] must
//! be reached through `tray_bridge::ui`, which queues onto the Qt GUI thread —
//! on macOS that IS the AppKit main thread.
//!
//! Click behaviour matches the reference and the Tauri tray: the menu is NOT
//! permanently attached to the status item. The status button gets its own
//! target-action firing on both left and right mouse-up; LEFT toggles the
//! window, RIGHT (or control-click) pops the menu transiently.
//!
//! **NOT COMPILED ON LINUX.** Every objc2 name below is taken from the
//! reference, which builds on a Mac, and the feature set in `Cargo.toml` is
//! copied from its own `Cargo.toml` rather than deduced.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEventMask, NSEventModifierFlags, NSEventType,
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSData, NSInteger, NSSize, NSString};

// 44px assets (= 22pt @2x menu bar). Filename trap, shared with Linux:
// `tray-dark-*` holds the WHITE glyph, `tray-light-*` holds the BLACK glyph.
const ICON_COLOR: &[u8] = include_bytes!("../icons/tray-color-44.png");
const ICON_WHITE: &[u8] = include_bytes!("../icons/tray-dark-44.png");
const ICON_BLACK: &[u8] = include_bytes!("../icons/tray-light-44.png");

// Menu item tags -> actions.
const TAG_PLAY_PAUSE: NSInteger = 1;
const TAG_NEXT: NSInteger = 2;
const TAG_PREVIOUS: NSInteger = 3;
const TAG_SHOW_HIDE: NSInteger = 4;
const TAG_QUIT: NSInteger = 5;

thread_local! {
    // Kept alive for the tray's lifetime; dropping the status item removes it
    // from the menu bar. All three are `!Send`, main-thread only.
    static STATUS_ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
    static MENU_TARGET: RefCell<Option<Retained<QbzTrayMenuTarget>>> = const { RefCell::new(None) };
    // The menu is NOT permanently attached to the status item (that would make
    // a left-click pop it). It lives here and is only flashed onto the status
    // item for the duration of a right/control-click pop-up.
    static MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
}

declare_class!(
    struct QbzTrayMenuTarget;

    unsafe impl ClassType for QbzTrayMenuTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        // Distinct from the reference's class name is NOT required — the two
        // binaries never share a process — but the name is kept identical so
        // a crash log from either build reads the same.
        const NAME: &'static str = "QbzTrayMenuTarget";
    }

    impl DeclaredClass for QbzTrayMenuTarget {
        type Ivars = ();
    }

    unsafe impl QbzTrayMenuTarget {
        #[method(onMenuItem:)]
        fn on_menu_item(&self, sender: Option<&NSMenuItem>) {
            let tag = sender.map(|s| unsafe { s.tag() }).unwrap_or(0);
            dispatch_tag(tag);
        }

        // Fires on left AND right mouse-up of the status-bar button (see
        // `sendActionOn` in `create`). We inspect the current event to route:
        // right-click / control-click -> pop the menu; plain left -> toggle.
        #[method(onStatusButton:)]
        fn on_status_button(&self, _sender: Option<&AnyObject>) {
            handle_status_click();
        }
    }
);

impl QbzTrayMenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Route a clicked menu item's tag to the shared dispatch helpers.
///
/// No context to look up — unlike the reference, every helper here marshals
/// itself onto whatever thread it needs.
fn dispatch_tag(tag: NSInteger) {
    log::info!("[tray] menu item activated: tag={tag}");
    match tag {
        TAG_PLAY_PAUSE => crate::tray_qt::dispatch_play_pause(),
        TAG_NEXT => crate::tray_qt::dispatch_next(),
        TAG_PREVIOUS => crate::tray_qt::dispatch_previous(),
        TAG_SHOW_HIDE => crate::tray_qt::toggle_window(),
        TAG_QUIT => crate::tray_qt::quit(),
        other => log::debug!("[tray] unhandled menu tag {other}"),
    }
}

/// Status-bar button click router. Reads the current AppKit event: a
/// right-click or control-click pops the menu, a plain left-click toggles the
/// window. Main thread only (it is an AppKit action callback).
fn handle_status_click() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let (is_right, is_ctrl) = match app.currentEvent() {
        Some(ev) => {
            let ty = unsafe { ev.r#type() };
            let mods = unsafe { ev.modifierFlags() };
            (
                ty == NSEventType::RightMouseUp,
                mods.contains(NSEventModifierFlags::NSEventModifierFlagControl),
            )
        }
        None => (false, false),
    };

    if is_right || is_ctrl {
        pop_up_menu(mtm);
    } else {
        crate::tray_qt::toggle_window();
    }
}

/// Pop the tray menu transiently. Non-deprecated replacement for
/// `popUpStatusItemMenu:`: flash the menu onto the status item, simulate a
/// click (which opens it modally), then detach it so a left-click does not
/// open it. Main thread only.
fn pop_up_menu(mtm: MainThreadMarker) {
    STATUS_ITEM.with(|s| {
        let Some(status_item) = s.borrow().as_ref().cloned() else {
            return;
        };
        MENU.with(|m| {
            if let Some(menu) = m.borrow().as_ref() {
                unsafe { status_item.setMenu(Some(menu)) };
                if let Some(button) = unsafe { status_item.button(mtm) } {
                    unsafe { button.performClick(None) };
                }
                unsafe { status_item.setMenu(None) };
            }
        });
    });
}

/// Resolve the icon bytes + whether to render it as a macOS template image
/// (template = adapts to the light/dark menu bar automatically).
/// - "color"      -> full vinyl, not a template
/// - "mono-light" -> white glyph (`tray-dark`), not a template
/// - "mono-dark"  -> black glyph (`tray-light`), not a template
/// - "auto"/other -> black glyph as a template, so macOS adapts it
fn icon_for(theme: &str) -> (&'static [u8], bool) {
    match theme {
        "color" => (ICON_COLOR, false),
        "mono-light" => (ICON_WHITE, false),
        "mono-dark" => (ICON_BLACK, false),
        _ => (ICON_BLACK, true),
    }
}

/// Build an `NSImage` from PNG bytes, marking it a template image when asked.
fn make_image(bytes: &[u8], is_template: bool) -> Option<Retained<NSImage>> {
    let data = NSData::with_bytes(bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    unsafe { image.setTemplate(is_template) };
    // The PNG assets are 44px (22pt @2x). Without an explicit point size the
    // menu bar renders them at native pixel size -> a giant icon. Pin to the
    // standard menu-bar glyph box (18pt; the bar is 22pt tall).
    unsafe { image.setSize(NSSize::new(18.0, 18.0)) };
    Some(image)
}

/// Apply the resolved icon to the status item's button.
fn apply_icon(status_item: &NSStatusItem, theme: &str, mtm: MainThreadMarker) {
    let (bytes, is_template) = icon_for(theme);
    let Some(image) = make_image(bytes, is_template) else {
        log::error!("[tray] failed to decode menu-bar icon");
        return;
    };
    if let Some(button) = unsafe { status_item.button(mtm) } {
        unsafe { button.setImage(Some(&image)) };
    }
}

/// Build the menu-bar item + menu. MUST run on the main thread — reach it
/// through `tray_bridge::ui`, never directly from the tokio runtime.
///
/// Returns whether the item was created, so `tray_qt::init` can set `TRAY` (and
/// therefore `trayLive`) only when there really is a tray. Getting that wrong
/// is what makes close-to-tray hide into nothing.
pub(crate) fn create(theme_override: &str) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("[tray] create called off the main thread");
        return false;
    };

    // The action target. Held alive in a thread_local so the menu's weak
    // target reference stays valid.
    let target = QbzTrayMenuTarget::new(mtm);
    let target_obj: &AnyObject = &target;
    let action = sel!(onMenuItem:);

    // The menu: 3 transport items, separator, show/hide, separator, quit.
    let menu = NSMenu::new(mtm);
    let empty_key = NSString::from_str("");
    let make_item = |title: &str, tag: NSInteger| -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(title),
                Some(action),
                &empty_key,
            )
        };
        unsafe {
            item.setTarget(Some(target_obj));
            item.setTag(tag);
            item.setEnabled(true);
        }
        item
    };

    menu.addItem(&make_item(&qbz_i18n::t("Play/Pause"), TAG_PLAY_PAUSE));
    menu.addItem(&make_item(&qbz_i18n::t("Next Track"), TAG_NEXT));
    menu.addItem(&make_item(&qbz_i18n::t("Previous Track"), TAG_PREVIOUS));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&make_item(&qbz_i18n::t("Show/Hide Window"), TAG_SHOW_HIDE));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&make_item(&qbz_i18n::t("Quit QBZ"), TAG_QUIT));

    // Build the status item and wire the icon.
    let status_bar = unsafe { NSStatusBar::systemStatusBar() };
    let status_item = unsafe { status_bar.statusItemWithLength(NSVariableStatusItemLength) };
    apply_icon(&status_item, theme_override, mtm);

    // Do NOT attach the menu permanently (that makes any click open it). Give
    // the status button its own action that fires on left AND right mouse-up;
    // `handle_status_click` decides toggle-vs-menu.
    if let Some(button) = unsafe { status_item.button(mtm) } {
        unsafe {
            button.setTarget(Some(target_obj));
            button.setAction(Some(sel!(onStatusButton:)));
            button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        }
    }

    STATUS_ITEM.with(|s| *s.borrow_mut() = Some(status_item));
    MENU_TARGET.with(|t| *t.borrow_mut() = Some(target));
    MENU.with(|m| *m.borrow_mut() = Some(menu));

    // A bare `cargo run` binary is NOT a bundled .app. Without an explicit
    // Regular activation policy + activation, macOS treats the app as a
    // background process and `[NSApp sendAction:]` may not route the menu item
    // target-action. Force Regular + active.
    ensure_regular_active_app(mtm);

    log::info!("[tray] menu-bar item initialized (theme={theme_override})");
    true
}

/// Re-theme the live menu-bar icon. Main thread only.
pub(crate) fn set_icon_theme(theme: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATUS_ITEM.with(|s| {
        if let Some(status_item) = s.borrow().as_ref() {
            apply_icon(status_item, theme, mtm);
        }
    });
}

/// Force the app to a Regular, active application so macOS dispatches the
/// `NSStatusItem` menu-item actions. Main thread only.
///
/// This deliberately overlaps `tray_qt::set_mac_dock_hidden`, which flips the
/// SAME policy to `.accessory` for the Hide-Dock setting. No conflict: this
/// runs once at tray creation, while the window is up and Regular is correct;
/// the Hide-Dock arm only fires on a close-to-tray afterwards.
fn ensure_regular_active_app(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}
