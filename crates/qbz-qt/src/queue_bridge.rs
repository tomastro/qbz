//! QbzQueue — queue-panel domain bridge (phase 23 split of the QbzBridge
//! God-object; the pattern is documented in main.rs). Props: the legacy
//! queueModel list + the queueJson document. Invokables: the panel's
//! tabs/pagination/search and row actions — one-line forwards into the
//! crate handlers.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_queue {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;

        include!("cxx-qt-lib/qlist.h");
        type QList_QVariant = cxx_qt_lib::QList<cxx_qt_lib::QVariant>;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // --- Queue panel -------------------------------------------------
        // POC: empty until phase 4 (QML shows the empty states).
        #[qproperty(QList_QVariant, queue_model)]
        // --- Queue panel (phase 9) -----------------------------------------
        // One JSON document (queue_qt.rs QueueDoc: current/upcoming/history
        // + pagination + #442 section markers). Supersedes `queueModel`.
        #[qproperty(QString, queue_json)]
        // True whenever the core still has at least one queue row, including
        // the idle state produced by Clear -> Add/Next/Later. Unlike
        // `QbzPlayer.npHasTrack`, this deliberately does not require a current
        // cursor: it is the transport's answer to "can Play start anything?".
        #[qproperty(bool, has_play_target)]
        // Full chronological queue projection used only by QueueView. It is
        // opt-in so ordinary sidebar publishes stay paged and cheap.
        #[qproperty(QString, extended_queue_json)]
        // --- Immersive coverflow (2026-08-02 immersive-port contract §4.4) --
        // `{"index":i32,"tracks":[{id,title,artist,artUrl}]}` over the FULL
        // flat queue ([history oldest-first, NOW, upcoming]); `index` = flat
        // position of NOW (= history.len). The tracks array is rebuilt only
        // on id-sequence change; a pure advance moves only `index`. The
        // immersive Queue badge + coverflow read THIS, never the paged
        // `queue_json`. Full-shape default, never "{}" (trap 15).
        #[qproperty(QString, coverflow_json)]
        // --- Sleep timer (queue footer) ------------------------------------
        // Kept OUT of `queue_json`: the countdown reformats every second and
        // the document is re-parsed by three QML files on every change, so
        // folding it in would re-parse the whole queue once a second for a
        // string nothing else reads. Two scalar properties instead — the same
        // split the reference makes with its own `SleepTimerState` global.
        // `sleep_remaining` is pre-formatted in Rust (single source of truth).
        // "The queue was empty and you just dropped tracks into it — start
        // playing?" (owner ask 2026-08-10). A drop is an ADD, never a play:
        // `insert_dragged_track` deliberately does not start playback, because
        // dropping onto a queue that is already running must not hijack it.
        // On an EMPTY queue that leaves the user one click short of what they
        // almost certainly meant, so the panel asks instead of guessing.
        //
        // Set only on the empty->filled transition, and cleared by either
        // answer. A property rather than a signal so a prompt that is up when
        // the panel remounts is still up afterwards.
        #[qproperty(bool, drop_play_prompt)]
        #[qproperty(bool, sleep_active)]
        #[qproperty(QString, sleep_remaining)]
        type QbzQueue = super::QbzQueueRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY
        /// domain singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzQueue>);

        /// Queue panel: tabs/pagination/search.
        #[qinvokable]
        fn queue_set_page(self: Pin<&mut QbzQueue>, page: i32);
        #[qinvokable]
        fn queue_set_search(self: Pin<&mut QbzQueue>, query: QString);
        /// Row actions.
        #[qinvokable]
        fn queue_play_upcoming(self: Pin<&mut QbzQueue>, index: i32);
        /// Answer the empty-queue drop prompt: `true` starts the queue from
        /// its first row, `false` just dismisses.
        #[qinvokable]
        fn queue_answer_drop_prompt(self: Pin<&mut QbzQueue>, play: bool);
        /// Immersive coverflow / up-next rows: play by QUEUE-WIDE 0-based
        /// index into the UNFILTERED upcoming list (§4.4). `queuePlayUpcoming`
        /// is PAGE-LOCAL (resolved through the filtered/paginated VIEW) —
        /// never use it from immersive (trap 23).
        #[qinvokable]
        fn queue_play_upcoming_flat(self: Pin<&mut QbzQueue>, index: i32);
        #[qinvokable]
        fn queue_extended_opened(self: Pin<&mut QbzQueue>);
        #[qinvokable]
        fn queue_extended_closed(self: Pin<&mut QbzQueue>);
        #[qinvokable]
        fn queue_extended_play(
            self: Pin<&mut QbzQueue>,
            phase: QString,
            index: i32,
            expected_id: QString,
        );
        #[qinvokable]
        fn queue_extended_drop(
            self: Pin<&mut QbzQueue>,
            phase: QString,
            index: i32,
            expected_id: QString,
            target_phase: QString,
            slot: i32,
        );
        #[qinvokable]
        fn queue_remove_upcoming_flat(self: Pin<&mut QbzQueue>, index: i32);
        #[qinvokable]
        fn queue_remove_all_after_flat(self: Pin<&mut QbzQueue>, index: i32);
        #[qinvokable]
        fn queue_remove_upcoming(self: Pin<&mut QbzQueue>, index: i32);
        #[qinvokable]
        fn queue_remove_all_after(self: Pin<&mut QbzQueue>, index: i32);
        #[qinvokable]
        fn queue_move_track(self: Pin<&mut QbzQueue>, from: i32, to: i32);
        #[qinvokable]
        fn queue_play_history(self: Pin<&mut QbzQueue>, index: i32);
        #[qinvokable]
        fn queue_toggle_favorite(self: Pin<&mut QbzQueue>, kind: QString, id: QString);
        /// Row menu: "Stop after this" / "Cancel stop after this" (idempotent).
        #[qinvokable]
        fn queue_toggle_stop_after(self: Pin<&mut QbzQueue>, id: QString);
        /// Row menu: seed the Add-to-Playlist picker with this one row.
        #[qinvokable]
        fn queue_add_to_playlist(self: Pin<&mut QbzQueue>, index: i32);
        /// Footer: Clear queue.
        #[qinvokable]
        fn queue_clear(self: Pin<&mut QbzQueue>);
        /// Footer: save the whole queue as a playlist (seeds the picker).
        #[qinvokable]
        fn queue_save_as_playlist(self: Pin<&mut QbzQueue>);
        /// Footer: toggle infinite play (InfiniteRadio autoplay).
        #[qinvokable]
        fn queue_toggle_infinite_play(self: Pin<&mut QbzQueue>);
        /// The panel became visible — pull a fresh snapshot. The queue can
        /// change while the panel is closed (a session restore, a play from
        /// any view), and the publish dedup means the panel would otherwise
        /// mount against whatever document happened to be last posted.
        #[qinvokable]
        fn queue_panel_opened(self: Pin<&mut QbzQueue>);
        /// Footer sleep timer: arm for `minutes` / cancel.
        #[qinvokable]
        fn sleep_timer_set(self: Pin<&mut QbzQueue>, minutes: i32);
        #[qinvokable]
        fn sleep_timer_cancel(self: Pin<&mut QbzQueue>);
    }

    impl cxx_qt::Threading for QbzQueue {}
}

use cxx_qt_lib::{QList, QVariant};
use qbz_queue::QbzQueue;

type QListQVariant = QList<QVariant>;

/// Rust side of the queue bridge (plain storage, phase-1 pattern).
pub struct QbzQueueRust {
    queue_model: QListQVariant,
    queue_json: QString,
    has_play_target: bool,
    extended_queue_json: QString,
    coverflow_json: QString,
    drop_play_prompt: bool,
    sleep_active: bool,
    sleep_remaining: QString,
}

impl Default for QbzQueueRust {
    fn default() -> Self {
        Self {
            queue_model: QListQVariant::default(),
            queue_json: QString::from("{}"),
            has_play_target: false,
            extended_queue_json: QString::from(
                r#"{"rows":[],"currentIndex":-1,"historyCount":0,"upcomingCount":0,"stopAfterId":"","infinitePlay":false,"searchQuery":""}"#,
            ),
            // Full-shape default (trap 15): QML reads `.tracks.length` in the
            // pre-publish frame.
            coverflow_json: QString::from(r#"{"index":0,"tracks":[]}"#),
            drop_play_prompt: false,
            sleep_active: false,
            sleep_remaining: QString::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzQueue>> = OnceLock::new();

/// Queue a queue-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzQueue>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_queue::QbzQueue {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] queue Qt thread already registered");
        }
    }

    pub fn queue_set_page(self: Pin<&mut Self>, page: i32) {
        crate::queue_set_page(page);
    }

    pub fn queue_set_search(self: Pin<&mut Self>, query: QString) {
        crate::queue_set_search(query.to_string());
    }

    pub fn queue_answer_drop_prompt(self: Pin<&mut Self>, play: bool) {
        crate::queue_qt::answer_drop_prompt(play);
    }

    pub fn queue_play_upcoming(self: Pin<&mut Self>, index: i32) {
        crate::queue_play_upcoming(index);
    }

    pub fn queue_play_upcoming_flat(self: Pin<&mut Self>, index: i32) {
        crate::queue_play_upcoming_flat(index);
    }

    pub fn queue_extended_opened(self: Pin<&mut Self>) {
        crate::queue_extended_opened();
    }

    pub fn queue_extended_closed(self: Pin<&mut Self>) {
        crate::queue_qt::extended_closed();
    }

    pub fn queue_extended_play(
        self: Pin<&mut Self>,
        phase: QString,
        index: i32,
        expected_id: QString,
    ) {
        crate::queue_extended_play(phase.to_string(), index, expected_id.to_string());
    }

    pub fn queue_extended_drop(
        self: Pin<&mut Self>,
        phase: QString,
        index: i32,
        expected_id: QString,
        target_phase: QString,
        slot: i32,
    ) {
        crate::queue_extended_drop(
            phase.to_string(),
            index,
            expected_id.to_string(),
            target_phase.to_string(),
            slot,
        );
    }

    pub fn queue_remove_upcoming_flat(self: Pin<&mut Self>, index: i32) {
        crate::queue_remove_upcoming_flat(index);
    }

    pub fn queue_remove_all_after_flat(self: Pin<&mut Self>, index: i32) {
        crate::queue_remove_all_after_flat(index);
    }

    pub fn queue_remove_upcoming(self: Pin<&mut Self>, index: i32) {
        crate::queue_remove_upcoming(index);
    }

    pub fn queue_remove_all_after(self: Pin<&mut Self>, index: i32) {
        crate::queue_remove_all_after(index);
    }

    pub fn queue_move_track(self: Pin<&mut Self>, from: i32, to: i32) {
        crate::queue_move_track(from, to);
    }

    pub fn queue_play_history(self: Pin<&mut Self>, index: i32) {
        crate::queue_play_history(index);
    }

    pub fn queue_toggle_favorite(self: Pin<&mut Self>, kind: QString, id: QString) {
        crate::queue_toggle_favorite(kind.to_string(), id.to_string());
    }

    pub fn queue_toggle_stop_after(self: Pin<&mut Self>, id: QString) {
        crate::queue_toggle_stop_after(id.to_string());
    }

    pub fn queue_add_to_playlist(self: Pin<&mut Self>, index: i32) {
        crate::queue_add_to_playlist(index);
    }

    pub fn queue_clear(self: Pin<&mut Self>) {
        crate::queue_clear();
    }

    pub fn queue_save_as_playlist(self: Pin<&mut Self>) {
        crate::queue_save_as_playlist();
    }

    pub fn queue_toggle_infinite_play(self: Pin<&mut Self>) {
        crate::queue_toggle_infinite_play();
    }

    pub fn queue_panel_opened(self: Pin<&mut Self>) {
        crate::queue_panel_opened();
    }

    pub fn sleep_timer_set(self: Pin<&mut Self>, minutes: i32) {
        crate::sleep_timer_qt::set(crate::app(), minutes);
    }

    pub fn sleep_timer_cancel(self: Pin<&mut Self>) {
        crate::sleep_timer_qt::cancel();
    }
}
