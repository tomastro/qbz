//! QbzTrackReplace — the "Find available version" modal's QML singleton
//! (2026-08-17 unavailable-tracks contract §6).
//!
//! Its own singleton rather than more surface on `QbzBridge` or on the playlist
//! document, for the reason the neighbouring modal bridges give: the modal is a
//! window-level overlay mounted once in `AppShell`, opened from a row context
//! menu, and it must outlive the view that opened it — an apply reloads the
//! playlist underneath it, which republishes `playlist_json` while this modal is
//! still up. Sharing a document would make every candidate keystroke republish
//! the whole playlist view, and every playlist republish reset the modal.
//!
//! Props: ONE JSON document (`track_replace_qt.rs` `ReplaceDoc` — the open /
//! loading / applying flags, the dead row's title+artist, the ranked candidates
//! and the selection). No `#[qsignal]`: outcomes are toasts (toast_qt.rs) and
//! state changes are property republishes, the `QbzMyQbzAdd` shape.
//!
//! The open payload is a JSON OBJECT rather than eight invokable arguments, the
//! `QbzMyQbzAdd.open(JSON.stringify(...))` idiom: the host already holds the row
//! as a parsed JSON object Rust produced, so `JSON.stringify(...)` is one call
//! with no Rust-side lookup table.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_track_replace_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // ONE JSON document (track_replace_qt.rs ReplaceDoc).
        #[qproperty(QString, replace_json)]
        type QbzTrackReplace = super::QbzTrackReplaceRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzTrackReplace>);

        /// Open the modal for ONE dead row. The payload is the host's
        /// `DeadRow` object (`{playlistId, playlistTrackId, trackId, title,
        /// artist, album?, isrc?, durationSecs?}`); a payload without a
        /// playlist id or a track id is refused with a `log::warn!` and
        /// nothing else — an impossible combination must not be reachable, so
        /// the user never sees a block.
        #[qinvokable]
        fn open(self: Pin<&mut QbzTrackReplace>, payload_json: QString);

        /// Library > All track/album tombstone: open the same ranked picker in
        /// album-search mode. The footer then offers non-mutating navigation
        /// and an explicit add-first favourite replacement.
        #[qinvokable]
        fn open_release(self: Pin<&mut QbzTrackReplace>, payload_json: QString);

        /// Re-run the search with an edited query (the reference's one good
        /// idea — the "title artist" guess is often not the right words).
        #[qinvokable]
        fn search(self: Pin<&mut QbzTrackReplace>, query: QString);

        /// Pick a candidate row (the confirm button acts on this id).
        #[qinvokable]
        fn select(self: Pin<&mut QbzTrackReplace>, track_id: QString);

        /// Non-mutating third action: open the selected candidate's album in
        /// either playlist-track or Library-release mode.
        #[qinvokable]
        fn open_selected_album(self: Pin<&mut QbzTrackReplace>);

        /// Perform the swap: add -> reposition -> remove. See
        /// `track_replace_qt.rs`'s header for why that order and why neither
        /// failure path rolls anything back.
        #[qinvokable]
        fn apply(self: Pin<&mut QbzTrackReplace>);

        /// Close and drop the pending row.
        #[qinvokable]
        fn close(self: Pin<&mut QbzTrackReplace>);
    }

    impl cxx_qt::Threading for QbzTrackReplace {}
}

use qbz_track_replace_bridge::QbzTrackReplace;

/// Rust side of the replacement bridge (plain storage, phase-1 pattern).
pub struct QbzTrackReplaceRust {
    replace_json: QString,
}

impl Default for QbzTrackReplaceRust {
    fn default() -> Self {
        Self {
            // Parseable default so QML's JSON.parse never throws on frame 1
            // (home_bridge.rs:250-257).
            replace_json: QString::from("{}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzTrackReplace>> = OnceLock::new();

/// Queue a replacement-modal mutation onto the Qt event loop (no-op before boot
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzTrackReplace>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_track_replace_bridge::QbzTrackReplace {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] track replace Qt thread already registered");
        }
        // The modal is closed until a host opens it, and the closed document is
        // already seeded in Default — nothing to publish here.
    }

    pub fn open(self: Pin<&mut Self>, payload_json: QString) {
        crate::track_replace_qt::open(&payload_json.to_string());
    }

    pub fn open_release(self: Pin<&mut Self>, payload_json: QString) {
        crate::track_replace_qt::open_release(&payload_json.to_string());
    }

    pub fn search(self: Pin<&mut Self>, query: QString) {
        crate::track_replace_qt::search(&query.to_string());
    }

    pub fn select(self: Pin<&mut Self>, track_id: QString) {
        crate::track_replace_qt::select(&track_id.to_string());
    }

    pub fn open_selected_album(self: Pin<&mut Self>) {
        crate::track_replace_qt::open_selected_album();
    }

    pub fn apply(self: Pin<&mut Self>) {
        crate::track_replace_qt::apply();
    }

    pub fn close(self: Pin<&mut Self>) {
        crate::track_replace_qt::close();
    }
}
