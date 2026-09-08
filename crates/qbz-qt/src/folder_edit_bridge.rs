//! QbzFolderEdit — the playlist-FOLDER modals' QML singleton (contract §3.2).
//!
//! One of the THREE singletons the Playlist Manager surface owns (D2), not one
//! god object: `QbzPlaylistManager` holds the manager document and the
//! playlist-organisation writes, `QbzPlaylistEdit` owns the shared
//! rename/description/delete modal, and this one owns the two folder panels —
//! the sidebar's small "New folder" create panel and the full folder editor
//! (name · icon preset · colour · custom image · hidden · delete).
//!
//! It is a SEPARATE singleton for the same reason `QbzMyQbzAdd` is separate
//! from `QbzMyQbz`: its callers live outside the manager view. The sidebar's
//! `...` menu opens the create panel, the sidebar's row context menu opens the
//! editor and flips `hidden`, and in both of those sessions the manager view
//! has typically never been mounted — so nothing here may be conditional on
//! the manager's cache (§5.3).
//!
//! TWO documents, because the two panels are independent surfaces with
//! independent open flags (§4.4 / §4.5):
//!   * `createJson` — `{ open, creating }`. The NAME is not in it: text state
//!     is QML-local and `createSubmit(name)` takes it as an argument
//!     (`MyQbzModals.qml`'s convention). The create panel owns its own draft
//!     so reopening it can never show whatever was last typed in the editor.
//!   * `editJson` — the editor's seeds plus the two RUST-OWNED fields the file
//!     dialog writes (`hasCustomImage` / `customImagePath`), `busy`, and the
//!     preset + swatch constants, published rather than hardcoded in QML so
//!     the list has exactly one definition (§4.5).
//!
//! **No `#[qsignal]`** — outcomes are republishes and toasts
//! (`playlist_picker_bridge.rs:13`).
//!
//! `busy` HOLDS THE PANEL OPEN (D22). The reference closes the editor before
//! the write, which makes a failed create invisible and leaves its own `busy`
//! flag dead. Here a save or delete disarms the buttons, keeps the panel up,
//! and only closes on success; a failure stays open and toasts.
//!
//! The module is `qbz_folder_edit_bridge` — a bridge module is never named
//! after a workspace crate (E0659); the QML type name comes from the QObject,
//! so QML is unaffected.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_folder_edit_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // The small create panel: `{ "open": false, "creating": false }`.
        #[qproperty(QString, create_json)]
        // The full folder editor (§4.5). Parseable default so QML's
        // JSON.parse never throws on the pre-publish frame.
        #[qproperty(QString, edit_json)]
        type QbzFolderEdit = super::QbzFolderEditRust;

        /// Registers this object's Qt-thread hop. Without it every `ui()`
        /// publish from Rust is dropped on the floor, silently and forever:
        /// the panels mount, no document ever arrives, and nothing is logged
        /// on either side.
        #[qinvokable]
        fn boot(self: Pin<&mut QbzFolderEdit>);

        // -- create panel ------------------------------------------------
        /// Sidebar `...` -> "New folder": the small name-only create panel.
        #[qinvokable]
        fn open_create(self: Pin<&mut QbzFolderEdit>);

        /// Create the folder. The name is QML-local and arrives as an
        /// argument; an empty / whitespace-only name and a re-entrant submit
        /// are both refused.
        #[qinvokable]
        fn create_submit(self: Pin<&mut QbzFolderEdit>, name: QString);

        /// Dismiss the create panel. Refused while the create is in flight.
        #[qinvokable]
        fn create_close(self: Pin<&mut QbzFolderEdit>);

        // -- editor ------------------------------------------------------
        /// Manager toolbar -> "New folder": the FULL editor in create mode
        /// (`id == ""`, title "New folder", no Delete button). The toolbar and
        /// the sidebar deliberately open two different surfaces — that is the
        /// reference's split, not an accident.
        #[qinvokable]
        fn open_new_folder(self: Pin<&mut QbzFolderEdit>);

        /// Open the editor on an existing folder. Reads
        /// `folders_qt::load_folders_full()` DIRECTLY — never a manager cache,
        /// which is filled only by a network fetch and is empty in exactly the
        /// sessions the sidebar's Edit row runs in.
        #[qinvokable]
        fn open_editor(self: Pin<&mut QbzFolderEdit>, folder_id: QString);

        /// Native file picker for a custom folder image. Cancel is a silent
        /// no-op (reference parity).
        #[qinvokable]
        fn pick_image(self: Pin<&mut QbzFolderEdit>);

        /// Drop the custom image and fall back to the preset glyph. Called by
        /// the panel alongside its own preset write — which is what makes the
        /// reference's declared-but-never-called `clear-image` callback live.
        #[qinvokable]
        fn clear_image(self: Pin<&mut QbzFolderEdit>);

        /// Persist the editor. Name / preset / colour / hidden are QML-local
        /// drafts and arrive as arguments; the image path and the folder id
        /// are Rust-owned state, so a republish can never make the panel save
        /// under the wrong id.
        #[qinvokable]
        fn edit_save(
            self: Pin<&mut QbzFolderEdit>,
            name: QString,
            icon_preset: QString,
            icon_color: QString,
            is_hidden: bool,
        );

        /// Delete the open folder. The confirm sub-modal is QML-local
        /// (`QbzConfirmModal`); Rust only ever sees the confirmed outcome.
        #[qinvokable]
        fn edit_delete_confirmed(self: Pin<&mut QbzFolderEdit>);

        /// Dismiss the editor. Refused while a save or delete is in flight.
        #[qinvokable]
        fn edit_close(self: Pin<&mut QbzFolderEdit>);

        /// Sidebar row menu -> "Hide from sidebar" on a FOLDER row. Flips the
        /// flag without opening anything, leaving every other field untouched
        /// (`set_folder_hidden`'s whole reason to exist).
        #[qinvokable]
        fn set_hidden(self: Pin<&mut QbzFolderEdit>, folder_id: QString, hidden: bool);
    }

    impl cxx_qt::Threading for QbzFolderEdit {}
}

use qbz_folder_edit_bridge::QbzFolderEdit;

/// Rust side of the folder-edit bridge (plain storage, phase-1 pattern).
pub struct QbzFolderEditRust {
    create_json: QString,
    edit_json: QString,
}

impl Default for QbzFolderEditRust {
    fn default() -> Self {
        Self {
            // Both panels start closed, and both defaults are parseable so a
            // binding that reads `doc.open` on frame 1 cannot throw.
            create_json: QString::from("{\"open\":false,\"creating\":false}"),
            edit_json: QString::from("{\"open\":false}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzFolderEdit>> = OnceLock::new();

/// Queue a folder-modal mutation onto the Qt event loop (no-op before `boot()`
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzFolderEdit>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_folder_edit_bridge::QbzFolderEdit {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] folder edit Qt thread already registered");
        }
        // Publish the closed documents synchronously, so the preset + swatch
        // constants are on the QML side before the first open rather than one
        // event-loop turn after it.
        crate::folder_edit_qt::publish_all();
    }

    pub fn open_create(self: Pin<&mut Self>) {
        crate::folder_edit_qt::open_create();
    }

    pub fn create_submit(self: Pin<&mut Self>, name: QString) {
        crate::folder_edit_qt::create_submit(&name.to_string());
    }

    pub fn create_close(self: Pin<&mut Self>) {
        crate::folder_edit_qt::create_close();
    }

    pub fn open_new_folder(self: Pin<&mut Self>) {
        crate::folder_edit_qt::open_new_folder();
    }

    pub fn open_editor(self: Pin<&mut Self>, folder_id: QString) {
        crate::folder_edit_qt::open_editor(&folder_id.to_string());
    }

    pub fn pick_image(self: Pin<&mut Self>) {
        crate::folder_edit_qt::pick_image();
    }

    pub fn clear_image(self: Pin<&mut Self>) {
        crate::folder_edit_qt::clear_image();
    }

    pub fn edit_save(
        self: Pin<&mut Self>,
        name: QString,
        icon_preset: QString,
        icon_color: QString,
        is_hidden: bool,
    ) {
        crate::folder_edit_qt::edit_save(
            &name.to_string(),
            &icon_preset.to_string(),
            &icon_color.to_string(),
            is_hidden,
        );
    }

    pub fn edit_delete_confirmed(self: Pin<&mut Self>) {
        crate::folder_edit_qt::edit_delete_confirmed();
    }

    pub fn edit_close(self: Pin<&mut Self>) {
        crate::folder_edit_qt::edit_close();
    }

    pub fn set_hidden(self: Pin<&mut Self>, folder_id: QString, hidden: bool) {
        crate::folder_edit_qt::set_hidden(&folder_id.to_string(), hidden);
    }
}
