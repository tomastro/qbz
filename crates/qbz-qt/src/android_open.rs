//! Android replacement for the desktop-launcher based `open::that` API.

use std::ffi::OsStr;

unsafe extern "C" {
    fn qbz_qt_open_target(utf8: *const u8, len: usize) -> bool;
}

pub fn that(target: impl AsRef<OsStr>) -> Result<(), String> {
    let text = target.as_ref().to_string_lossy();
    // SAFETY: C++ copies this slice into a QString before hopping to Qt's GUI
    // thread, so no pointer escapes this call.
    let opened = unsafe { qbz_qt_open_target(text.as_ptr(), text.len()) };
    opened
        .then_some(())
        .ok_or_else(|| format!("Android could not open {text}"))
}
