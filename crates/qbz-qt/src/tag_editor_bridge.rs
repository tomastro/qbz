//! QbzTagEditor — app-wide Qt metadata-editor singleton.
//!
//! The editor is mounted once in AppShell because saving republishes the album
//! view underneath it. Draft fields stay QML-local; Rust publishes only the
//! immutable seed, operation state and remote lookup documents.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_tag_editor_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, editor_open)]
        #[qproperty(bool, track_editor_open)]
        #[qproperty(i32, track_editor_initial_index)]
        #[qproperty(bool, editor_loading)]
        #[qproperty(bool, editor_saving)]
        #[qproperty(i32, editor_progress_current)]
        #[qproperty(i32, editor_progress_total)]
        /// Immutable album/version seed. Parseable while closed.
        #[qproperty(QString, editor_json)]
        /// Search results or one fully loaded metadata record.
        #[qproperty(QString, remote_json)]
        #[qproperty(bool, remote_searching)]
        #[qproperty(bool, remote_loading)]
        #[qproperty(bool, artwork_searching)]
        #[qproperty(bool, artwork_loading)]
        /// Bumps for every remote publish, including two byte-identical ones.
        #[qproperty(i32, remote_seq)]
        type QbzTagEditor = super::QbzTagEditorRust;

        #[qinvokable]
        fn boot(self: Pin<&mut QbzTagEditor>);

        #[qinvokable]
        fn close(self: Pin<&mut QbzTagEditor>);

        /// Drop the editor session because navigation already left its view.
        /// Unlike `close`, this must not mutate history a second time.
        #[qinvokable]
        fn leave(self: Pin<&mut QbzTagEditor>);

        /// Promote the current track-modal snapshot into the existing full
        /// album editor without resolving the album a second time.
        #[qinvokable]
        fn promote_to_album_editor(self: Pin<&mut QbzTagEditor>);

        /// The entire validated draft, encoded as JSON. File paths never come
        /// back from QML; Rust resolves row ids against the open session.
        #[qinvokable]
        fn save(self: Pin<&mut QbzTagEditor>, draft_json: QString);

        #[qinvokable]
        fn search_remote(
            self: Pin<&mut QbzTagEditor>,
            provider: QString,
            title: QString,
            artist: QString,
        );

        #[qinvokable]
        fn load_remote(self: Pin<&mut QbzTagEditor>, provider: QString, provider_id: QString);

        #[qinvokable]
        fn open_remote(self: Pin<&mut QbzTagEditor>, provider: QString, provider_id: QString);

        #[qinvokable]
        fn choose_artwork(self: Pin<&mut QbzTagEditor>);

        #[qinvokable]
        fn search_artwork(
            self: Pin<&mut QbzTagEditor>,
            provider: QString,
            title: QString,
            artist: QString,
            catalog_number: QString,
        );

        #[qinvokable]
        fn select_artwork(self: Pin<&mut QbzTagEditor>, candidate_id: QString);

        #[qinvokable]
        fn clear_artwork(self: Pin<&mut QbzTagEditor>);
    }

    impl cxx_qt::Threading for QbzTagEditor {}
}

use qbz_tag_editor_bridge::QbzTagEditor;

pub struct QbzTagEditorRust {
    editor_open: bool,
    track_editor_open: bool,
    track_editor_initial_index: i32,
    editor_loading: bool,
    editor_saving: bool,
    editor_progress_current: i32,
    editor_progress_total: i32,
    editor_json: QString,
    remote_json: QString,
    remote_searching: bool,
    remote_loading: bool,
    artwork_searching: bool,
    artwork_loading: bool,
    remote_seq: i32,
}

impl Default for QbzTagEditorRust {
    fn default() -> Self {
        Self {
            editor_open: false,
            track_editor_open: false,
            track_editor_initial_index: -1,
            editor_loading: false,
            editor_saving: false,
            editor_progress_current: 0,
            editor_progress_total: 0,
            editor_json: QString::from("{}"),
            remote_json: QString::from("{}"),
            remote_searching: false,
            remote_loading: false,
            artwork_searching: false,
            artwork_loading: false,
            remote_seq: 0,
        }
    }
}

static QT_THREAD: OnceLock<CxxQtThread<QbzTagEditor>> = OnceLock::new();

pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzTagEditor>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_tag_editor_bridge::QbzTagEditor {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] tag editor Qt thread already registered");
            return;
        }
        log::info!("[qbz-qt] tag editor bridge booted");
    }

    pub fn close(self: Pin<&mut Self>) {
        crate::tag_editor_qt::close();
    }

    pub fn leave(self: Pin<&mut Self>) {
        crate::tag_editor_qt::leave();
    }

    pub fn promote_to_album_editor(self: Pin<&mut Self>) {
        crate::tag_editor_qt::promote_to_album_editor();
    }

    pub fn save(self: Pin<&mut Self>, draft_json: QString) {
        crate::tag_editor_qt::save(&draft_json.to_string());
    }

    pub fn search_remote(self: Pin<&mut Self>, provider: QString, title: QString, artist: QString) {
        crate::tag_editor_qt::search_remote(
            &provider.to_string(),
            &title.to_string(),
            &artist.to_string(),
        );
    }

    pub fn load_remote(self: Pin<&mut Self>, provider: QString, provider_id: QString) {
        crate::tag_editor_qt::load_remote(&provider.to_string(), &provider_id.to_string());
    }

    pub fn open_remote(self: Pin<&mut Self>, provider: QString, provider_id: QString) {
        crate::tag_editor_qt::open_remote(&provider.to_string(), &provider_id.to_string());
    }

    pub fn choose_artwork(self: Pin<&mut Self>) {
        crate::tag_editor_qt::choose_artwork();
    }

    pub fn search_artwork(
        self: Pin<&mut Self>,
        provider: QString,
        title: QString,
        artist: QString,
        catalog_number: QString,
    ) {
        crate::tag_editor_qt::search_artwork(
            &provider.to_string(),
            &title.to_string(),
            &artist.to_string(),
            &catalog_number.to_string(),
        );
    }

    pub fn select_artwork(self: Pin<&mut Self>, candidate_id: QString) {
        crate::tag_editor_qt::select_artwork(&candidate_id.to_string());
    }

    pub fn clear_artwork(self: Pin<&mut Self>) {
        crate::tag_editor_qt::clear_artwork();
    }
}
