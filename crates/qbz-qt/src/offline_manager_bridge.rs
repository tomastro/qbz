//! QbzOffline — Offline Cache Manager domain bridge (phase-23 per-domain
//! pattern; the pattern is documented in main.rs).
//!
//! The module is `qbz_offline_manager_bridge`, not `qbz_offline`: bridge
//! modules are never named after a workspace crate (the `qbz_library`
//! collision is the documented precedent). The QML type name comes from the
//! QObject (`QbzOffline`), not from the module.
//!
//! Props: the ONE manager document + its loading flag.
//! Invokables: the view's own toolbar and per-row actions. Every body is a
//! one-line forward into `crate::offline_manager_qt` (the data) or
//! `crate::offline_cache_qt` (the mutations) — the same split the reference
//! makes between `offline_manager.rs` and `offline_cache.rs`.
//!
//! NOT here: the checkbox state. Selection is QML-local in this port
//! (`offline_manager_qt`'s header explains why), so the two bulk arms take the
//! checked ids as a JSON array instead of asking Rust what is ticked.
//!
//! A new bridge is SEVEN artefacts: this file · its `build.rs` `rust_files`
//! entry (which IS the QML singleton registration — `#[qml_element]
//! #[qml_singleton]` only takes effect for files listed there) · the `main.rs`
//! `mod` line · `Main.qml`'s `boot()` call · the `ContentRouter.qml` arm · the
//! view in `build.rs`'s `qml_files` · the entry point that opens it. A bridge
//! missing from `build.rs` does NOT fail the build — its singleton simply does
//! not exist and every `QbzOffline.foo()` becomes a runtime ReferenceError
//! that `cargo check` cannot see. `qml_singleton_xref.py` is the gate.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_offline_manager_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        /// ONE JSON document (`offline_manager_qt::ManagerDoc`: the stats bar,
        /// the toolbar state, the artist rail and the flat album+track row
        /// list); QML `JSON.parse`s it once per publish.
        ///
        /// `"{}"` and never `""` — the view parses it on its first frame and
        /// an empty string throws.
        #[qproperty(QString, manager_json)]
        /// Defaults to TRUE, like `QbzBlacklist.blacklistLoading` and for the
        /// same reason: the Loader mounts the view BEFORE
        /// `Component.onCompleted` fires `reload()`, so a `false` default
        /// flashes the empty state ("No offline tracks yet") before the
        /// spinner — on the one screen whose whole job is to show you that you
        /// DO have downloads.
        #[qproperty(bool, manager_loading)]
        /// Shared album/playlist offline-cache preflight.  The key is
        /// `album:<id>` / `playlist:<id>` so only the button that launched the
        /// request draws a busy state.  The choice document is deliberately
        /// tiny; Rust retains the fetched track snapshot while the modal is
        /// open instead of serialising a potentially huge playlist to QML.
        #[qproperty(bool, collection_preflight_loading)]
        #[qproperty(QString, collection_preflight_key)]
        #[qproperty(bool, collection_choice_open)]
        #[qproperty(QString, collection_choice_json)]
        type QbzOffline = super::QbzOfflineRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzOffline>);

        /// Settings > Offline > "Open manager": push the route and load.
        #[qinvokable]
        fn open_manager(self: Pin<&mut QbzOffline>);
        /// Re-read the index and re-publish. The view calls this from
        /// `Component.onCompleted` — nav back/forward runs no per-view load.
        #[qinvokable]
        fn reload(self: Pin<&mut QbzOffline>);

        // --- Toolbar ------------------------------------------------------
        /// Artist rail click. `""` = All artists.
        #[qinvokable]
        fn select_artist(self: Pin<&mut QbzOffline>, name: QString);
        /// 0 A-Z · 1 Recent · 2 Largest · 3 Smallest.
        #[qinvokable]
        fn set_sort(self: Pin<&mut QbzOffline>, index: i32);
        /// "Failed only" chip.
        #[qinvokable]
        fn toggle_failed(self: Pin<&mut QbzOffline>);
        /// The GB field: persist the cache ceiling and refresh.
        #[qinvokable]
        fn set_limit(self: Pin<&mut QbzOffline>, gb: i32);

        // --- Whole-cache actions (also the two Settings rows) -------------
        /// Open the cache directory in the desktop file manager.
        #[qinvokable]
        fn open_folder(self: Pin<&mut QbzOffline>);
        /// Purge every cached copy. The CALLER confirms — this does not.
        #[qinvokable]
        fn clear_all(self: Pin<&mut QbzOffline>);

        // --- Album / playlist entry-point preflight -----------------------
        #[qinvokable]
        fn cache_album(self: Pin<&mut QbzOffline>, album_id: QString);
        #[qinvokable]
        fn cache_playlist(self: Pin<&mut QbzOffline>, playlist_id: QString);
        /// `mode`: `all` (also repairs existing copies) | `missing`.
        #[qinvokable]
        fn confirm_collection_cache(self: Pin<&mut QbzOffline>, mode: QString);
        #[qinvokable]
        fn cancel_collection_cache(self: Pin<&mut QbzOffline>);

        // --- Per-row ------------------------------------------------------
        #[qinvokable]
        fn remove_track(self: Pin<&mut QbzOffline>, track_id: QString);
        #[qinvokable]
        fn remove_album(self: Pin<&mut QbzOffline>, album_id: QString);
        #[qinvokable]
        fn redownload_track(self: Pin<&mut QbzOffline>, track_id: QString);
        #[qinvokable]
        fn redownload_album(self: Pin<&mut QbzOffline>, album_id: QString);
        /// Track row body click — play it now.
        #[qinvokable]
        fn play_track(self: Pin<&mut QbzOffline>, track_id: QString);

        // --- Bulk (the checked rows; ids come down as a JSON array) -------
        #[qinvokable]
        fn bulk_redownload(self: Pin<&mut QbzOffline>, ids_json: QString);
        #[qinvokable]
        fn bulk_remove(self: Pin<&mut QbzOffline>, ids_json: QString);
    }

    impl cxx_qt::Threading for QbzOffline {}
}

use qbz_offline_manager_bridge::QbzOffline;

/// Rust side of the offline bridge (plain storage, phase-1 pattern).
pub struct QbzOfflineRust {
    manager_json: QString,
    manager_loading: bool,
    collection_preflight_loading: bool,
    collection_preflight_key: QString,
    collection_choice_open: bool,
    collection_choice_json: QString,
}

impl Default for QbzOfflineRust {
    fn default() -> Self {
        Self {
            manager_json: QString::from("{}"),
            manager_loading: true,
            collection_preflight_loading: false,
            collection_preflight_key: QString::default(),
            collection_choice_open: false,
            collection_choice_json: QString::from("{}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzOffline>> = OnceLock::new();

/// Queue an offline-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzOffline>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

/// Parse a JSON array of track-id strings into ids. Anything unparseable is
/// dropped rather than failing the whole bulk action — the array is built by
/// QML from row ids the document itself published, so a bad entry means one
/// stale row, not a broken button.
fn ids_from_json(raw: &str) -> Vec<u64> {
    serde_json::from_str::<Vec<String>>(raw)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.parse::<u64>().ok())
        .collect()
}

impl qbz_offline_manager_bridge::QbzOffline {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] offline Qt thread already registered");
        }
    }

    pub fn open_manager(self: Pin<&mut Self>) {
        crate::offline_manager_qt::open();
    }
    pub fn reload(self: Pin<&mut Self>) {
        crate::offline_manager_qt::load();
    }

    pub fn select_artist(self: Pin<&mut Self>, name: QString) {
        crate::offline_manager_qt::select_artist(name.to_string());
    }
    pub fn set_sort(self: Pin<&mut Self>, index: i32) {
        crate::offline_manager_qt::set_sort(index);
    }
    pub fn toggle_failed(self: Pin<&mut Self>) {
        crate::offline_manager_qt::toggle_failed();
    }
    pub fn set_limit(self: Pin<&mut Self>, gb: i32) {
        crate::offline_manager_qt::set_limit(gb);
    }

    pub fn open_folder(self: Pin<&mut Self>) {
        crate::offline_cache_qt::open_folder();
    }
    pub fn clear_all(self: Pin<&mut Self>) {
        crate::offline_cache_qt::clear_all();
    }

    pub fn cache_album(self: Pin<&mut Self>, album_id: QString) {
        crate::offline_cache_qt::cache_album(album_id.to_string());
    }
    pub fn cache_playlist(self: Pin<&mut Self>, playlist_id: QString) {
        crate::offline_cache_qt::cache_playlist(playlist_id.to_string());
    }
    pub fn confirm_collection_cache(self: Pin<&mut Self>, mode: QString) {
        crate::offline_cache_qt::confirm_collection_cache(mode.to_string());
    }
    pub fn cancel_collection_cache(self: Pin<&mut Self>) {
        crate::offline_cache_qt::cancel_collection_cache();
    }

    pub fn remove_track(self: Pin<&mut Self>, track_id: QString) {
        if let Ok(id) = track_id.to_string().parse::<u64>() {
            crate::offline_cache_qt::remove_cached(id);
        }
    }
    pub fn remove_album(self: Pin<&mut Self>, album_id: QString) {
        crate::offline_cache_qt::remove_album(album_id.to_string());
    }
    pub fn redownload_track(self: Pin<&mut Self>, track_id: QString) {
        if let Ok(id) = track_id.to_string().parse::<u64>() {
            crate::offline_cache_qt::redownload_track(id);
        }
    }
    pub fn redownload_album(self: Pin<&mut Self>, album_id: QString) {
        // `failed_only = false`: the row's refresh button re-fetches the whole
        // album, the same arm the album page's "Refresh offline copy" takes.
        crate::offline_cache_qt::redownload_album(album_id.to_string(), false);
    }
    pub fn play_track(self: Pin<&mut Self>, track_id: QString) {
        if let Ok(id) = track_id.to_string().parse::<u64>() {
            crate::play_track(id);
        }
    }

    pub fn bulk_redownload(self: Pin<&mut Self>, ids_json: QString) {
        for id in ids_from_json(&ids_json.to_string()) {
            crate::offline_cache_qt::redownload_track(id);
        }
    }
    pub fn bulk_remove(self: Pin<&mut Self>, ids_json: QString) {
        for id in ids_from_json(&ids_json.to_string()) {
            crate::offline_cache_qt::remove_cached(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ids_from_json;

    #[test]
    fn bulk_ids_parse_and_survive_a_bad_entry() {
        assert_eq!(ids_from_json(r#"["1","22","333"]"#), vec![1, 22, 333]);
        // One stale row must not cost the other two their action.
        assert_eq!(ids_from_json(r#"["1","plex:9","3"]"#), vec![1, 3]);
        assert!(ids_from_json("[]").is_empty());
        assert!(ids_from_json("not json").is_empty());
    }
}
