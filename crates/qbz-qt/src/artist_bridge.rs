//! QbzArtist — Artist detail view domain bridge (phase 23 split of the
//! QbzBridge God-object; the pattern is documented in main.rs).
//!
//! The module is `qbz_artist_bridge`, not `qbz_artist`: bridge modules are
//! never named after a workspace crate (the `qbz_library` collision is the
//! documented precedent). The QML type name comes from the QObject
//! (`QbzArtist`), not from the module, so QML is unaffected.
//!
//! Props: the one artist-view JSON document + its loading flag, and the one
//! discography-page document (artistReleasesJson — the sub-view reached from
//! "See discography", state and all).
//! Invokables: openArtist, resolveMusician, share, the portrait menu's three
//! (imageAddCustom / imageRemoveCustom / imageSaveAs), loadReleaseSection
//! (per-bucket "Load more"), setSectionSort (per-bucket release sort, persisted
//! by release_type), and the discography page's four —
//! openReleases / releasesLoadMore / releasesSetSort / releasesRetry.
//! Signal: releaseSectionReady (the next page of one releases bucket).
//!
//! `view_param_id` is NOT here and NOT in album_bridge.rs: it was a
//! write-only property (three setters in main.rs — album, artist, playlist —
//! and zero readers in QML or Rust, verified by grep). Splitting it would
//! have duplicated dead state into two singletons; the id each view actually
//! needs already travels inside its own document (`ArtistViewData.id` /
//! `AlbumHeader.id`), so it was deleted outright instead.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_artist_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // --- Artist detail view --------------------------------------------
        #[qproperty(bool, artist_loading)]
        // ONE JSON document (artist_qt.rs ArtistViewData: header + top
        // tracks + releases buckets + labels/similar/playlists).
        #[qproperty(QString, artist_json)]
        // --- Discography sub-view ------------------------------------------
        // ONE JSON document too (artist_releases_qt.rs ArtistReleasesDoc:
        // header + the paged grid + loading / error / has-more / sort). Its
        // `loading` rides INSIDE the document rather than as a second
        // property — the reason is written down on that struct.
        #[qproperty(QString, artist_releases_json)]
        type QbzArtist = super::QbzArtistRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY
        /// domain singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzArtist>);

        /// Open the artist detail view (pushes "artist" on the nav stack).
        #[qinvokable]
        fn open_artist(self: Pin<&mut QbzArtist>, artist_id: QString);
        /// Artist-network / relationship row: resolve a musician NAME (they
        /// carry no catalog id) and open their artist page when confident.
        #[qinvokable]
        fn resolve_musician(self: Pin<&mut QbzArtist>, name: QString, role: QString);

        /// Header ⋯ → Share. `ArtistPageView.slint:530-538` fires
        /// `media-action("artist", ArtistState.id, "share")`; the arm at
        /// `main.rs:12749` copies `share::qobuz_artist_url(&id)` and raises
        /// the "Link copied" success toast. Link-only — artists have no
        /// Song.link/Album.link path (see share_qt.rs).
        ///
        /// `#[auto_cxx_name]` on this block makes the QML name
        /// `QbzArtist.share(...)`; renaming the Rust fn renames the QML call,
        /// which QML only discovers when the menu opens.
        #[qinvokable]
        fn share(self: Pin<&mut QbzArtist>, artist_id: QString);

        /// Portrait right-click menu (cover_artwork_qt.rs): pick / clear the
        /// custom artist image, save the portrait to disk. The store is keyed
        /// by artist NAME (`ArtistPageView.slint:312`), which is why the name
        /// travels instead of the id; `artwork_url` is the Qobuz portrait URL,
        /// needed for the hash -> override link and as the save-as fallback
        /// source. "Open in browser" is NOT here — it reuses
        /// `QbzShell.openExternalUrl`, exactly as the album cover menu does.
        ///
        /// `#[auto_cxx_name]` makes the QML names QbzArtist.imageAddCustom /
        /// .imageRemoveCustom / .imageSaveAs.
        #[qinvokable]
        fn image_add_custom(self: Pin<&mut QbzArtist>, name: QString, artwork_url: QString);
        #[qinvokable]
        fn image_remove_custom(self: Pin<&mut QbzArtist>, name: QString, artwork_url: QString);
        #[qinvokable]
        fn image_save_as(self: Pin<&mut QbzArtist>, name: QString, artwork_url: QString);

        /// ArtistView "In library" toggle: warm the Library feed if this
        /// session never loaded it, else repaint the tab's rows from the
        /// cache (`main::warm_library`). `#[auto_cxx_name]` makes the QML
        /// name `QbzArtist.warmLibrary`.
        #[qinvokable]
        fn warm_library(self: Pin<&mut QbzArtist>);

        /// ArtistView per-section "Load more" — the next releases page.
        #[qinvokable]
        fn load_release_section(
            self: Pin<&mut QbzArtist>,
            artist_id: QString,
            release_type: QString,
            offset: i32,
        );

        /// ArtistView per-section sort — `ReleaseGrid.slint:26`
        /// `set-section-sort(release-type, sort)`, fired by that view's
        /// QbzSelect. `sort` is one of `default` | `newest` | `oldest` |
        /// `title-asc` | `title-desc` (the .slint's index mapping at :81-89),
        /// and those five strings are BOTH the persisted values and the
        /// sort-function keys.
        ///
        /// Persists by release_type and re-sorts the LOADED bucket in place —
        /// the reference never refetches (main.rs:14997-15007 is the whole
        /// handler; see the wire note on `artist_qt::resort_section`).
        ///
        /// `#[auto_cxx_name]` makes the QML name `QbzArtist.setSectionSort`.
        #[qinvokable]
        fn set_section_sort(self: Pin<&mut QbzArtist>, release_type: QString, sort: QString);

        // --- Discography page ----------------------------------------------
        // `#[auto_cxx_name]` makes the QML names QbzArtist.openReleases /
        // .releasesLoadMore / .releasesSetSort / .releasesRetry. QML resolves
        // singleton members LAZILY: a call with no matching invokable here
        // compiles clean and throws the first time a user clicks, so these four
        // and the QML that calls them are one edit.

        /// "See discography" (artist/ReleaseGrid.slint:63-68 ->
        /// ArtistPageView.slint:928-929 -> main.rs:15055-15087) and the album
        /// page's "From the same artist" View all
        /// (album/AlbumPageView.slint:1131 -> main.rs:11839-11865, which pins
        /// `release_type` to "album").
        ///
        /// The NAME travels with the call rather than being read out of the
        /// artist document, because the second door has no artist document
        /// open — it passes `AlbumState.artist`, exactly as main.rs:11844 does.
        #[qinvokable]
        fn open_releases(
            self: Pin<&mut QbzArtist>,
            artist_id: QString,
            artist_name: QString,
            release_type: QString,
        );

        /// Infinite-scroll tail (ArtistReleasesView.slint:121-129). The page
        /// has no "Load more" button — the view fires this when the flick gets
        /// within 600px of the bottom, and every guard lives in Rust as well so
        /// a burst of scroll frames cannot queue two pages.
        #[qinvokable]
        fn releases_load_more(self: Pin<&mut QbzArtist>);

        /// The header's sort select. Client-side only, persisted per
        /// release_type in the SAME store as the artist page's per-section
        /// picker (artist_prefs.rs) — see artist_releases_qt::set_sort.
        #[qinvokable]
        fn releases_set_sort(self: Pin<&mut QbzArtist>, sort: QString);

        /// The error state's Retry button (main.rs:15158-15176) — a full reset
        /// + refetch that records NO nav entry.
        #[qinvokable]
        fn releases_retry(self: Pin<&mut QbzArtist>);

        /// Emitted with the next page of a releases bucket.
        #[qsignal]
        fn release_section_ready(
            self: Pin<&mut QbzArtist>,
            release_type: QString,
            cards_json: QString,
            has_more: bool,
        );
    }

    impl cxx_qt::Threading for QbzArtist {}
}

use qbz_artist_bridge::QbzArtist;

/// Rust side of the artist bridge (plain storage, phase-1 pattern).
pub struct QbzArtistRust {
    artist_loading: bool,
    artist_json: QString,
    artist_releases_json: QString,
}

impl Default for QbzArtistRust {
    fn default() -> Self {
        Self {
            artist_loading: false,
            artist_json: QString::from("{}"),
            // "{}" and not "": the views JSON.parse this straight, and an empty
            // string throws into the catch arm on the very first frame.
            artist_releases_json: QString::from("{}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzArtist>> = OnceLock::new();

/// Queue an artist-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzArtist>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_artist_bridge::QbzArtist {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] artist Qt thread already registered");
        }
    }

    pub fn open_artist(self: Pin<&mut Self>, artist_id: QString) {
        crate::open_artist(artist_id.to_string());
    }

    pub fn warm_library(self: Pin<&mut Self>) {
        crate::warm_library();
    }

    pub fn load_release_section(
        self: Pin<&mut Self>,
        artist_id: QString,
        release_type: QString,
        offset: i32,
    ) {
        crate::load_release_section(artist_id.to_string(), release_type.to_string(), offset);
    }
    /// Straight through to `artist_qt`, with no `crate::` forwarder in main.rs:
    /// unlike `load_release_section` (main.rs:856) this path is SYNCHRONOUS and
    /// touches no `AppRuntime` — it persists a small json and patches the
    /// already-stashed document. Wrapping it in an async hop would only add a
    /// frame of latency between the click and the re-ordered grid.
    pub fn set_section_sort(self: Pin<&mut Self>, release_type: QString, sort: QString) {
        crate::artist_qt::resort_section(&release_type.to_string(), &sort.to_string());
    }

    // Portrait menu — straight through to `cover_artwork_qt`, no `crate::`
    // forwarder: these mutate the artwork store and repaint through
    // `artist_qt::apply_custom_image`, and touch neither nav nor AppRuntime.
    pub fn image_add_custom(self: Pin<&mut Self>, name: QString, artwork_url: QString) {
        crate::cover_artwork_qt::add_custom_artist_image(name.to_string(), artwork_url.to_string());
    }

    pub fn image_remove_custom(self: Pin<&mut Self>, name: QString, artwork_url: QString) {
        crate::cover_artwork_qt::remove_custom_artist_image(
            name.to_string(),
            artwork_url.to_string(),
        );
    }

    pub fn image_save_as(self: Pin<&mut Self>, name: QString, artwork_url: QString) {
        crate::cover_artwork_qt::save_artist_image_as(name.to_string(), artwork_url.to_string());
    }

    pub fn resolve_musician(self: Pin<&mut Self>, name: QString, role: QString) {
        crate::resolve_musician(name.to_string(), role.to_string());
    }

    /// Straight through to `share_qt` — no nav and no bridge state is touched,
    /// so there is no `crate::` forwarder in main.rs for this one (the
    /// `crate::open_artist` style exists because that one mutates both).
    pub fn share(self: Pin<&mut Self>, artist_id: QString) {
        crate::share_qt::share_artist(artist_id.to_string());
    }

    // --- Discography page ---------------------------------------------------
    // One-line delegates into `artist_releases_qt`, with no `crate::` forwarder
    // in main.rs: that indirection exists for the paths that mutate main.rs's
    // own state (LAST_DETAIL, the nav latch). This page owns its state in its
    // own module and records its own nav entry, so there is nothing for main.rs
    // to coordinate.

    pub fn open_releases(
        self: Pin<&mut Self>,
        artist_id: QString,
        artist_name: QString,
        release_type: QString,
    ) {
        crate::artist_releases_qt::open(
            artist_id.to_string(),
            artist_name.to_string(),
            release_type.to_string(),
        );
    }

    pub fn releases_load_more(self: Pin<&mut Self>) {
        crate::artist_releases_qt::load_more();
    }

    pub fn releases_set_sort(self: Pin<&mut Self>, sort: QString) {
        crate::artist_releases_qt::set_sort(sort.to_string());
    }

    pub fn releases_retry(self: Pin<&mut Self>) {
        crate::artist_releases_qt::retry();
    }
}
