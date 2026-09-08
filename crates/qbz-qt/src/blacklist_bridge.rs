//! QbzBlacklist — Blacklist domain bridge (phase-23 per-domain pattern; the
//! pattern is documented in main.rs).
//!
//! The module is `qbz_blacklist_bridge`, not `qbz_blacklist`: bridge modules are
//! never named after a workspace crate (the `qbz_library` collision is the
//! documented precedent). The QML type name comes from the QObject
//! (`QbzBlacklist`), not from the module, so QML is unaffected.
//!
//! Props: the ONE manager document (all three axes) + its loading flag.
//! Invokables: the manager's own actions plus the three consumer-surface entry
//! points (`artistToggle`, `albumToggle`, `blockAlbum`, `dismissArtist`) — the
//! blacklist is mutated from album cards, album/artist headers and artist cards,
//! not only from the manager, and a domain owns its own mutations.
//! Signal: `blacklistChanged(key, value)` — the cross-surface settle channel,
//! same two-arg `"{kind}:{id}"` shape as `QbzLibrary.pinChanged` /
//! `libraryFavoriteChanged`.
//!
//! Every invokable body is a one-line forward into `crate::blacklist_qt`.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_blacklist_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // ONE JSON document (blacklist_qt.rs BlacklistDoc: activeTab + enabled
        // + searchQuery + the three axes with their FULL counts); QML
        // JSON.parse()s it once.
        #[qproperty(QString, blacklist_json)]
        // Defaults to TRUE, unlike every other *_loading in the tree: the
        // Loader mounts this view BEFORE Component.onCompleted fires reload(),
        // so a `false` default would flash the empty state ("No blacklisted
        // artists") before the spinner. Slint's BlacklistState.loading has the
        // same `: true` default for the same reason.
        #[qproperty(bool, blacklist_loading)]
        type QbzBlacklist = super::QbzBlacklistRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzBlacklist>);

        /// Settings > Blacklist > Manage: push the "blacklist" route and load.
        #[qinvokable]
        fn open_manager(self: Pin<&mut QbzBlacklist>);
        /// Re-read the store and re-publish. The view calls this from
        /// Component.onCompleted because nav back/forward runs no per-view load.
        #[qinvokable]
        fn reload(self: Pin<&mut QbzBlacklist>);
        /// 0 = Artists, 1 = Albums, 2 = Recommendations.
        #[qinvokable]
        fn set_tab(self: Pin<&mut QbzBlacklist>, tab: i32);
        /// Search-as-you-type; no debounce, matching the reference.
        #[qinvokable]
        fn search_changed(self: Pin<&mut QbzBlacklist>, query: QString);
        /// Flip the global enable flag (the counts are unaffected by it).
        #[qinvokable]
        fn toggle_enabled(self: Pin<&mut QbzBlacklist>);

        /// Artists tab row `x` — un-blacklist, optimistic, no confirm.
        #[qinvokable]
        fn remove_artist(self: Pin<&mut QbzBlacklist>, artist_id: QString);
        /// Artists tab Clear All (leaves the enabled flag + the album axis).
        #[qinvokable]
        fn clear_all_artists(self: Pin<&mut QbzBlacklist>);
        /// Artists tab row body click — open the artist page.
        #[qinvokable]
        fn open_artist(self: Pin<&mut QbzBlacklist>, artist_id: QString);

        /// Albums tab row `x` — unblock, optimistic, no confirm.
        #[qinvokable]
        fn remove_album(self: Pin<&mut QbzBlacklist>, album_id: QString);
        /// Albums tab Clear All (leaves the enabled flag + the artist axis).
        #[qinvokable]
        fn clear_all_albums(self: Pin<&mut QbzBlacklist>);
        /// Albums tab row body click — open the album page.
        #[qinvokable]
        fn open_album(self: Pin<&mut QbzBlacklist>, album_id: QString);

        /// Album card / list row ⋯ menu "Block this album" (one-way). The
        /// display fields are persisted with the row, so the manager can render
        /// it without a fetch — `cover_url` must be the REMOTE url, never a
        /// `file://` cache path.
        #[qinvokable]
        fn block_album(
            self: Pin<&mut QbzBlacklist>,
            album_id: QString,
            title: QString,
            artist: QString,
            cover_url: QString,
        );
        /// Album page header ⋯ menu Block <-> Unblock; settles on
        /// `blacklistChanged("album:" + id, blocked)`.
        #[qinvokable]
        fn album_toggle(
            self: Pin<&mut QbzBlacklist>,
            album_id: QString,
            title: QString,
            artist: QString,
            cover_url: QString,
        );
        /// Artist page ⋯ menu Blacklist <-> Show; settles on
        /// `blacklistChanged("artist:" + id, blacklisted)`.
        #[qinvokable]
        fn artist_toggle(self: Pin<&mut QbzBlacklist>, artist_id: QString, artist_name: QString);

        /// Artist card ⋯ menu "Not interested" — reco-scoped dismissal, NOT the
        /// blacklist. THREE args: the store only backfills an EMPTY name/image,
        /// so dropping the image url would poison the row permanently.
        #[qinvokable]
        fn dismiss_artist(
            self: Pin<&mut QbzBlacklist>,
            artist_id: QString,
            artist_name: QString,
            image_url: QString,
        );
        /// Recommendations tab row `x` — undo one dismissal.
        #[qinvokable]
        fn remove_dismissed(self: Pin<&mut QbzBlacklist>, artist_id: QString);

        /// Albums tab cover window: a JSON array of the cover urls currently
        /// visible. Arrivals land on `QbzLibrary.libraryArtworkReady(url, path)`.
        #[qinvokable]
        fn artwork_window(self: Pin<&mut QbzBlacklist>, urls_json: QString);

        /// Cross-surface settle after any single-item mutation. `key` is the
        /// composite `"{kind}:{id}"` (`"artist:1234567"` / `"album:0060254798934"`)
        /// and the value is the SETTLED state — flipped on success, unchanged on
        /// failure, so this is also the rollback channel.
        #[qsignal]
        fn blacklist_changed(self: Pin<&mut QbzBlacklist>, key: QString, value: bool);
    }

    impl cxx_qt::Threading for QbzBlacklist {}
}

use qbz_blacklist_bridge::QbzBlacklist;

/// Rust side of the blacklist bridge (plain storage, phase-1 pattern).
pub struct QbzBlacklistRust {
    blacklist_json: QString,
    blacklist_loading: bool,
}

impl Default for QbzBlacklistRust {
    fn default() -> Self {
        Self {
            // Never QString::default() for a document QML JSON.parse()s.
            blacklist_json: QString::from("{}"),
            // See the #[qproperty] comment: `true` is load-bearing.
            blacklist_loading: true,
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzBlacklist>> = OnceLock::new();

/// Queue a blacklist-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzBlacklist>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_blacklist_bridge::QbzBlacklist {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] blacklist Qt thread already registered");
        }
    }

    pub fn open_manager(self: Pin<&mut Self>) {
        crate::blacklist_qt::open_manager();
    }
    pub fn reload(self: Pin<&mut Self>) {
        crate::blacklist_qt::load();
    }
    pub fn set_tab(self: Pin<&mut Self>, tab: i32) {
        crate::blacklist_qt::set_tab(tab);
    }
    pub fn search_changed(self: Pin<&mut Self>, query: QString) {
        crate::blacklist_qt::search_changed(query.to_string());
    }
    pub fn toggle_enabled(self: Pin<&mut Self>) {
        crate::blacklist_qt::toggle_enabled();
    }

    pub fn remove_artist(self: Pin<&mut Self>, artist_id: QString) {
        crate::blacklist_qt::remove_artist(artist_id.to_string());
    }
    pub fn clear_all_artists(self: Pin<&mut Self>) {
        crate::blacklist_qt::clear_all_artists();
    }
    pub fn open_artist(self: Pin<&mut Self>, artist_id: QString) {
        crate::blacklist_qt::open_artist(artist_id.to_string());
    }

    pub fn remove_album(self: Pin<&mut Self>, album_id: QString) {
        crate::blacklist_qt::remove_album(album_id.to_string());
    }
    pub fn clear_all_albums(self: Pin<&mut Self>) {
        crate::blacklist_qt::clear_all_albums();
    }
    pub fn open_album(self: Pin<&mut Self>, album_id: QString) {
        crate::blacklist_qt::open_album(album_id.to_string());
    }

    pub fn block_album(
        self: Pin<&mut Self>,
        album_id: QString,
        title: QString,
        artist: QString,
        cover_url: QString,
    ) {
        crate::blacklist_qt::block_album(
            album_id.to_string(),
            title.to_string(),
            artist.to_string(),
            cover_url.to_string(),
        );
    }
    pub fn album_toggle(
        self: Pin<&mut Self>,
        album_id: QString,
        title: QString,
        artist: QString,
        cover_url: QString,
    ) {
        crate::blacklist_qt::album_toggle(
            album_id.to_string(),
            title.to_string(),
            artist.to_string(),
            cover_url.to_string(),
        );
    }
    pub fn artist_toggle(self: Pin<&mut Self>, artist_id: QString, artist_name: QString) {
        crate::blacklist_qt::artist_toggle(artist_id.to_string(), artist_name.to_string());
    }

    pub fn dismiss_artist(
        self: Pin<&mut Self>,
        artist_id: QString,
        artist_name: QString,
        image_url: QString,
    ) {
        crate::blacklist_qt::dismiss_artist(
            artist_id.to_string(),
            artist_name.to_string(),
            image_url.to_string(),
        );
    }
    pub fn remove_dismissed(self: Pin<&mut Self>, artist_id: QString) {
        crate::blacklist_qt::remove_dismissed(artist_id.to_string());
    }

    pub fn artwork_window(self: Pin<&mut Self>, urls_json: QString) {
        crate::blacklist_qt::artwork_window(urls_json.to_string());
    }
}
