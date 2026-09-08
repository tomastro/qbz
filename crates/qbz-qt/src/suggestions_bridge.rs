//! QbzSuggestions — immersive Suggestions domain bridge (viz_bridge.rs
//! exemplar: OnceLock<CxxQtThread>, `ui()` no-op pre-boot, `boot()`
//! invokable, `impl Default` for construction-seeded values).
//!
//! Port of the Slint `SuggestionsState` + `SuggestionsActions` globals per
//! the immersive-port contract
//! (`qbz-nix-docs/qt-frontend/2026-08-02-immersive-port/00-CONTRACT.md` §4.5,
//! block B4). ONE JSON document with EXACTLY the Slint field set
//! (`state.slint:872-908`, translated to the §4.5 JSON shape):
//! `{loading, error, cards:[{kind,title,subtitle,coverUrls,playlistId,
//! seedTrackId,seedTrackName,seedArtistId,badge,loading}], tracks:[{id,
//! title,artist,artistId,duration,artUrl,explicit}]}` — full-shape default,
//! NEVER "{}" (trap 15). The Slint `error: string` ("" | "error") becomes a
//! bool per the contract; the Slint top-level artist-id/seed-track-id dedup
//! fields stay Rust-side (the panel derives loading/error/empty from this
//! shape alone, §4.5).
//!
//! Covers ride the shared artwork pipeline like `coverflowJson` (§4.4): the
//! document carries remote URLs, QML resolves them through
//! `QbzShell.sidebarArtworkWindow` + `QbzLibrary.libraryArtworkReady` — Qt
//! has no per-slot decoded-image slots (Slint's cover0..cover3).

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

/// Full-shape default (trap 15): QML reads `.cards.length` in the
/// pre-publish frame.
const SUGGESTIONS_EMPTY: &str = r#"{"loading":false,"error":false,"cards":[],"tracks":[]}"#;

#[cxx_qt::bridge]
pub mod qbz_suggestions {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // The §4.5 document (see the file header). Written ONLY by
        // suggestions_qt through `ui()`.
        #[qproperty(QString, suggestions_json)]
        type QbzSuggestions = super::QbzSuggestionsRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY
        /// domain singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzSuggestions>);

        /// Entry + now-playing-change refresh (state.slint:919-923). The
        /// Rust side reads the artist-id + title off the now-playing model
        /// (the Qt NowPlayingState, main.rs:16699-16702 parity). A "" id
        /// resets to the empty (no-track) state.
        #[qinvokable]
        fn load(self: Pin<&mut QbzSuggestions>, track_id: QString);
        /// Play / queue / play-next a curated artist playlist by id
        /// (state.slint:912-914).
        #[qinvokable]
        fn play_playlist(self: Pin<&mut QbzSuggestions>, playlist_id: QString);
        #[qinvokable]
        fn queue_playlist(self: Pin<&mut QbzSuggestions>, playlist_id: QString);
        #[qinvokable]
        fn play_next_playlist(self: Pin<&mut QbzSuggestions>, playlist_id: QString);
        /// Start the seed-track Song Radio: (seed-track-id, seed-track-name,
        /// seed-artist-id) — state.slint:916.
        #[qinvokable]
        fn start_radio(
            self: Pin<&mut QbzSuggestions>,
            seed_track_id: QString,
            seed_track_name: QString,
            seed_artist_id: QString,
        );
        /// Play a single recommended track by id (state.slint:918).
        #[qinvokable]
        fn play_track(self: Pin<&mut QbzSuggestions>, track_id: QString);
    }

    impl cxx_qt::Threading for QbzSuggestions {}
}

use qbz_suggestions::QbzSuggestions;

/// Rust side of the suggestions bridge (plain storage, phase-1 pattern).
pub struct QbzSuggestionsRust {
    suggestions_json: QString,
}

impl Default for QbzSuggestionsRust {
    fn default() -> Self {
        Self {
            suggestions_json: QString::from(SUGGESTIONS_EMPTY),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (viz_bridge.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzSuggestions>> = OnceLock::new();

/// Queue a suggestions-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzSuggestions>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_suggestions::QbzSuggestions {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] suggestions Qt thread already registered");
        }
    }

    pub fn load(self: Pin<&mut Self>, track_id: QString) {
        crate::suggestions_qt::load(track_id.to_string());
    }

    pub fn play_playlist(self: Pin<&mut Self>, playlist_id: QString) {
        crate::suggestions_qt::play_playlist(playlist_id.to_string());
    }

    pub fn queue_playlist(self: Pin<&mut Self>, playlist_id: QString) {
        crate::suggestions_qt::queue_playlist(playlist_id.to_string());
    }

    pub fn play_next_playlist(self: Pin<&mut Self>, playlist_id: QString) {
        crate::suggestions_qt::play_next_playlist(playlist_id.to_string());
    }

    pub fn start_radio(
        self: Pin<&mut Self>,
        seed_track_id: QString,
        seed_track_name: QString,
        seed_artist_id: QString,
    ) {
        crate::suggestions_qt::start_radio(
            seed_track_id.to_string(),
            seed_track_name.to_string(),
            seed_artist_id.to_string(),
        );
    }

    pub fn play_track(self: Pin<&mut Self>, track_id: QString) {
        crate::suggestions_qt::play_track(track_id.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_doc_is_full_shape() {
        // trap 15: the construction-seeded default parses and carries all
        // four top-level fields — QML derives loading/error/empty from it
        // in the pre-publish frame.
        let doc: serde_json::Value = serde_json::from_str(SUGGESTIONS_EMPTY).unwrap();
        assert_eq!(doc["loading"], false);
        assert_eq!(doc["error"], false);
        assert!(doc["cards"].is_array());
        assert!(doc["tracks"].is_array());
    }
}
