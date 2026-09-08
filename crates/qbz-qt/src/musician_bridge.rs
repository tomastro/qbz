//! QbzMusician — the musician domain bridge (contract §13.2).
//!
//! Two documents and a publish counter, five invokables. The controller is
//! `musician_qt.rs`; this file is glue and nothing else, exactly like
//! `artist_bridge.rs`.
//!
//! The module is `qbz_musician_bridge`, not `qbz_musician`: bridge modules are
//! never named after a workspace crate (the `qbz_library` collision is the
//! documented precedent). The QML type name comes from the QObject
//! (`QbzMusician`), not from the module.
//!
//! # A new bridge is SEVEN artefacts, not one file
//!
//! This file · its `build.rs` `rust_files` entry · the `main.rs` `mod` line ·
//! the QML singleton registration (which IS the build.rs entry — `#[qml_element]
//! #[qml_singleton]` only takes effect for files listed there) · `Main.qml`'s
//! `boot()` call · `ContentRouter.qml`'s `musician` arm · the global modal
//! mount. Five of those are integration-owned files; this lane ships the two it
//! owns and hands the rest over as snippets. A bridge missing from `build.rs`
//! does not fail the build: its QML singleton simply does not exist, and every
//! `QbzMusician.foo()` becomes a runtime ReferenceError that `cargo check`
//! cannot see. `qml_singleton_xref.py` is the gate that catches it.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_musician_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        /// The MusicianPage document (route `musician`) — state, header,
        /// bands, the paged "Appears On" grid. Shape frozen in contract
        /// §13.2.
        #[qproperty(QString, musician_json)]
        /// The MusicianModal document. `""` — NOT `"{}"` — is the closed
        /// state, so the QML host tests the raw string before parsing it.
        /// Everything else on the bridge defaults to `"{}"` precisely so a
        /// `JSON.parse` on frame one cannot throw; this one is the deliberate
        /// exception and the host must honour it.
        #[qproperty(QString, modal_json)]
        /// Publish counter, bumped by every write to either document above.
        /// QML re-parses on the QString change already; this exists so a view
        /// can force a re-read when the text happens to be byte-identical
        /// (resolving the same musician twice), the same trick
        /// `QbzSession.trRev` plays.
        #[qproperty(i32, musician_rev)]
        type QbzMusician = super::QbzMusicianRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzMusician>);

        /// THE DISPATCHER — the one entry all six musician call sites use
        /// (artist network members / member-of / collaborators, album-credits
        /// performer, track-info performer, immersive track-info performer).
        ///
        /// Resolve the NAME (musicians carry no catalog id), then open the
        /// artist page, this page, or the modal according to confidence. The
        /// branch table and its two traps live on `musician_qt.rs`. **QML never
        /// branches on confidence** — a QML copy of that switch would be six
        /// copies of it.
        ///
        /// `role` is the caller's own: `roles[0] || "Band Member"` /
        /// literal `"Band"` / `roles[0] || "Collaborator"` /
        /// `roles[0] || "Performer"`. It is echoed verbatim into every
        /// appearance card's role badge, so it must be the per-row instrument
        /// where one exists — pass `modelData.role`, never the per-group key.
        ///
        /// `#[auto_cxx_name]` makes the QML name
        /// `QbzMusician.resolveAndOpen(name, role)`.
        #[qinvokable]
        fn resolve_and_open(self: Pin<&mut QbzMusician>, name: QString, role: QString);

        /// The page's "Load more" button. Every guard lives in Rust so a burst
        /// of clicks cannot queue two pages.
        #[qinvokable]
        fn load_more(self: Pin<&mut QbzMusician>);

        /// An "Appears On" card — straight through to the shell's album
        /// router, which records its own nav entry.
        #[qinvokable]
        fn open_album(self: Pin<&mut QbzMusician>, album_id: QString);

        /// A "Bands & Projects" chip. Takes the group's MBID; the controller
        /// looks the NAME up in the open document and runs it back through the
        /// dispatcher, because no MBID -> Qobuz resolver exists anywhere in the
        /// tree.
        #[qinvokable]
        fn open_band(self: Pin<&mut QbzMusician>, mbid: QString);

        /// All four modal dismissal paths (Escape, the X, the backdrop, a
        /// parent modal closing) land here. Clears `modalJson` only — the page
        /// document is left intact.
        #[qinvokable]
        fn close_modal(self: Pin<&mut QbzMusician>);
    }

    impl cxx_qt::Threading for QbzMusician {}
}

use qbz_musician_bridge::QbzMusician;

/// Rust side of the musician bridge (plain storage, phase-1 pattern).
pub struct QbzMusicianRust {
    musician_json: QString,
    modal_json: QString,
    musician_rev: i32,
}

impl Default for QbzMusicianRust {
    fn default() -> Self {
        Self {
            // "{}" and not "": the page JSON.parses this straight, and an empty
            // string throws into the catch arm on the very first frame.
            musician_json: QString::from("{}"),
            // "" IS the closed state for the modal — see the property doc.
            modal_json: QString::from(""),
            musician_rev: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzMusician>> = OnceLock::new();

/// Queue a musician-bridge mutation onto the Qt event loop (no-op before boot
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzMusician>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_musician_bridge::QbzMusician {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] musician Qt thread already registered");
        }
    }

    // One-line delegates into `musician_qt`, with no `crate::` forwarder in
    // main.rs: that indirection exists for paths that mutate main.rs's own state
    // (LAST_DETAIL, the nav latch). This domain owns its state in its own module
    // and records its own nav entry, so there is nothing for main.rs to
    // coordinate. `open_album` is the one exception and it delegates onward to
    // main.rs's router itself.

    pub fn resolve_and_open(self: Pin<&mut Self>, name: QString, role: QString) {
        crate::musician_qt::resolve_and_open(name.to_string(), role.to_string());
    }

    pub fn load_more(self: Pin<&mut Self>) {
        crate::musician_qt::load_more();
    }

    pub fn open_album(self: Pin<&mut Self>, album_id: QString) {
        crate::musician_qt::open_album(album_id.to_string());
    }

    pub fn open_band(self: Pin<&mut Self>, mbid: QString) {
        crate::musician_qt::open_band(mbid.to_string());
    }

    pub fn close_modal(self: Pin<&mut Self>) {
        crate::musician_qt::close_modal();
    }
}
