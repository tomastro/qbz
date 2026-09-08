//! QbzMyQbzAdd — the app-wide "Add to Mixtape/Collection" picker (spec 02 §4.2).
//!
//! The module is `qbz_myqbz_add_bridge`, never `qbz_mixtape`: bridge modules are
//! never named after a workspace crate (library_bridge.rs:1-6 precedent). The
//! QML type name comes from the QObject, so QML is unaffected.
//!
//! A SEPARATE singleton from `QbzMyQbz` on purpose: this is the only MyQBZ
//! surface other domains' QML touches (TrackRow, PlayerBar, NowPlayingBarSmall
//! and the three Local Library bulk surfaces), so keeping it apart means none of
//! them can couple to detail state, and a detail-side publish can never perturb
//! an open picker.
//!
//! Props: the one picker document. No `#[qsignal]` — outcomes are toasts
//! (toast_qt.rs) and state changes are property republishes.
//!
//! The payload is a JSON ARRAY rather than per-caller invokables: Slint builds
//! `Vec<AddItem>` in Rust because Slint models cannot be read generically
//! (main.rs:2337,2369,5453 are three hand-written builders), while in Qt every
//! caller already holds the row as a parsed JSON object Rust produced, so
//! `JSON.stringify([...])` is one call with no Rust-side lookup table. Precedent
//! for a JSON argument: `QbzShell.sidebarArtworkWindow(urlsJson)`
//! (shell_bridge.rs:263).

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_myqbz_add_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // ONE JSON document (myqbz_add_qt.rs AddDoc: open/loading flags, header
        // strings, the create sub-panel and the collection rows).
        #[qproperty(QString, add_json)]
        type QbzMyQbzAdd = super::QbzMyQbzAddRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzMyQbzAdd>);

        /// Open the picker for a JSON ARRAY of AddItem objects
        /// (`{itemType, source, sourceItemId, title, subtitle?, artworkUrl?,
        /// year?, trackCount?}`). An empty array is a no-op
        /// (myqbz_add.rs:75-77). Plex rows pass `source: "local"` — there is no
        /// "plex" source (myqbz_add.rs:30,58).
        #[qinvokable]
        fn open(self: Pin<&mut QbzMyQbzAdd>, items_json: QString);

        /// Row click: insert every pending item into that collection with
        /// `allow_duplicate = false`, then toast the outcome.
        #[qinvokable]
        fn pick(self: Pin<&mut QbzMyQbzAdd>, collection_id: QString);

        /// Name substring filter — Rust re-filters its row cache and
        /// republishes.
        #[qinvokable]
        fn search(self: Pin<&mut QbzMyQbzAdd>, query: QString);

        /// Footer chip → create sub-panel (`kind` ∈ mixtape | collection).
        #[qinvokable]
        fn show_create(self: Pin<&mut QbzMyQbzAdd>, kind: QString);

        /// Back out of the create sub-panel to the picker.
        #[qinvokable]
        fn create_back(self: Pin<&mut QbzMyQbzAdd>);

        /// Create the collection, add the pending items, close, toast.
        #[qinvokable]
        fn create_and_add(self: Pin<&mut QbzMyQbzAdd>, kind: QString, name: QString);

        /// Close and clear the pending items.
        #[qinvokable]
        fn close(self: Pin<&mut QbzMyQbzAdd>);
    }

    impl cxx_qt::Threading for QbzMyQbzAdd {}
}

use qbz_myqbz_add_bridge::QbzMyQbzAdd;

/// Rust side of the add-picker bridge (plain storage, phase-1 pattern).
pub struct QbzMyQbzAddRust {
    add_json: QString,
}

impl Default for QbzMyQbzAddRust {
    fn default() -> Self {
        Self {
            // Parseable default so QML's JSON.parse never throws on frame 1
            // (home_bridge.rs:250-257).
            add_json: QString::from("{}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzMyQbzAdd>> = OnceLock::new();

/// Queue an add-picker mutation onto the Qt event loop (no-op before boot
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzMyQbzAdd>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_myqbz_add_bridge::QbzMyQbzAdd {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] myqbz add Qt thread already registered");
        }
        // The picker is closed until a host opens it, and the closed document is
        // already seeded in Default — nothing to publish here.
    }

    pub fn open(self: Pin<&mut Self>, items_json: QString) {
        crate::myqbz_add_qt::open(&items_json.to_string());
    }

    pub fn pick(self: Pin<&mut Self>, collection_id: QString) {
        crate::myqbz_add_qt::pick(&collection_id.to_string());
    }

    pub fn search(self: Pin<&mut Self>, query: QString) {
        crate::myqbz_add_qt::search(&query.to_string());
    }

    pub fn show_create(self: Pin<&mut Self>, kind: QString) {
        crate::myqbz_add_qt::show_create(&kind.to_string());
    }

    pub fn create_back(self: Pin<&mut Self>) {
        crate::myqbz_add_qt::create_back();
    }

    pub fn create_and_add(self: Pin<&mut Self>, kind: QString, name: QString) {
        crate::myqbz_add_qt::create_and_add(&kind.to_string(), &name.to_string());
    }

    pub fn close(self: Pin<&mut Self>) {
        crate::myqbz_add_qt::close();
    }
}
