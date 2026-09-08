//! QbzDisco — Discography Builder domain bridge (spec 02 §4.3).
//!
//! The module is `qbz_disco_bridge`; bridge modules are never named after a
//! workspace crate (library_bridge.rs:1-6 precedent). The QML type name comes
//! from the QObject (`QbzDisco`), not from the module.
//!
//! A SEPARATE singleton from `QbzMyQbz` on purpose: the builder is reached from
//! the ARTIST page (ArtistView's ⋯ overflow → "Create Artist Collection"), owns
//! an artist-scoped session of its own, and never coexists with the MyQBZ views.
//! It is also the ONLY creator of `kind == artist_collection`, which both grids
//! and the detail render.
//!
//! Props: the one builder document. No `#[qsignal]` — outcomes are toasts
//! (toast_qt.rs, and note that "Artist Collection created" / "Failed to create
//! collection" are RAW English literals in the reference, absent from all eight
//! catalogs, reproduced verbatim) and state changes are property republishes.
//!
//! Route id: the builder view mounts on `QbzShell.currentView == "discobuilder"`
//! (orchestrator's canonical route table; spec 02's `"disco"` is superseded by
//! spec 01 §1.1). The `nav_qt::record` call lives in the controller's `open`, not
//! here.
//!
//! Deliberately NOT bridge members: `open-type-menu-key` (pure popover
//! bookkeeping — the Qt popup owns its own open state) and
//! `name-trimmed-empty` / `name-changed` (Slint cannot `trim()`; QML can, and
//! `create(name)` takes the name directly).

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_disco_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // ONE JSON document (myqbz_builder_qt.rs DiscoDoc: artist header,
        // loading/error, order, tri-state header flags, footer counts and the
        // grouped candidates with their alternates).
        #[qproperty(QString, disco_json)]
        type QbzDisco = super::QbzDiscoRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzDisco>);

        /// Entry point from ArtistView's ⋯ overflow: nav-record the route, reset
        /// the session, fetch the three sources (Qobuz first — it resolves the
        /// artist name — then local + Plex), group and publish.
        #[qinvokable]
        fn open(self: Pin<&mut QbzDisco>, artist_id: QString);

        /// Cancel button — plain history back.
        #[qinvokable]
        fn back(self: Pin<&mut QbzDisco>);

        /// `order_by` ∈ release_date | title | manual.
        #[qinvokable]
        fn set_order(self: Pin<&mut QbzDisco>, order_by: QString);

        /// Per-candidate checkbox (`cand_key` is "{source}|{sourceItemId}").
        #[qinvokable]
        fn toggle_checked(self: Pin<&mut QbzDisco>, group_key: QString, cand_key: QString);

        /// Header tri-state: touches non-compilation PRIMARIES only —
        /// alternates and compilations keep whatever the user set.
        #[qinvokable]
        fn toggle_all(self: Pin<&mut QbzDisco>);

        /// Title click → the album page, when the id is non-blank.
        #[qinvokable]
        fn open_album(self: Pin<&mut QbzDisco>, source: QString, source_item_id: QString);

        /// Release-type override: writes the process-global sidecar and
        /// republishes. `choice` ∈ album | ep | single | live | compilation.
        #[qinvokable]
        fn set_type_override(
            self: Pin<&mut QbzDisco>,
            source: QString,
            source_item_id: QString,
            choice: QString,
        );

        /// Clears one override (back to the classifier's verdict).
        #[qinvokable]
        fn reset_type_override(self: Pin<&mut QbzDisco>, source: QString, source_item_id: QString);

        /// Create the artist_collection: trimmed-empty name ⇒ the raw literal
        /// "Artist Collection" (untranslated, 1:1 with myqbz_builder.rs:858),
        /// then navigate to the new collection's detail.
        #[qinvokable]
        fn create(self: Pin<&mut QbzDisco>, name: QString);
    }

    impl cxx_qt::Threading for QbzDisco {}
}

use qbz_disco_bridge::QbzDisco;

/// Rust side of the Discography Builder bridge (plain storage, phase-1 pattern).
pub struct QbzDiscoRust {
    disco_json: QString,
}

impl Default for QbzDiscoRust {
    fn default() -> Self {
        Self {
            // Parseable default so QML's JSON.parse never throws on frame 1
            // (home_bridge.rs:250-257).
            disco_json: QString::from("{}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzDisco>> = OnceLock::new();

/// Queue a builder-bridge mutation onto the Qt event loop (no-op before boot
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzDisco>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_disco_bridge::QbzDisco {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] disco Qt thread already registered");
        }
        // The builder has no session until an artist page opens it, and the
        // empty document is already seeded in Default.
    }

    pub fn open(self: Pin<&mut Self>, artist_id: QString) {
        crate::myqbz_builder_qt::open(artist_id.to_string());
    }

    pub fn back(self: Pin<&mut Self>) {
        crate::myqbz_builder_qt::back();
    }

    pub fn set_order(self: Pin<&mut Self>, order_by: QString) {
        crate::myqbz_builder_qt::set_order(&order_by.to_string());
    }

    pub fn toggle_checked(self: Pin<&mut Self>, group_key: QString, cand_key: QString) {
        crate::myqbz_builder_qt::toggle_checked(&group_key.to_string(), &cand_key.to_string());
    }

    pub fn toggle_all(self: Pin<&mut Self>) {
        crate::myqbz_builder_qt::toggle_all();
    }

    pub fn open_album(self: Pin<&mut Self>, source: QString, source_item_id: QString) {
        crate::myqbz_builder_qt::open_album(&source.to_string(), &source_item_id.to_string());
    }

    pub fn set_type_override(
        self: Pin<&mut Self>,
        source: QString,
        source_item_id: QString,
        choice: QString,
    ) {
        crate::myqbz_builder_qt::set_type_override(
            &source.to_string(),
            &source_item_id.to_string(),
            &choice.to_string(),
        );
    }

    pub fn reset_type_override(self: Pin<&mut Self>, source: QString, source_item_id: QString) {
        crate::myqbz_builder_qt::reset_type_override(
            &source.to_string(),
            &source_item_id.to_string(),
        );
    }

    pub fn create(self: Pin<&mut Self>, name: QString) {
        crate::myqbz_builder_qt::create(name.to_string());
    }
}
