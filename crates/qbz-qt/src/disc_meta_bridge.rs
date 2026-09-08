//! QbzDiscMeta — the "correct this disc" modal's QML singleton.
//!
//! One document (`metaJson`) and six invokables; every outcome is a republish
//! or a toast, so there is no `#[qsignal]` here (the `playlist_picker_bridge`
//! convention).
//!
//! A SEPARATE singleton rather than more properties on `QbzLocal`, for the
//! reason `QbzFolderEdit` is separate from `QbzPlaylistManager`: this surface
//! is about the MEDIUM in the drive, not about the Local Library document, and
//! folding it in would make every disc lookup republish a 2378-row browse
//! document's neighbour.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_disc_meta_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        /// The whole modal in one document. Parseable while closed so a
        /// binding reading `doc.open` on frame 1 cannot throw.
        #[qproperty(QString, meta_json)]
        type QbzDiscMeta = super::QbzDiscMetaRust;

        /// Registers the Qt-thread hop. Without it every publish from Rust is
        /// dropped silently and the modal never receives a document.
        #[qinvokable]
        fn boot(self: Pin<&mut QbzDiscMeta>);

        /// Open, seeded from the disc that is on screen.
        #[qinvokable]
        fn open(self: Pin<&mut QbzDiscMeta>);

        #[qinvokable]
        fn close(self: Pin<&mut QbzDiscMeta>);

        /// "musicbrainz" | "discogs". Clears the results with it.
        #[qinvokable]
        fn set_provider(self: Pin<&mut QbzDiscMeta>, provider: QString);

        /// Run a search. The text is QML-local until it is submitted, the
        /// `TrackReplacementModal` convention.
        #[qinvokable]
        fn search(self: Pin<&mut QbzDiscMeta>, query: QString);

        /// Fetch one candidate in full and preview its track list. Selecting
        /// is not applying.
        #[qinvokable]
        fn select(self: Pin<&mut QbzDiscMeta>, provider_id: QString);

        /// Write the previewed release onto the session and into the store.
        #[qinvokable]
        fn apply(self: Pin<&mut QbzDiscMeta>);

        /// Drop this disc's correction.
        #[qinvokable]
        fn forget(self: Pin<&mut QbzDiscMeta>);
    }

    impl cxx_qt::Threading for QbzDiscMeta {}
}

use qbz_disc_meta_bridge::QbzDiscMeta;

pub struct QbzDiscMetaRust {
    meta_json: QString,
}

impl Default for QbzDiscMetaRust {
    fn default() -> Self {
        Self {
            meta_json: QString::from("{\"open\":false}"),
        }
    }
}

static QT_THREAD: OnceLock<CxxQtThread<QbzDiscMeta>> = OnceLock::new();

pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzDiscMeta>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_disc_meta_bridge::QbzDiscMeta {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] disc meta Qt thread already registered");
            return;
        }
        // One line, once, so "never booted" and "booted and nothing happened"
        // can be told apart from the log — the distinction `media_controls_qt`
        // documents as the reason its own first push is logged.
        log::info!("[qbz-qt] disc meta bridge booted");
    }

    pub fn open(self: Pin<&mut Self>) {
        crate::disc_meta_qt::open();
    }

    pub fn close(self: Pin<&mut Self>) {
        crate::disc_meta_qt::close();
    }

    pub fn set_provider(self: Pin<&mut Self>, provider: QString) {
        crate::disc_meta_qt::set_provider(&provider.to_string());
    }

    pub fn search(self: Pin<&mut Self>, query: QString) {
        crate::disc_meta_qt::search(&query.to_string());
    }

    pub fn select(self: Pin<&mut Self>, provider_id: QString) {
        crate::disc_meta_qt::select(&provider_id.to_string());
    }

    pub fn apply(self: Pin<&mut Self>) {
        crate::disc_meta_qt::apply();
    }

    pub fn forget(self: Pin<&mut Self>) {
        crate::disc_meta_qt::forget();
    }
}
