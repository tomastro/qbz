//! QbzPurchases — the Purchases domain bridge (contract §G.1).
//!
//! Two documents and a publish counter. The controller is
//! `purchases_qt.rs`; this file is glue and nothing else, exactly like
//! `musician_bridge.rs`.
//!
//! The module is `qbz_purchases_bridge`, not `qbz_purchases`: bridge modules
//! are never named after a workspace crate (the `qbz_library` collision is the
//! documented precedent). The QML type name comes from the QObject
//! (`QbzPurchases`), not from the module.
//!
//! # A new bridge is SEVEN artefacts, not one file
//!
//! This file · its `build.rs` `rust_files` entry · the `main.rs` `mod` line ·
//! the QML singleton registration (which IS the build.rs entry — `#[qml_element]
//! #[qml_singleton]` only takes effect for files listed there) · `Main.qml`'s
//! `boot()` call · `ContentRouter.qml`'s `purchases` AND `purchase-album` arms ·
//! the sidebar row. Five of those belong to lane D; this lane ships the two
//! files it owns plus the `mod` lines. A bridge missing from `build.rs` does
//! not fail the build: its QML singleton simply does not exist, and every
//! `QbzPurchases.foo()` becomes a runtime ReferenceError that `cargo check`
//! cannot see. `qml_singleton_xref.py` is the gate that catches it.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_purchases_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        /// The Purchases LIST document (route `purchases`) — state, tab, the
        /// two per-type counters, the toolbar and the two already
        /// filtered+sorted row arrays. Shape frozen in contract §G.2.
        #[qproperty(QString, list_json)]
        /// The purchased-album DETAIL document (route `purchase-album`) —
        /// header, format dropdown, disc-grouped tracklist, per-track download
        /// status, the track-counted progress block and the goodies list.
        /// Shape frozen in contract §G.3.
        ///
        /// `"{}"` and never `""`: both documents are `JSON.parse`d on the first
        /// frame, and an empty string throws. (`""` is a legitimate default
        /// only where empty IS the closed state of a modal — neither of these
        /// is one.)
        #[qproperty(QString, album_json)]
        /// Publish counter, bumped by every write to EITHER document above.
        /// QML re-parses on the QString change already; this exists so a view
        /// can force a re-read when the text happens to be byte-identical
        /// (re-opening the same album, or a filter toggle that changes nothing
        /// visible), the same trick `QbzSession.trRev` plays.
        #[qproperty(i32, purchases_rev)]
        /// Revision for the process-wide entitlement index. AlbumCard passes
        /// this into its read-only lookup invokables so every mounted card
        /// re-evaluates when the account index is refreshed or invalidated.
        #[qproperty(i32, entitlement_rev)]
        /// The purchases with a download IN FLIGHT right now, as a JSON array
        /// of `{ id, title }` — the nav flyout's "Downloads · N" section. A
        /// download is process-wide and survives leaving its page; this is
        /// how the user finds it again. `"[]"` while nothing runs.
        #[qproperty(QString, active_downloads_json)]
        type QbzPurchases = super::QbzPurchasesRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzPurchases>);

        /// Entering the list route. Runs the ONCE-per-session guarded metadata
        /// load (registry + the two per-type totals, concurrently) and the
        /// active tab's load. Re-entry while the metadata is already loaded
        /// only reloads the tab — the §5 `metadataLoaded` guard, which is
        /// Tauri's and is owed.
        #[qinvokable]
        fn open_list(self: Pin<&mut QbzPurchases>);

        /// Leaving the list route — `Component.onDestruction`. Drops ALL
        /// transient state (tab, sort, direction, grouping, view mode, search
        /// text and both row caches) because Tauri's `{#if}` chain destroys the
        /// component and Qt must not preserve it by accident (§10-F). The four
        /// persisted preferences and the region-notice flag survive; so does
        /// the download store, which is process-wide by design.
        #[qinvokable]
        fn leave(self: Pin<&mut QbzPurchases>);

        /// `"albums"` | `"tracks"`. Loads that tab's list if it has not been
        /// loaded this visit.
        #[qinvokable]
        fn set_tab(self: Pin<&mut QbzPurchases>, tab: QString);

        /// The search FIRE — **QML owns the 300 ms debounce** with a one-shot
        /// `Timer`; this is what the timer calls. It REFETCHES FROM THE SERVER
        /// and re-requests BOTH types, then filters in Rust (§5). An empty
        /// query refetches the active tab.
        #[qinvokable]
        fn search(self: Pin<&mut QbzPurchases>, query: QString);

        /// Albums tab only. `key` is `"purchased"` | `"artist"` | `"album"` |
        /// `"quality"`; `ascending` is the direction.
        #[qinvokable]
        fn set_sort(self: Pin<&mut QbzPurchases>, key: QString, ascending: bool);

        /// `"off"` | `"alpha"` | `"artist"` (Albums) · `"off"` | `"name"` |
        /// `"artist"` | `"album"` (Tracks).
        #[qinvokable]
        fn set_group(self: Pin<&mut QbzPurchases>, key: QString);

        /// `"grid"` | `"list"`. Albums tab only.
        #[qinvokable]
        fn set_view_mode(self: Pin<&mut QbzPurchases>, mode: QString);

        /// Persisted (`purchases_hide_unavailable`).
        #[qinvokable]
        fn set_hide_unavailable(self: Pin<&mut QbzPurchases>, on: bool);

        /// Persisted (`purchases_hide_downloaded`).
        #[qinvokable]
        fn set_hide_downloaded(self: Pin<&mut QbzPurchases>, on: bool);

        /// `"all"` | `"hires"` | `"cd"` | `"lossy"`. Persisted
        /// (`purchases_quality_filter`) — the FOURTH preference, the one the
        /// first draft of the contract missed.
        #[qinvokable]
        fn set_quality_filter(self: Pin<&mut QbzPurchases>, value: QString);

        /// The genre filter (albums tab). Transient; `""` = all genres.
        #[qinvokable]
        fn set_genre_filter(self: Pin<&mut QbzPurchases>, value: QString);

        /// Dismiss the one-time region notice. Persisted
        /// (`purchases_region_notice_seen`).
        #[qinvokable]
        fn dismiss_region_notice(self: Pin<&mut QbzPurchases>);

        /// Open the purchased-album detail. RECORDS the album and starts the
        /// fetch; the QML caller then calls `QbzShell.navigateTo("purchase-album")`
        /// — in THAT order (§C.2).
        #[qinvokable]
        fn open_album(self: Pin<&mut QbzPurchases>, album_id: QString);

        /// AlbumView-only door: revalidates exact full-album ownership,
        /// availability and explicit entitled formats before navigating.
        #[qinvokable]
        fn open_entitled_album(self: Pin<&mut QbzPurchases>, album_id: QString);

        /// Persist AlbumView's `qobuz` or exact `purchase(format_id)` source.
        /// Track ids remain QStrings inside the JSON array (QML ints are 32-bit).
        #[qinvokable]
        fn set_album_playback_mode(
            self: Pin<&mut QbzPurchases>,
            album_id: QString,
            mode: QString,
            format_id: i32,
            track_ids_json: QString,
        );

        /// Purchase detail Play: prefer the highest complete healthy local
        /// copy, then enter the ordinary album playback pipeline.
        #[qinvokable]
        fn play_album(self: Pin<&mut QbzPurchases>);

        /// Read-only AlbumCard entitlement lookup. `revision` is a QML binding
        /// dependency; the controller reads the process-wide index.
        #[qinvokable]
        fn is_album_purchased(self: &QbzPurchases, album_id: QString, revision: i32) -> bool;

        /// Highest exact format owned for the AlbumCard tooltip, or empty when
        /// the entitlement carries no format list.
        #[qinvokable]
        fn album_purchased_quality(
            self: &QbzPurchases,
            album_id: QString,
            revision: i32,
        ) -> QString;

        /// Change the selected download format. RE-SCOPES every downloaded
        /// mark on the detail screen, the progress block AND the
        /// Add-to-Library affordance (§6) — the single most surprising
        /// behaviour on that screen.
        #[qinvokable]
        fn set_format(self: Pin<&mut QbzPurchases>, format_id: i32);

        /// Native folder picker for the download destination.
        #[qinvokable]
        fn choose_destination(self: Pin<&mut QbzPurchases>);

        /// "Download all" — the FRONTEND loop: strictly sequential,
        /// concurrency 1, cancellable between tracks (§4.1).
        #[qinvokable]
        fn download_album(self: Pin<&mut QbzPurchases>);

        /// Resume one exact persistent partial copy in its existing leaf.
        #[qinvokable]
        fn continue_copy(self: Pin<&mut QbzPurchases>, copy_id: QString);

        /// One track. `track_id` is a **QString**, never an int: QML `int` is
        /// 32-bit and Qobuz track ids overflow it (§G.0).
        #[qinvokable]
        fn download_track(self: Pin<&mut QbzPurchases>, track_id: QString);

        /// The retry control on a failed row. Same primitive as
        /// `download_track`; a separate name because the row's affordance is a
        /// different one and the contract lists both.
        #[qinvokable]
        fn retry_track(self: Pin<&mut QbzPurchases>, track_id: QString);

        /// Cooperative cancel. The in-flight track finishes; every track that
        /// has not started becomes `cancelled` (§4.1).
        #[qinvokable]
        fn cancel_album_download(self: Pin<&mut QbzPurchases>);

        /// Album-level goodies (booklets, videos). Counted SEPARATELY from the
        /// track progress — a goodie is not a track (§14.3).
        #[qinvokable]
        fn download_goodies(self: Pin<&mut QbzPurchases>);

        /// "Add to Library" — the full sequence, not a banner (§6): add the
        /// resolved album subfolder as a library folder, clear the download
        /// state, toast, start a background scan.
        #[qinvokable]
        fn add_to_library(self: Pin<&mut QbzPurchases>);

        /// Register/refresh one exact copy through Local Library's typed API.
        #[qinvokable]
        fn add_copy_to_library(self: Pin<&mut QbzPurchases>, copy_id: QString);

        /// Open the registered copy folder in the platform file browser.
        #[qinvokable]
        fn show_copy_location(self: Pin<&mut QbzPurchases>, copy_id: QString);

        /// Remove this exact copy folder from Local Library, preserving files
        /// and the purchase-copy inventory.
        #[qinvokable]
        fn remove_copy_from_library(self: Pin<&mut QbzPurchases>, copy_id: QString);

        /// Forget an unindexed download record without deleting user files.
        #[qinvokable]
        fn forget_copy_record(self: Pin<&mut QbzPurchases>, copy_id: QString);

        /// Acknowledge the one-shot post-batch modal without mutating the copy.
        #[qinvokable]
        fn dismiss_batch_result(self: Pin<&mut QbzPurchases>, copy_id: QString);
    }

    impl cxx_qt::Threading for QbzPurchases {}
}

use qbz_purchases_bridge::QbzPurchases;

/// Rust side of the purchases bridge (plain storage, phase-1 pattern).
pub struct QbzPurchasesRust {
    list_json: QString,
    album_json: QString,
    purchases_rev: i32,
    entitlement_rev: i32,
    active_downloads_json: QString,
}

impl Default for QbzPurchasesRust {
    fn default() -> Self {
        Self {
            // "{}" and not "": both views JSON.parse these straight, and an
            // empty string throws into the catch arm on the very first frame.
            list_json: QString::from("{}"),
            album_json: QString::from("{}"),
            purchases_rev: 0,
            entitlement_rev: 0,
            // Same rule: the flyout JSON.parses it on the first frame.
            active_downloads_json: QString::from("[]"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzPurchases>> = OnceLock::new();

/// Queue a purchases-bridge mutation onto the Qt event loop (no-op before boot
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzPurchases>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_purchases_bridge::QbzPurchases {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] purchases Qt thread already registered");
        }
    }

    // One-line delegates into `purchases_qt`. No `crate::` forwarder in
    // main.rs: this domain owns all of its state in its own module, and the one
    // navigation it performs is recorded by the QML caller's
    // `QbzShell.navigateTo` (§C.2), so there is nothing for main.rs to
    // coordinate.

    pub fn open_list(self: Pin<&mut Self>) {
        crate::purchases_qt::open_list();
    }

    pub fn leave(self: Pin<&mut Self>) {
        crate::purchases_qt::leave();
    }

    pub fn set_tab(self: Pin<&mut Self>, tab: QString) {
        crate::purchases_qt::set_tab(tab.to_string());
    }

    pub fn search(self: Pin<&mut Self>, query: QString) {
        crate::purchases_qt::search(query.to_string());
    }

    pub fn set_sort(self: Pin<&mut Self>, key: QString, ascending: bool) {
        crate::purchases_qt::set_sort(key.to_string(), ascending);
    }

    pub fn set_group(self: Pin<&mut Self>, key: QString) {
        crate::purchases_qt::set_group(key.to_string());
    }

    pub fn set_view_mode(self: Pin<&mut Self>, mode: QString) {
        crate::purchases_qt::set_view_mode(mode.to_string());
    }

    pub fn set_hide_unavailable(self: Pin<&mut Self>, on: bool) {
        crate::purchases_qt::set_hide_unavailable(on);
    }

    pub fn set_hide_downloaded(self: Pin<&mut Self>, on: bool) {
        crate::purchases_qt::set_hide_downloaded(on);
    }

    pub fn set_quality_filter(self: Pin<&mut Self>, value: QString) {
        crate::purchases_qt::set_quality_filter(value.to_string());
    }

    pub fn set_genre_filter(self: Pin<&mut Self>, value: QString) {
        crate::purchases_qt::set_genre_filter(value.to_string());
    }

    pub fn dismiss_region_notice(self: Pin<&mut Self>) {
        crate::purchases_qt::dismiss_region_notice();
    }

    pub fn open_album(self: Pin<&mut Self>, album_id: QString) {
        crate::purchases_qt::open_album(album_id.to_string());
    }

    pub fn open_entitled_album(self: Pin<&mut Self>, album_id: QString) {
        crate::purchases_qt::open_entitled_album(album_id.to_string());
    }

    pub fn set_album_playback_mode(
        self: Pin<&mut Self>,
        album_id: QString,
        mode: QString,
        format_id: i32,
        track_ids_json: QString,
    ) {
        crate::purchases_qt::set_album_playback_mode(
            album_id.to_string(),
            mode.to_string(),
            format_id,
            track_ids_json.to_string(),
        );
    }

    pub fn play_album(self: Pin<&mut Self>) {
        crate::purchases_qt::play_album();
    }

    pub fn is_album_purchased(&self, album_id: QString, revision: i32) -> bool {
        crate::purchases_qt::is_album_purchased(&album_id.to_string(), revision)
    }

    pub fn album_purchased_quality(&self, album_id: QString, revision: i32) -> QString {
        QString::from(
            crate::purchases_qt::album_purchased_quality(&album_id.to_string(), revision).as_str(),
        )
    }

    pub fn set_format(self: Pin<&mut Self>, format_id: i32) {
        crate::purchases_qt::set_format(format_id);
    }

    pub fn choose_destination(self: Pin<&mut Self>) {
        crate::purchases_qt::choose_destination();
    }

    pub fn download_album(self: Pin<&mut Self>) {
        crate::purchases_qt::download_album();
    }

    pub fn continue_copy(self: Pin<&mut Self>, copy_id: QString) {
        crate::purchases_qt::continue_copy(copy_id.to_string());
    }

    pub fn download_track(self: Pin<&mut Self>, track_id: QString) {
        crate::purchases_qt::download_track(track_id.to_string());
    }

    pub fn retry_track(self: Pin<&mut Self>, track_id: QString) {
        crate::purchases_qt::download_track(track_id.to_string());
    }

    pub fn cancel_album_download(self: Pin<&mut Self>) {
        crate::purchases_qt::cancel_album_download();
    }

    pub fn download_goodies(self: Pin<&mut Self>) {
        crate::purchases_qt::download_goodies();
    }

    pub fn add_to_library(self: Pin<&mut Self>) {
        crate::purchases_qt::add_to_library();
    }

    pub fn add_copy_to_library(self: Pin<&mut Self>, copy_id: QString) {
        crate::purchases_qt::add_copy_to_library(copy_id.to_string());
    }

    pub fn show_copy_location(self: Pin<&mut Self>, copy_id: QString) {
        crate::purchases_qt::show_copy_location(copy_id.to_string());
    }

    pub fn remove_copy_from_library(self: Pin<&mut Self>, copy_id: QString) {
        crate::purchases_qt::remove_copy_from_library(copy_id.to_string());
    }

    pub fn forget_copy_record(self: Pin<&mut Self>, copy_id: QString) {
        crate::purchases_qt::forget_copy_record(copy_id.to_string());
    }

    pub fn dismiss_batch_result(self: Pin<&mut Self>, copy_id: QString) {
        crate::purchases_qt::dismiss_batch_result(copy_id.to_string());
    }
}
