//! Shared in-app toast publisher — the Qt port of
//! `qbz-slint-common/src/toast.rs` (spec 02 §2.4).
//!
//! Contract, 1:1 with the reference:
//!
//! * ONE toast at a time — a new one REPLACES the current (`toast.rs:22-27`).
//!   Here that falls out of the model: there is a single document and a single
//!   monotonic `seq`, so a publish overwrites whatever was showing and the QML
//!   host restarts its auto-hide timer off the `seq` change.
//! * Auto-hide by kind — success 3000 · info 3000 · warning 4000 · error 5000.
//!   The delay itself lives in QML (`controls/QbzToast.qml`), keyed off `seq`,
//!   so Rust never owns a timer and never needs a hide round-trip.
//! * The global toggle is applied HERE, in Rust: non-error toasts honour
//!   `in_app_toasts`, errors ALWAYS show (`toast.rs:47-50`). The QML host does
//!   not read `QbzBridge.settingsJson` at all.
//!
//! Unlike the Slint version there is no thread rule: every publisher hops
//! through `shell_bridge::ui()`, so these are callable from the Qt thread, from
//! a `crate::spawn` task and from inside `spawn_blocking` alike. Before
//! `QbzShell.boot()` the hop is a silent no-op, same as every other bridge.
//!
use std::sync::atomic::{AtomicI32, Ordering};

use cxx_qt_lib::QString;
use serde::Serialize;

/// Monotonically increasing document id. QML shows a toast when this changes,
/// which is also what makes a repeat of the SAME message re-show.
static SEQ: AtomicI32 = AtomicI32::new(0);

/// The four kinds with live producers (icon + tint per kind).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

impl ToastKind {
    /// The wire value QML switches on.
    fn as_str(self) -> &'static str {
        match self {
            ToastKind::Success => "success",
            ToastKind::Info => "info",
            ToastKind::Warning => "warning",
            ToastKind::Error => "error",
        }
    }
}

/// `ToastDoc` → `QbzShell.toastJson` (spec 02 §5.9). Every field is a single
/// word, so there is nothing for `#[serde(rename)]` to camelCase here.
#[derive(Serialize)]
struct ToastDoc {
    seq: i32,
    kind: &'static str,
    message: String,
}

/// Publish a toast. Applies the `in_app_toasts` gate, then one `ui()` hop.
pub(crate) fn show(message: impl Into<String>, kind: ToastKind) {
    // Non-error toasts honour the global toggle; errors always show
    // (`toast.rs:47-50`). Read straight from ui_prefs.json through the public
    // accessor — `settings_qt::pref_bool` is THE reader for a bool pref and the
    // settings document itself is built from the same call
    // (`settings_qt.rs:1508`).
    if kind != ToastKind::Error && !crate::settings_qt::pref_bool("in_app_toasts", true) {
        return;
    }

    let doc = ToastDoc {
        seq: SEQ.fetch_add(1, Ordering::Relaxed).wrapping_add(1),
        kind: kind.as_str(),
        message: message.into(),
    };
    let json = serde_json::to_string(&doc).unwrap_or_else(|_| "{}".into());
    crate::shell_bridge::ui(move |mut b| {
        b.as_mut().set_toast_json(QString::from(json.as_str()));
    });
}

// ---- Convenience wrappers (same set as the reference) ----------------------

pub(crate) fn success(message: impl Into<String>) {
    show(message, ToastKind::Success);
}

pub(crate) fn info(message: impl Into<String>) {
    show(message, ToastKind::Info);
}

pub(crate) fn warning(message: impl Into<String>) {
    show(message, ToastKind::Warning);
}

pub(crate) fn error(message: impl Into<String>) {
    show(message, ToastKind::Error);
}
