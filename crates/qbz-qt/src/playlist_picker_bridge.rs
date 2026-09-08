//! QbzPlaylistPicker — the app-wide "Add to playlist" picker, port of
//! `crates/qbz/src/playlist_picker.rs` + `primitives/PlaylistPickerModal.slint`.
//!
//! A SEPARATE singleton from `QbzPlaylist` for the same reason `QbzMyQbzAdd` is
//! separate from `QbzMyQbz`: this is the surface every OTHER domain's QML
//! touches (the queue footer and row menu, the multi-select bars, the unified
//! per-track menu on every catalog surface, the Local Library rows, the local
//! playlist detail and both now-playing bars), so none of them couples to
//! playlist-detail state, and a detail republish can never perturb an open
//! picker.
//!
//! Three QML entry points, one per PAYLOAD KIND, because the kind is what the
//! whole module is built to keep unmistakable: `open_for_track` /
//! `open_for_tracks` take Qobuz catalog ids, `open_for_local_row` takes a
//! local-playlist DETAIL row and resolves its ref in Rust. Local LIBRARY rows
//! have their own resolver already and go through
//! `QbzLocal.bulkAction("track", …, "add-to-playlist")` (`local_bulk.rs`),
//! which is the only place that can turn a Tracks-table row id into a
//! source-aware ref.
//!
//! Props: one JSON document (`playlist_picker_qt.rs PickerDoc`), covering the
//! picker itself, the inline create panel and the duplicate-confirm sub-modal.
//! No `#[qsignal]` — outcomes are toasts and state changes are republishes.
//!
//! The module is `qbz_playlist_picker_bridge`, never `qbz_playlist`: a bridge
//! module is never named after a workspace crate (E0659; `library_bridge.rs:1-6`
//! precedent). The QML type name comes from the QObject, so QML is unaffected.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_playlist_picker_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // ONE JSON document (playlist_picker_qt.rs PickerDoc: open/loading/
        // creating flags, the playlist rows, the inline create panel and the
        // duplicate-confirm sub-modal).
        #[qproperty(QString, picker_json)]
        type QbzPlaylistPicker = super::QbzPlaylistPickerRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzPlaylistPicker>);

        /// Open the picker carrying ONE Qobuz catalog track id — the per-row
        /// "Add to playlist" entry of the unified track menu
        /// (`primitives/TrackContextMenu.slint:145`). The caller must have
        /// established that the id IS a catalog id; a LocalLibrary row id or a
        /// Plex synthetic id means a different track once it reaches Qobuz
        /// (see this file's sibling `open_for_local_row`, and the four-case
        /// matrix in `playlist_picker_qt.rs`'s header).
        #[qinvokable]
        fn open_for_track(self: Pin<&mut QbzPlaylistPicker>, track_id: QString);

        /// Same, for a JSON string ARRAY of Qobuz catalog track ids (a
        /// section-level "Add all to playlist"). Unparseable entries are
        /// dropped by `playlist_picker_qt::open_for_ids`.
        #[qinvokable]
        fn open_for_tracks(self: Pin<&mut QbzPlaylistPicker>, ids_json: QString);

        /// Same, for a WHOLE catalog album by id — the album-card ⋯ menu
        /// entry. The album's streamable tracks are fetched off-thread and
        /// become the carried payload (playlist_picker_qt::open_for_album).
        #[qinvokable]
        fn open_for_album(self: Pin<&mut QbzPlaylistPicker>, album_id: QString);

        /// Open the picker in LOCAL MODE for ONE row of the currently open
        /// LOCAL playlist detail. The ref is built by
        /// `local_playlist_qt::local_picker_ref_for_row` — never by QML, which
        /// only knows the row's display id and cannot recover a Plex rating
        /// key from it. A Qobuz row of a local playlist returns `None` here and
        /// must ride `open_for_track` instead.
        #[qinvokable]
        fn open_for_local_row(self: Pin<&mut QbzPlaylistPicker>, row_id: QString);

        /// Row click — add the carried tracks to that playlist. Runs duplicate
        /// detection first; if the target already holds any of them the
        /// confirm sub-modal opens instead of adding.
        #[qinvokable]
        fn pick(self: Pin<&mut QbzPlaylistPicker>, playlist_id: QString);

        /// Name substring filter — Rust re-filters its row cache and
        /// republishes (the `QbzMyQbzAdd.search` convention).
        #[qinvokable]
        fn search(self: Pin<&mut QbzPlaylistPicker>, query: QString);

        /// Reveal / hide the inline "Create new playlist" name field.
        #[qinvokable]
        fn show_create(self: Pin<&mut QbzPlaylistPicker>, open: bool);

        /// Create the playlist, add the carried tracks, close, toast.
        #[qinvokable]
        fn create_and_add(self: Pin<&mut QbzPlaylistPicker>, name: QString);

        /// Duplicate-confirm sub-modal: add every carried track, or only the
        /// ones the target does not already hold.
        #[qinvokable]
        fn confirm_duplicates(self: Pin<&mut QbzPlaylistPicker>, add_all: bool);

        /// Dismiss the duplicate-confirm sub-modal without adding anything.
        #[qinvokable]
        fn cancel_duplicates(self: Pin<&mut QbzPlaylistPicker>);

        /// Close and clear the carried tracks.
        #[qinvokable]
        fn close(self: Pin<&mut QbzPlaylistPicker>);
    }

    impl cxx_qt::Threading for QbzPlaylistPicker {}
}

use qbz_playlist_picker_bridge::QbzPlaylistPicker;

/// Rust side of the picker bridge (plain storage, phase-1 pattern).
pub struct QbzPlaylistPickerRust {
    picker_json: QString,
}

impl Default for QbzPlaylistPickerRust {
    fn default() -> Self {
        Self {
            // Parseable default so QML's JSON.parse never throws on frame 1.
            picker_json: QString::from("{}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzPlaylistPicker>> = OnceLock::new();

/// Queue a picker mutation onto the Qt event loop (no-op before boot registers
/// the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzPlaylistPicker>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_playlist_picker_bridge::QbzPlaylistPicker {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] playlist picker Qt thread already registered");
        }
        // Closed until a host opens it, and the closed document is already
        // seeded in Default — nothing to publish here.
    }

    pub fn open_for_track(self: Pin<&mut Self>, track_id: QString) {
        let id = track_id.to_string();
        if id.is_empty() {
            return;
        }
        crate::playlist_picker_qt::open_for_ids(&crate::app(), vec![id]);
    }

    pub fn open_for_tracks(self: Pin<&mut Self>, ids_json: QString) {
        let ids: Vec<String> = serde_json::from_str(&ids_json.to_string()).unwrap_or_default();
        if ids.is_empty() {
            log::debug!("[qbz-qt] playlist picker: openForTracks with an empty id list, ignored");
            return;
        }
        crate::playlist_picker_qt::open_for_ids(&crate::app(), ids);
    }

    pub fn open_for_album(self: Pin<&mut Self>, album_id: QString) {
        crate::playlist_picker_qt::open_for_album(album_id.to_string());
    }

    pub fn open_for_local_row(self: Pin<&mut Self>, row_id: QString) {
        let id = row_id.to_string();
        // `local_picker_ref_for_row` reads the OPEN local detail's queue
        // snapshot; `None` = no local detail open, an unresolvable row, or a
        // Qobuz row (which belongs on `open_for_track`). Refusing beats
        // guessing: the numeric display id of a Plex row parses as a catalog
        // id perfectly happily and would store a different track.
        match crate::local_playlist_qt::local_picker_ref_for_row(&id) {
            Some(reference) => {
                crate::playlist_picker_qt::open_for_local_refs(&crate::app(), vec![reference])
            }
            None => log::warn!(
                "[qbz-qt] playlist picker: no local-mode ref for detail row {id} — \
                 not a local/Plex row of an open local playlist, refusing to add"
            ),
        }
    }

    pub fn pick(self: Pin<&mut Self>, playlist_id: QString) {
        crate::playlist_picker_qt::pick(&playlist_id.to_string());
    }

    pub fn search(self: Pin<&mut Self>, query: QString) {
        crate::playlist_picker_qt::search(&query.to_string());
    }

    pub fn show_create(self: Pin<&mut Self>, open: bool) {
        crate::playlist_picker_qt::show_create(open);
    }

    pub fn create_and_add(self: Pin<&mut Self>, name: QString) {
        crate::playlist_picker_qt::create_and_add(&name.to_string());
    }

    pub fn confirm_duplicates(self: Pin<&mut Self>, add_all: bool) {
        crate::playlist_picker_qt::confirm_duplicates(add_all);
    }

    pub fn cancel_duplicates(self: Pin<&mut Self>) {
        crate::playlist_picker_qt::cancel_duplicates();
    }

    pub fn close(self: Pin<&mut Self>) {
        crate::playlist_picker_qt::close();
    }
}
