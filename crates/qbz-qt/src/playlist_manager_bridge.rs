//! QbzPlaylistManager — the Playlist Manager's QML singleton.
//!
//! One of THREE singletons this surface owns (contract D2), not one god
//! object: this one holds the manager document, the canonical folder list and
//! the playlist-ORGANISATION writes (favourite, hidden, move-to-folder,
//! reorder). `QbzFolderEdit` owns the folder modals and `QbzPlaylistEdit` owns
//! the shared rename/description/delete modal, so a folder save can never
//! perturb an open playlist editor and neither of them couples to the
//! manager's own state.
//!
//! Delegates only — every body here is one line into `playlist_manager_qt` /
//! `playlist_manager_ops`. Two properties are read by surfaces OUTSIDE the
//! manager view: `foldersJson` feeds the sidebar's row context menu and both
//! move-to-folder menus, and it is kept fresh by
//! `playlist_manager_qt::refresh_folders()`, which never consults the manager
//! cache (D5).
//!
//! **No `#[qsignal]`** on any of the three bridges — outcomes are republishes
//! and toasts (the `playlist_picker_bridge.rs:13` convention).
//!
//! The module is `qbz_playlist_manager_bridge`, never `qbz_playlist_manager`
//! and never `qbz_playlist`: a bridge module named after a workspace crate is
//! E0659. The QML type name comes from the QObject, so QML is unaffected.
//!
//! Every id crossing this boundary is a **string**, and a `local:<uuid>` must
//! survive the round trip untouched — nothing on either side may parse one as
//! an integer.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_playlist_manager_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // The manager document (playlist_manager_qt.rs ManagerDoc: toolbar
        // state, the two counters, the filtered+sorted flat set and the
        // flattened tree).
        #[qproperty(QString, manager_json)]
        // The CANONICAL folder array — every consumer reads this one property,
        // including the two surfaces that exist before the manager document
        // does (the sidebar's row menu and its move-to-folder list). Default
        // "[]" so a pre-publish `JSON.parse` yields an empty array, not a throw.
        #[qproperty(QString, folders_json)]
        // Written by the LOAD path ONLY: navigate() / reload() set it true and
        // the load's terminal rebuild clears it. The document publish never
        // touches it — see playlist_manager_qt's header for what breaks when
        // the two are conflated.
        #[qproperty(bool, pm_loading)]
        type QbzPlaylistManager = super::QbzPlaylistManagerRust;

        /// Registers this object's Qt-thread hop. Until Main.qml calls it,
        /// every `ui()` post is a SILENT no-op — the view mounts, every
        /// publish is dropped, and nothing is logged on either side.
        #[qinvokable]
        fn boot(self: Pin<&mut QbzPlaylistManager>);

        /// Open the manager: record the route, raise the spinner, publish the
        /// toolbar. Does NOT load — the view's `Component.onCompleted` does.
        #[qinvokable]
        fn navigate(self: Pin<&mut QbzPlaylistManager>);

        /// The single load (network + one blocking DB hop), from the view's
        /// `Component.onCompleted`. That is also the back/forward re-entry
        /// path, where `navigate()` never ran.
        #[qinvokable]
        fn reload(self: Pin<&mut QbzPlaylistManager>);

        /// Search-as-you-type. The field's text is QML-local; this stores the
        /// query at once and coalesces the rebuild on a 150 ms debounce.
        #[qinvokable]
        fn search_changed(self: Pin<&mut QbzPlaylistManager>, query: QString);

        /// Visibility filter: `"all" | "visible" | "hidden"`.
        #[qinvokable]
        fn set_filter(self: Pin<&mut QbzPlaylistManager>, value: QString);

        /// Sort option. Re-selecting the ACTIVE one flips the direction (#657).
        #[qinvokable]
        fn set_sort(self: Pin<&mut QbzPlaylistManager>, value: QString);

        /// View mode: `"grid" | "list" | "tree"`.
        #[qinvokable]
        fn set_view_mode(self: Pin<&mut QbzPlaylistManager>, value: QString);

        /// Folder mode on/off; leaving it while in the tree falls back to grid.
        #[qinvokable]
        fn toggle_folder_mode(self: Pin<&mut QbzPlaylistManager>);

        /// Collapse / expand the FOLDERS section (the only collapsible one).
        #[qinvokable]
        fn toggle_folders_collapsed(self: Pin<&mut QbzPlaylistManager>);

        /// Grid/list folder-card navigation. Editing remains a separate
        /// QbzFolderEdit action owned exclusively by the pencil affordance.
        #[qinvokable]
        fn open_folder(self: Pin<&mut QbzPlaylistManager>, folder_id: QString);

        /// Return from the current grid/list folder to the manager root.
        #[qinvokable]
        fn close_folder(self: Pin<&mut QbzPlaylistManager>);

        /// Expand / collapse one folder in the tree.
        #[qinvokable]
        fn toggle_tree_folder(self: Pin<&mut QbzPlaylistManager>, folder_id: QString);

        /// Flip the favourite flag. Both id kinds; no sidebar refresh.
        #[qinvokable]
        fn toggle_favorite(self: Pin<&mut QbzPlaylistManager>, id: QString);

        /// Flip the hidden flag. Both id kinds; refreshes the sidebar through
        /// the verb that also runs offline.
        #[qinvokable]
        fn toggle_hidden(self: Pin<&mut QbzPlaylistManager>, id: QString);

        /// Move a playlist into a folder. `""` as `folder_id` means ROOT.
        #[qinvokable]
        fn move_to_folder(
            self: Pin<&mut QbzPlaylistManager>,
            playlist_id: QString,
            folder_id: QString,
        );

        /// Custom-order arrow up (Qobuz rows only — see `reorder_step`).
        #[qinvokable]
        fn move_up(self: Pin<&mut QbzPlaylistManager>, id: QString);

        /// Custom-order arrow down.
        #[qinvokable]
        fn move_down(self: Pin<&mut QbzPlaylistManager>, id: QString);
    }

    impl cxx_qt::Threading for QbzPlaylistManager {}
}

use qbz_playlist_manager_bridge::QbzPlaylistManager;

/// Rust side of the manager bridge (plain storage — the controller owns every
/// piece of real state).
pub struct QbzPlaylistManagerRust {
    manager_json: QString,
    folders_json: QString,
    pm_loading: bool,
}

impl Default for QbzPlaylistManagerRust {
    fn default() -> Self {
        Self {
            // Parseable defaults so QML's guarded JSON.parse never throws on
            // frame 1, and so `doc.playlists` is `undefined` rather than a
            // crash before the first publish.
            manager_json: QString::from("{}"),
            folders_json: QString::from("[]"),
            // FALSE: the spinner is raised by navigate()/reload(), not by the
            // default. A default of true would strand the view spinning in any
            // session where neither ran.
            pm_loading: false,
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzPlaylistManager>> = OnceLock::new();

/// Queue a manager mutation onto the Qt event loop (no-op before `boot()`
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzPlaylistManager>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_playlist_manager_bridge::QbzPlaylistManager {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] playlist manager Qt thread already registered");
        }
        crate::playlist_manager_qt::boot();
    }

    pub fn navigate(self: Pin<&mut Self>) {
        crate::playlist_manager_qt::navigate();
    }

    pub fn reload(self: Pin<&mut Self>) {
        crate::playlist_manager_qt::reload();
    }

    pub fn search_changed(self: Pin<&mut Self>, query: QString) {
        crate::playlist_manager_qt::search_changed(&query.to_string());
    }

    pub fn set_filter(self: Pin<&mut Self>, value: QString) {
        crate::playlist_manager_qt::set_filter(&value.to_string());
    }

    pub fn set_sort(self: Pin<&mut Self>, value: QString) {
        crate::playlist_manager_qt::set_sort(&value.to_string());
    }

    pub fn set_view_mode(self: Pin<&mut Self>, value: QString) {
        crate::playlist_manager_qt::set_view_mode(&value.to_string());
    }

    pub fn toggle_folder_mode(self: Pin<&mut Self>) {
        crate::playlist_manager_qt::toggle_folder_mode();
    }

    pub fn toggle_folders_collapsed(self: Pin<&mut Self>) {
        crate::playlist_manager_qt::toggle_folders_collapsed();
    }

    pub fn open_folder(self: Pin<&mut Self>, folder_id: QString) {
        crate::playlist_manager_qt::open_folder(&folder_id.to_string());
    }

    pub fn close_folder(self: Pin<&mut Self>) {
        crate::playlist_manager_qt::close_folder();
    }

    pub fn toggle_tree_folder(self: Pin<&mut Self>, folder_id: QString) {
        crate::playlist_manager_qt::toggle_tree_folder(&folder_id.to_string());
    }

    pub fn toggle_favorite(self: Pin<&mut Self>, id: QString) {
        crate::playlist_manager_ops::toggle_favorite(&id.to_string());
    }

    pub fn toggle_hidden(self: Pin<&mut Self>, id: QString) {
        crate::playlist_manager_ops::toggle_hidden(&id.to_string());
    }

    pub fn move_to_folder(self: Pin<&mut Self>, playlist_id: QString, folder_id: QString) {
        crate::playlist_manager_ops::move_to_folder(
            &playlist_id.to_string(),
            &folder_id.to_string(),
        );
    }

    pub fn move_up(self: Pin<&mut Self>, id: QString) {
        crate::playlist_manager_ops::move_up(&id.to_string());
    }

    pub fn move_down(self: Pin<&mut Self>, id: QString) {
        crate::playlist_manager_ops::move_down(&id.to_string());
    }
}
