//! Custom ALSA error handler.
//!
//! libasound's default error handler prints directly to stderr. During
//! device enumeration, probing PCMs that wrap `dmix` or `route` plugins
//! emits benign errors (e.g. "unable to open slave" when PipeWire holds
//! the hardware exclusively, "Found no matching channel map" when a
//! surround route rejects the probe layout). These appear in the app
//! log as noise even though enumeration tolerates them.
//!
//! This module installs a Rust-side handler that routes those errors
//! through the `log` crate at `debug!` level. They stay visible when
//! the user runs with `RUST_LOG=debug`, but do not pollute normal runs.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::sync::Once;

static INIT: Once = Once::new();

unsafe extern "C" fn handler(
    file: *const c_char,
    line: c_int,
    function: *const c_char,
    err: c_int,
    fmt: *const c_char,
) {
    let cstr = |p: *const c_char| -> &'static str {
        if p.is_null() {
            "?"
        } else {
            // SAFETY: libasound passes statically-allocated strings here.
            unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("?")
        }
    };

    // The `fmt` is a printf-style format; variadic args are not accessible
    // from stable Rust without a C shim. The format string alone is usually
    // descriptive enough for debug triage (e.g. "unable to open slave").
    log::debug!(
        "[ALSA] {}:{} ({}) err={}: {}",
        cstr(file),
        line,
        cstr(function),
        err,
        cstr(fmt)
    );
}

/// Install the custom ALSA error handler once per process. Subsequent
/// calls are no-ops.
pub fn install_once() {
    INIT.call_once(|| {
        // libasound's `snd_lib_error_handler_t` is a C variadic pointer
        // (`int, ..., const char *fmt, ...`). Stable Rust cannot _define_
        // a variadic extern fn, but a non-variadic callee that simply
        // ignores the varargs is ABI-compatible on every platform we
        // target (System V AMD64, AArch64 AAPCS, Windows x64 — all let
        // the callee leave extra-register/stack args untouched).
        //
        // SAFETY:
        //  - `snd_lib_error_set_handler` is process-global; gated by Once.
        //  - The transmute narrows the declared type but preserves the
        //    prefix argument list; we read the fixed args and never touch
        //    the varargs, so the convention difference is benign.
        type VariadicHandler = alsa_sys::snd_lib_error_handler_t;
        unsafe {
            let as_variadic: VariadicHandler =
                std::mem::transmute::<*const (), VariadicHandler>(handler as *const ());
            alsa_sys::snd_lib_error_set_handler(as_variadic);
        }
        log::info!("[ALSA] Custom error handler installed (routes to log::debug)");
    });
}
