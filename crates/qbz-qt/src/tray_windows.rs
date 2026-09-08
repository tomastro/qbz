//! Windows notification-area tray, backed by `cxx/win_tray.cpp`.
//!
//! The sibling of `tray_linux.rs` (StatusNotifierItem) and `tray_macos.rs`
//! (NSStatusItem). All three drive the same `tray_qt` dispatchers and, since
//! this one reuses their five `qbz_i18n` keys, present the same menu.
//!
//! GUI THREAD ONLY. The icon owns a hidden window with a message loop, so a
//! call from tokio would post to a thread that never pumps.
#![cfg(target_os = "windows")]

unsafe extern "C" {
    fn qbz_win_tray_set_labels(
        play_pause: *const u16,
        next: *const u16,
        previous: *const u16,
        show_hide: *const u16,
        quit: *const u16,
    );
    fn qbz_win_tray_set_callbacks(
        on_left_click: extern "C" fn(),
        on_play_pause: extern "C" fn(),
        on_next: extern "C" fn(),
        on_previous: extern "C" fn(),
        on_quit: extern "C" fn(),
    );
    fn qbz_win_tray_create(tooltip: *const u16) -> bool;
    fn qbz_win_tray_set_tooltip(tooltip: *const u16);
    fn qbz_win_tray_set_playing(playing: bool);
    fn qbz_win_tray_destroy();
}

/// NUL-terminated UTF-16, the only string shape the Win32 `W` APIs take.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Truncate to at most `max_units` UTF-16 CODE UNITS, never mid-character.
///
/// The Win32 buffers are sized in `wchar_t`, i.e. UTF-16 units, and an emoji
/// or any astral character costs two of them. Bounding by `chars()` counts
/// scalar values instead, so a 120-char string can be 240 units; `wcsncpy_s`
/// with `_TRUNCATE` then cuts at the unit boundary and can keep a HIGH
/// SURROGATE with its partner gone -- malformed UTF-16 handed to the shell.
///
/// Stepping whole `char`s and paying `len_utf16()` for each cannot split a
/// pair, because a pair is one `char` here.
fn truncate_utf16(s: &str, max_units: usize) -> String {
    let mut out = String::new();
    let mut units = 0usize;
    for ch in s.chars() {
        let n = ch.len_utf16();
        if units + n > max_units {
            break;
        }
        out.push(ch);
        units += n;
    }
    out
}

// The C ABI callbacks. Each is the same dispatcher the Linux and macOS menus
// call, so a tray action behaves identically on all three.
extern "C" fn on_left_click() {
    crate::tray_qt::present();
}
extern "C" fn on_play_pause() {
    crate::tray_qt::dispatch_play_pause();
}
extern "C" fn on_next() {
    crate::tray_qt::dispatch_next();
}
extern "C" fn on_previous() {
    crate::tray_qt::dispatch_previous();
}
extern "C" fn on_quit() {
    crate::tray_qt::quit();
}

/// Create the icon. `false` means no tray exists, and the caller MUST leave
/// the handle empty when it does: `should_hide_on_close` consults that handle,
/// so pretending a failed tray succeeded makes close-to-tray hide the window
/// with nothing left to bring it back.
pub(crate) fn create() -> bool {
    // Labels first -- the C side copies them, and the menu is built on demand
    // from whatever it last received.
    // The C side holds each label in a 64-unit buffer, so 63 plus the
    // terminator. Truncated HERE, where char boundaries are known, rather than
    // by `wcsncpy_s` in C, which counts units and would happily halve a
    // surrogate pair.
    const LABEL_UNITS: usize = 63;
    let play_pause = wide(&truncate_utf16(&qbz_i18n::t("Play/Pause"), LABEL_UNITS));
    let next = wide(&truncate_utf16(&qbz_i18n::t("Next Track"), LABEL_UNITS));
    let previous = wide(&truncate_utf16(&qbz_i18n::t("Previous Track"), LABEL_UNITS));
    let show_hide = wide(&truncate_utf16(
        &qbz_i18n::t("Show/Hide Window"),
        LABEL_UNITS,
    ));
    let quit = wide(&truncate_utf16(&qbz_i18n::t("Quit QBZ"), LABEL_UNITS));

    // SAFETY: five NUL-terminated UTF-16 buffers, alive across the call, which
    // copies them into its own storage.
    unsafe {
        qbz_win_tray_set_labels(
            play_pause.as_ptr(),
            next.as_ptr(),
            previous.as_ptr(),
            show_hide.as_ptr(),
            quit.as_ptr(),
        );
    }

    // SAFETY: all five are `extern "C"` fns with no captured state, valid for
    // the life of the process.
    unsafe {
        qbz_win_tray_set_callbacks(on_left_click, on_play_pause, on_next, on_previous, on_quit);
    }

    let tooltip = wide("QBZ");
    // SAFETY: NUL-terminated and alive across the call.
    unsafe { qbz_win_tray_create(tooltip.as_ptr()) }
}

/// Hover text, bounded to what `szTip` holds: 128 UTF-16 units including the
/// terminator.
pub(crate) fn set_tooltip(title: &str, desc: &str) {
    let joined = if desc.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n{desc}")
    };
    // szTip is 128 units INCLUDING the terminator. Track titles are full of
    // non-ASCII and occasionally astral characters, which is exactly where a
    // unit-counting truncation goes wrong.
    let s = truncate_utf16(&joined, 127);
    let w = wide(&s);
    // SAFETY: NUL-terminated and alive across the call.
    unsafe { qbz_win_tray_set_tooltip(w.as_ptr()) }
}

pub(crate) fn set_playing(playing: bool) {
    // SAFETY: a plain bool by value.
    unsafe { qbz_win_tray_set_playing(playing) }
}

#[allow(dead_code)] // wired when a Windows quit path needs to remove the icon
pub(crate) fn destroy() {
    // SAFETY: no arguments; idempotent on the C side.
    unsafe { qbz_win_tray_destroy() }
}
