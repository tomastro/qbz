//! QbzLibrary — Library view domain bridge.
//!
//! The module is `qbz_library_bridge`, NOT `qbz_library`: this crate depends
//! on the `qbz-library` crate, whose Rust name is `qbz_library`, and a bridge
//! module by that name is ambiguous at every use site. The QML type name comes
//! from the QObject (`QbzLibrary`), not from the module, so QML is unaffected.
//! Original note: Library view domain bridge (phase 23 split of the QbzBridge
//! God-object; the pattern is documented in main.rs). Props: the merged-feed
//! document + counts + loading/error. Invokables: reload, windowed artwork
//! dispatch, favorite/pin toggles. Signals: artwork-ready / favorite-changed
//! / pin-changed (emitted through the ui() hop like before).

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_library_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // --- Library view --------------------------------------------------
        #[qproperty(bool, library_loading)]
        #[qproperty(QString, library_error)]
        // One JSON document: the FULL merged feed (tabs/search/sort/source
        // filters derive QML-side from the parsed array — see library_qt.rs
        // for the rationale + the measured parse cost).
        #[qproperty(QString, library_json)]
        // {tracks, albums, artists, playlists, labels, all} — tab badges.
        #[qproperty(QString, library_counts_json)]
        // Persisted toolbar choices (library_prefs.rs — the SAME
        // favorites_ui.json the shipping Slint build writes). Seeded in
        // `boot`, written back one key at a time through `set_library_pref`.
        #[qproperty(QString, library_prefs_json)]
        // Process/session-only All-tab filters. They intentionally do not
        // enter favorites_ui.json, but the singleton outlives LibraryView so
        // navigation cannot reset them with the component.
        #[qproperty(bool, session_show_purchases)]
        #[qproperty(bool, session_show_favorites)]
        #[qproperty(bool, session_show_following)]
        // Artists SIDEPANEL: the selected artist's release sections
        // (library_sidepanel.rs). Its own document, never folded into
        // `library_json`: a selection must not re-serialize a 10k-row feed.
        #[qproperty(QString, sidepanel_json)]
        type QbzLibrary = super::QbzLibraryRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY
        /// domain singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzLibrary>);

        /// Library retry button / manual refresh.
        #[qinvokable]
        fn reload_library(self: Pin<&mut QbzLibrary>);

        /// Windowed artwork: the grid/list reports the mounted window as a
        /// JSON array of artKeys (tab views filter the feed, so raw indices
        /// don't map); Rust dispatches covers for those keys (id-keyed via
        /// `libraryArtworkReady`).
        #[qinvokable]
        fn library_artwork_window(self: Pin<&mut QbzLibrary>, keys_json: QString);

        #[qinvokable]
        fn kiosk_artwork_window(self: Pin<&mut QbzLibrary>, keys_json: QString, pixels: i32);

        /// Card heart: toggle favorite (Qobuz API or the local store,
        /// routed by id shape); the result arrives via
        /// `libraryFavoriteChanged`.
        #[qinvokable]
        fn library_toggle_favorite(self: Pin<&mut QbzLibrary>, kind: QString, id: QString);

        /// Confirmed Library action: remove the album heart and every track
        /// heart belonging to one withdrawn Qobuz release. Downloads and
        /// purchase entitlements are untouched.
        #[qinvokable]
        fn library_remove_release_favorites(self: Pin<&mut QbzLibrary>, album_id: QString);

        /// LibraryView row play (PARITY-DEBT #5). `visible_ids_json` is the
        /// JSON array of the track ids the view is CURRENTLY rendering, in
        /// render order; `clicked_id` is the row that was hit. The Slint
        /// queues the whole VISIBLE list from the clicked row
        /// (playback.rs:3518-3524 `order_by_visible`) — this port used to play
        /// exactly one track and then stop dead.
        #[qinvokable]
        fn library_play_visible(
            self: Pin<&mut QbzLibrary>,
            visible_ids_json: QString,
            clicked_id: QString,
        );

        /// Emitted when a dispatched cover lands on disk (id-keyed —
        /// `{kind}:{id}`); QML updates its artwork map.
        #[qsignal]
        fn library_artwork_ready(self: Pin<&mut QbzLibrary>, key: QString, path: QString);

        /// Emitted after a heart toggle (optimistic flip + rollback).
        #[qsignal]
        fn library_favorite_changed(self: Pin<&mut QbzLibrary>, key: QString, value: bool);

        /// Library header: play (or shuffle) the whole VISIBLE track set.
        /// `shuffle` reorders this queue only — it never latches the player's
        /// shuffle mode (PARITY-DEBT #17).
        #[qinvokable]
        fn library_play_all(self: Pin<&mut QbzLibrary>, visible_ids_json: QString, shuffle: bool);

        /// Persist ONE toolbar choice. `key` is a camelCase name from
        /// `library_prefs::to_json` ("albumsView", "tracksGroup", …); `value`
        /// is always a string ("true"/"false" for the booleans).
        #[qinvokable]
        fn set_library_pref(self: Pin<&mut QbzLibrary>, key: QString, value: QString);

        /// Multi-select bulk bar. `scope` = "track" | "album"; `ids_json` is
        /// the JSON array the bar built from its own selection map (the QML
        /// owns the selection, so "select-all" / "clear" never arrive here).
        #[qinvokable]
        fn library_bulk_action(
            self: Pin<&mut QbzLibrary>,
            scope: QString,
            ids_json: QString,
            action: QString,
        );

        /// Artists sidepanel: load + show one artist's releases in the right
        /// pane. Result lands on `sidepanel_json`.
        #[qinvokable]
        fn sidepanel_select_artist(self: Pin<&mut QbzLibrary>, id: QString, name: QString);

        /// Artists sidepanel: drop the selection (leaving the mode).
        #[qinvokable]
        fn sidepanel_clear(self: Pin<&mut QbzLibrary>);

        /// AlbumCard pin badge: toggle pin (album/artist/playlist).
        #[qinvokable]
        fn toggle_pin(
            self: Pin<&mut QbzLibrary>,
            kind: QString,
            id: QString,
            title: QString,
            subtitle: QString,
            artwork_url: QString,
        );
        /// Warm in-memory lookup for recycled card delegates. Unlike the pin
        /// mutation, this performs no SQLite query and publishes nothing.
        #[qinvokable]
        fn pin_state(self: &QbzLibrary, kind: QString, id: QString) -> bool;
        /// Emitted after a pin toggle (`{kind}:{id}` key like artKey).
        #[qsignal]
        fn pin_changed(self: Pin<&mut QbzLibrary>, key: QString, value: bool);
    }

    impl cxx_qt::Threading for QbzLibrary {}
}

use qbz_library_bridge::QbzLibrary;

/// Rust side of the library bridge (plain storage, phase-1 pattern).
pub struct QbzLibraryRust {
    library_loading: bool,
    library_error: QString,
    library_json: QString,
    library_counts_json: QString,
    library_prefs_json: QString,
    session_show_purchases: bool,
    session_show_favorites: bool,
    session_show_following: bool,
    sidepanel_json: QString,
}

impl Default for QbzLibraryRust {
    fn default() -> Self {
        Self {
            library_loading: false,
            library_error: QString::default(),
            library_json: QString::from("[]"),
            library_counts_json: QString::from("{}"),
            // Parseable defaults so QML's JSON.parse never throws on frame 1
            // (the real values arrive in `boot`).
            library_prefs_json: QString::from("{}"),
            session_show_purchases: true,
            session_show_favorites: true,
            session_show_following: true,
            sidepanel_json: QString::from("{}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzLibrary>> = OnceLock::new();

/// Queue a library-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzLibrary>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_library_bridge::QbzLibrary {
    pub fn boot(mut self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] library Qt thread already registered");
        }
        // Seed the persisted toolbar choices so the FIRST paint of the Library
        // is the view mode / sort / group the user last picked, not the
        // defaults followed by a visible snap (`local_bridge::boot` does the
        // same for the Local Library). Cheap: one small json read.
        let prefs = crate::library_prefs::to_json();
        self.as_mut()
            .set_library_prefs_json(QString::from(prefs.as_str()));
    }

    pub fn set_library_pref(mut self: Pin<&mut Self>, key: QString, value: QString) {
        crate::library_prefs::set(&key.to_string(), &value.to_string());
        // Republish the document. The toolbar does not need it (each control
        // owns its own live state and only SEEDS from here), but the Settings
        // row for `allLocalScope` BINDS to it — without this its `currentIndex`
        // would keep answering with the value that was on disk at boot and the
        // select would snap back after every pick. Cheap: one small json read,
        // and it also makes the seed self-healing when the Slint build writes
        // the same file underneath us.
        let prefs = crate::library_prefs::to_json();
        self.as_mut()
            .set_library_prefs_json(QString::from(prefs.as_str()));
    }

    pub fn library_bulk_action(
        self: Pin<&mut Self>,
        scope: QString,
        ids_json: QString,
        action: QString,
    ) {
        crate::library_bulk::bulk_action(
            scope.to_string(),
            ids_json.to_string(),
            action.to_string(),
        );
    }

    pub fn sidepanel_select_artist(self: Pin<&mut Self>, id: QString, name: QString) {
        crate::library_sidepanel::select(id.to_string(), name.to_string());
    }

    pub fn sidepanel_clear(self: Pin<&mut Self>) {
        crate::library_sidepanel::clear();
    }

    pub fn reload_library(self: Pin<&mut Self>) {
        crate::reload_library();
    }

    pub fn library_artwork_window(self: Pin<&mut Self>, keys_json: QString) {
        crate::library_artwork_window(keys_json.to_string());
    }

    pub fn kiosk_artwork_window(self: Pin<&mut Self>, keys_json: QString, pixels: i32) {
        if crate::kiosk_profile_qt::active() {
            crate::library_artwork_window_at_px(keys_json.to_string(), Some(pixels));
        }
    }

    pub fn library_toggle_favorite(self: Pin<&mut Self>, kind: QString, id: QString) {
        crate::library_toggle_favorite(kind.to_string(), id.to_string());
    }

    pub fn library_remove_release_favorites(self: Pin<&mut Self>, album_id: QString) {
        crate::library_remove_release_favorites(album_id.to_string());
    }

    pub fn library_play_visible(
        self: Pin<&mut Self>,
        visible_ids_json: QString,
        clicked_id: QString,
    ) {
        crate::library_qt::play_from_visible(visible_ids_json.to_string(), clicked_id.to_string());
    }

    pub fn library_play_all(self: Pin<&mut Self>, visible_ids_json: QString, shuffle: bool) {
        crate::library_qt::play_visible_all(visible_ids_json.to_string(), shuffle);
    }

    pub fn toggle_pin(
        self: Pin<&mut Self>,
        kind: QString,
        id: QString,
        title: QString,
        subtitle: QString,
        artwork_url: QString,
    ) {
        crate::toggle_pin(
            kind.to_string(),
            id.to_string(),
            title.to_string(),
            subtitle.to_string(),
            artwork_url.to_string(),
        );
    }

    pub fn pin_state(&self, kind: QString, id: QString) -> bool {
        crate::sidebar_qt::is_pinned(&kind.to_string(), &id.to_string())
    }
}
