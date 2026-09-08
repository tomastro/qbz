//! QbzScene — ArtistScene ("artists from the same place") domain bridge.
//!
//! A NEW bridge rather than three more members on `QbzArtist` (TRACK-RULE 2:
//! one bridge per domain, never a God-object — `artist_bridge.rs` already
//! serves the artist page and its discography sub-view). Contract §13.1 freezes
//! everything below; a QML lane is written against it, and
//! `qml_singleton_xref.py` is the gate that proves the two halves agree.
//!
//! The module is `qbz_scene_bridge`, not `qbz_scene`: bridge modules are never
//! named after a workspace crate, and the QML type name comes from the QObject
//! (`QbzScene`), not from the module.
//!
//! Props: ONE JSON document (`artist_scene_qt::SceneDoc` — state, hero,
//! progress, the grid, and the sidepanel's own sub-document) plus its revision
//! counter.
//! Invokables: open / loadMore / retry / openArtist / selectArtist / close.
//!
//! **A "new bridge" is seven artefacts, not one file** (contract §5/B5), and
//! this is one of them. The other six are integration-owned and are listed in
//! the handoff report: `build.rs` `rust_files` registration, `main.rs` `mod`
//! line, the QML singleton registration, `Main.qml`'s `boot()` entry, and
//! `ContentRouter.qml`'s `scene` arm. Missing the router arm is a BLANK content
//! pane logged nowhere (`nav_qt.rs:9-23`).

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_scene_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // ONE JSON document (contract §13.1). Everything the view paints —
        // the five body states, the hero, the progress stream, the grid rows
        // and the sidepanel sub-document — travels inside it, so a frame is
        // never half-old.
        #[qproperty(QString, scene_json)]
        // Bumped on EVERY publish. QML binds it to force a re-read: a QString
        // identical to the previous one emits no change signal, and the
        // progress stream produces exactly that case (same phase, same pct,
        // same detail) more often than any other view in the tree.
        #[qproperty(i32, scene_rev)]
        type QbzScene = super::QbzSceneRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzScene>);

        /// The two doors (contract §2.1): the Network sidebar's Origin
        /// location row, and the header ⋯ "Artist Scene" item.
        ///
        /// Every query parameter travels WITH the call. The Slint port instead
        /// reads a process-global `static LOCATION_PARAMS` that the next
        /// artist page overwrites, so its "Load more" can append a different
        /// artist's scene into the open grid (`crates/qbz/src/artist.rs:1518`).
        ///
        /// `genres_csv` / `tags_csv` are comma-separated because the QML seam
        /// has no list type; empty segments are dropped.
        ///
        /// `artist_name` is the MUSICBRAINZ name, not the Qobuz one — it is
        /// what the hero subtitle "Based on {artist}" prints
        /// (`ArtistDetailView.svelte:2915`).
        ///
        /// `#[auto_cxx_name]` makes the QML name `QbzScene.open(...)`.
        // NOTE: no `#[allow(clippy::too_many_arguments)]` here — the
        // `#[cxx_qt::bridge]` macro rewrites this block and only understands
        // its own attributes plus doc comments. The allow lives on the `impl`
        // below, which is where the real function body is.
        #[qinvokable]
        fn open(
            self: Pin<&mut QbzScene>,
            mbid: QString,
            artist_name: QString,
            area_id: QString,
            city: QString,
            country: QString,
            country_code: QString,
            precision: QString,
            genres_csv: QString,
            tags_csv: QString,
        );

        /// The footer's explicit "Load More" — Tauri paginates on a button,
        /// not on scroll (`ArtistsByLocationView.svelte:390-398`). Every guard
        /// lives in Rust as well, so a double click cannot queue two pages.
        ///
        /// QML name: `QbzScene.loadMore()`.
        #[qinvokable]
        fn load_more(self: Pin<&mut QbzScene>);

        /// The error state's Retry. Re-arms the progress stream AND the
        /// cancellation token (contract DV-5 / B8) and restarts at offset 0,
        /// recording no nav entry.
        #[qinvokable]
        fn retry(self: Pin<&mut QbzScene>);

        /// Grid card / sidepanel row click -> the artist page.
        ///
        /// QML name: `QbzScene.openArtist(id)`.
        #[qinvokable]
        fn open_artist(self: Pin<&mut QbzScene>, qobuz_id: QString);

        /// Sidepanel mode: load that artist's albums into `panel` (limit 500,
        /// offset 0 — Tauri's numbers, `ArtistsByLocationView.svelte:345-349`),
        /// categorised into Discography / EPs & Singles / Live.
        ///
        /// QML name: `QbzScene.selectArtist(id)`.
        #[qinvokable]
        fn select_artist(self: Pin<&mut QbzScene>, qobuz_id: QString);

        /// View deactivation. **Not** cosmetic: one page runs up to 100
        /// SEQUENTIAL Qobuz artist searches plus rate-limited MusicBrainz
        /// calls, and a generation guard only drops the final publish — it
        /// stops none of that traffic (contract §3/B8). The view MUST call
        /// this when it is torn down or navigated away from.
        #[qinvokable]
        fn close(self: Pin<&mut QbzScene>);
    }

    impl cxx_qt::Threading for QbzScene {}
}

use qbz_scene_bridge::QbzScene;

/// Rust side of the scene bridge (plain storage, phase-1 pattern).
pub struct QbzSceneRust {
    scene_json: QString,
    scene_rev: i32,
}

impl Default for QbzSceneRust {
    fn default() -> Self {
        Self {
            // "{}" and not "": the view `JSON.parse`s this straight, and an
            // empty string throws into the catch arm on the very first frame.
            scene_json: QString::from("{}"),
            scene_rev: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzScene>> = OnceLock::new();

/// Queue a scene-bridge mutation onto the Qt event loop (no-op before boot
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzScene>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_scene_bridge::QbzScene {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] scene Qt thread already registered");
        }
    }

    // One-line delegates into `artist_scene_qt`, with no `crate::` forwarder in
    // main.rs: that indirection exists for the paths that mutate main.rs's own
    // state (LAST_DETAIL, the nav latch). This view owns its state in its own
    // module and records its own nav entry, so there is nothing for main.rs to
    // coordinate. The one exception is `open_artist`, which goes through
    // `crate::open_artist` precisely because that path DOES set LAST_DETAIL.

    #[allow(clippy::too_many_arguments)]
    pub fn open(
        self: Pin<&mut Self>,
        mbid: QString,
        artist_name: QString,
        area_id: QString,
        city: QString,
        country: QString,
        country_code: QString,
        precision: QString,
        genres_csv: QString,
        tags_csv: QString,
    ) {
        crate::artist_scene_qt::open(
            mbid.to_string(),
            artist_name.to_string(),
            area_id.to_string(),
            city.to_string(),
            country.to_string(),
            country_code.to_string(),
            precision.to_string(),
            genres_csv.to_string(),
            tags_csv.to_string(),
        );
    }

    pub fn load_more(self: Pin<&mut Self>) {
        crate::artist_scene_qt::load_more();
    }

    pub fn retry(self: Pin<&mut Self>) {
        crate::artist_scene_qt::retry();
    }

    pub fn open_artist(self: Pin<&mut Self>, qobuz_id: QString) {
        crate::artist_scene_qt::open_artist(qobuz_id.to_string());
    }

    pub fn select_artist(self: Pin<&mut Self>, qobuz_id: QString) {
        crate::artist_scene_qt::select_artist(qobuz_id.to_string());
    }

    pub fn close(self: Pin<&mut Self>) {
        crate::artist_scene_qt::close();
    }
}
