//! QbzPlaylistImport — the "Import Playlist" modal's bridge, port of the
//! `PlaylistImportState` / `PlaylistImportActions` globals in the reference's
//! `ui/state.slint:5821-5871` and the handler arms in its `main.rs`.
//!
//! A SEPARATE singleton from `QbzPlaylist` and from `QbzPlaylistManager`: the
//! importer is its own domain (an external-provider scraper + a Qobuz matcher),
//! it is reachable from two shell surfaces that outlive each other (the sidebar
//! `...` menu and the closed-sidebar flyout), and its modal must survive the
//! surface that opened it — `05 §5.8.5` states that requirement explicitly.
//!
//! Props: ONE JSON document (`playlist_import_qt.rs ImportDoc`) covering all
//! of the modal's states — URL entry, provider detection, the preview /
//! customisation panel, the live progress panel with its append-only log, and
//! the summary block. No `#[qsignal]`: outcomes are toasts, a sidebar refresh
//! and a navigation, all raised by the controller itself (`05 §5.8.6`).
//!
//! The module is `qbz_playlist_import_bridge`, never `qbz_playlist_import` —
//! that is the workspace crate's name and would be an E0659 ambiguity
//! (`05 §5.8.1`; the `library_bridge.rs:1-6` precedent). The QML type name
//! comes from the QObject, so QML is unaffected.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_playlist_import_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // ONE JSON document (playlist_import_qt.rs ImportDoc).
        #[qproperty(QString, import_json)]
        type QbzPlaylistImport = super::QbzPlaylistImportRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzPlaylistImport>);

        /// Open the modal, fully reset — NO ARGUMENTS (`05 §5.8.2`). The
        /// folder dropdown is read from library.db by the controller; the
        /// sidebar must not hand it in.
        #[qinvokable]
        fn open(self: Pin<&mut QbzPlaylistImport>);

        /// Hide the modal. Does NOT cancel an in-flight import (§1.8).
        #[qinvokable]
        fn close(self: Pin<&mut QbzPlaylistImport>);

        /// Every URL keystroke — Rust recomputes activeProvider / canFetch /
        /// showPreview (and the post-completion rearm path).
        #[qinvokable]
        fn url_edited(self: Pin<&mut QbzPlaylistImport>, text: QString);

        /// Every rename keystroke — keeps the controller's mirror fresh.
        #[qinvokable]
        fn name_edited(self: Pin<&mut QbzPlaylistImport>, text: QString);

        /// Folder dropdown selection (index into `folderIds`).
        #[qinvokable]
        fn set_folder_index(self: Pin<&mut QbzPlaylistImport>, index: i32);

        /// Step A — fetch the provider preview. No session required.
        #[qinvokable]
        fn fetch(self: Pin<&mut QbzPlaylistImport>);

        /// Step B — run the import (rename + folder choice), with live
        /// progress.
        #[qinvokable]
        fn execute(self: Pin<&mut QbzPlaylistImport>);

        // --- Source picker (2.0.3 expansion) -----------------------------
        //
        // Everything below feeds ONE `PlaylistSource`, which resolves to the
        // same `ImportPlaylist` the URL path always produced. The rename,
        // folder, progress, log and summary halves of this modal never learn
        // that a second kind of source exists.

        /// The picker changed (0 URL · 1 Playlist file · 2 JSON ·
        /// 3 ListenBrainz · 4 Last.fm). Everything the previous source
        /// contributed is dropped, and a service source prefills its handle
        /// from the connected account when there is one.
        #[qinvokable]
        fn source_changed(self: Pin<&mut QbzPlaylistImport>, index: i32);

        /// "Choose file…" — the native picker for the File and JSON sources.
        /// The size wall is enforced on the PATH before the read, so an
        /// oversize pick never reaches RAM.
        #[qinvokable]
        fn pick_file(self: Pin<&mut QbzPlaylistImport>);

        /// The ListenBrainz / Last.fm username-or-URL field, per keystroke.
        /// Recomputes the fetch gate, detects Last.fm profile-vs-playlist, and
        /// loads the ListenBrainz "created for you" list for a username.
        #[qinvokable]
        fn service_input_edited(self: Pin<&mut QbzPlaylistImport>, text: QString);

        /// Last.fm station picker (0 Library · 1 Mix · 2 Recommendations).
        #[qinvokable]
        fn set_station_index(self: Pin<&mut QbzPlaylistImport>, index: i32);

        /// ListenBrainz "created for you" picker.
        #[qinvokable]
        fn set_lb_playlist_index(self: Pin<&mut QbzPlaylistImport>, index: i32);
    }

    impl cxx_qt::Threading for QbzPlaylistImport {}
}

use qbz_playlist_import_bridge::QbzPlaylistImport;

/// Rust side of the importer bridge (plain storage, phase-1 pattern).
pub struct QbzPlaylistImportRust {
    import_json: QString,
}

impl Default for QbzPlaylistImportRust {
    fn default() -> Self {
        Self {
            // Parseable default so QML's JSON.parse never throws on frame 1.
            import_json: QString::from("{}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzPlaylistImport>> = OnceLock::new();

/// Queue an importer mutation onto the Qt event loop (no-op before boot
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzPlaylistImport>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_playlist_import_bridge::QbzPlaylistImport {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] playlist import Qt thread already registered");
        }
        // Closed until a host opens it, and the closed document is already
        // seeded in Default — nothing to publish here.
    }

    pub fn open(self: Pin<&mut Self>) {
        crate::playlist_import_qt::open();
    }

    pub fn close(self: Pin<&mut Self>) {
        crate::playlist_import_qt::close();
    }

    pub fn url_edited(self: Pin<&mut Self>, text: QString) {
        crate::playlist_import_qt::on_url_edited(&text.to_string());
    }

    pub fn name_edited(self: Pin<&mut Self>, text: QString) {
        crate::playlist_import_qt::on_name_edited(&text.to_string());
    }

    pub fn set_folder_index(self: Pin<&mut Self>, index: i32) {
        crate::playlist_import_qt::set_folder_index(index);
    }

    pub fn fetch(self: Pin<&mut Self>) {
        crate::playlist_import_qt::fetch();
    }

    pub fn execute(self: Pin<&mut Self>) {
        crate::playlist_import_qt::execute();
    }

    pub fn source_changed(self: Pin<&mut Self>, index: i32) {
        crate::playlist_import_qt::on_source_changed(index);
    }

    pub fn pick_file(self: Pin<&mut Self>) {
        crate::playlist_import_qt::pick_file();
    }

    pub fn service_input_edited(self: Pin<&mut Self>, text: QString) {
        crate::playlist_import_qt::on_service_input_edited(&text.to_string());
    }

    pub fn set_station_index(self: Pin<&mut Self>, index: i32) {
        crate::playlist_import_qt::set_station_index(index);
    }

    pub fn set_lb_playlist_index(self: Pin<&mut Self>, index: i32) {
        crate::playlist_import_qt::set_lb_playlist_index(index);
    }
}
