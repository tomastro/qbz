//! QbzDacWizard — the HiFi Wizard's QML singleton.
//!
//! A singleton of its own rather than more surface on `QbzBridge`, for the same
//! reason `QbzFolderEdit` is separate: the wizard is one self-contained modal
//! with a document nobody else reads, and its ONE caller (the Settings > Audio
//! row) does not want the settings document republished on every wizard poll —
//! the read-back ticks every 1.5 s while the test plays.
//!
//! **No `#[qsignal]`.** Every outcome is a republish of `wizardJson`; the modal
//! renders from the document and keeps no copy of anything Rust owns
//! (`playlist_picker_bridge.rs:13`).
//!
//! The module is `qbz_dac_wizard_bridge` — a bridge module is never named after
//! a workspace crate (E0659, and `qbz-dac-wizard` IS one). The QML type name
//! comes from the QObject, so QML is unaffected.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_dac_wizard_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        /// The whole wizard state (see `dac_wizard_qt::WizardDoc`). Parseable
        /// default so QML's `JSON.parse` never throws on the pre-publish frame.
        #[qproperty(QString, wizard_json)]
        type QbzDacWizard = super::QbzDacWizardRust;

        /// Registers this object's Qt-thread hop. Without it every publish
        /// from Rust is dropped on the floor, silently and forever: the modal
        /// mounts, no document ever arrives, and nothing is logged on either
        /// side.
        #[qinvokable]
        fn boot(self: Pin<&mut QbzDacWizard>);

        /// Settings > Audio > "Open Wizard": reset + seed the dropdowns, then
        /// probe the audio stack off the UI thread.
        #[qinvokable]
        fn open(self: Pin<&mut QbzDacWizard>);

        /// Scrim / header X / footer Close. Closing NEVER stops playback — the
        /// test step deliberately keeps playing underneath (reference §modal
        /// recipe: an overlay, not a window grab).
        #[qinvokable]
        fn close(self: Pin<&mut QbzDacWizard>);

        /// Check step: the user overrode the distribution (package manager).
        #[qinvokable]
        fn set_distro(self: Pin<&mut QbzDacWizard>, index: i32);

        /// Check step: the user overrode the init system (service commands).
        #[qinvokable]
        fn set_init(self: Pin<&mut QbzDacWizard>, index: i32);

        /// Entering the DACs step — enumerate sinks off the UI thread.
        #[qinvokable]
        fn run_detect(self: Pin<&mut QbzDacWizard>);

        /// Flip one enumerated candidate's checkbox.
        #[qinvokable]
        fn toggle_dac(self: Pin<&mut QbzDacWizard>, index: i32);

        /// Validate a pasted `node.name` (the manual escape hatch).
        #[qinvokable]
        fn validate_manual(self: Pin<&mut QbzDacWizard>, text: QString);

        /// Entering the review step — generate the per-DAC config snippets.
        #[qinvokable]
        fn gen_configs(self: Pin<&mut QbzDacWizard>);

        /// Collapse/expand one DAC's generated-config accordion.
        #[qinvokable]
        fn toggle_config(self: Pin<&mut QbzDacWizard>, index: i32);

        /// Test step: resolve the four curated tracks and play them.
        #[qinvokable]
        fn start_test(self: Pin<&mut QbzDacWizard>);

        /// Test step: pause and drop the "playing" state.
        #[qinvokable]
        fn stop_test(self: Pin<&mut QbzDacWizard>);

        /// Test step: jump straight to one of the four tracks.
        #[qinvokable]
        fn test_play_index(self: Pin<&mut QbzDacWizard>, index: i32);

        /// Test step: run the read-back against the user's OWN queue.
        #[qinvokable]
        fn verify_own(self: Pin<&mut QbzDacWizard>);

        /// One read-back tick (requested vs negotiated). Driven by the modal's
        /// 1.5 s Timer while the test plays.
        #[qinvokable]
        fn poll_test(self: Pin<&mut QbzDacWizard>);
    }

    impl cxx_qt::Threading for QbzDacWizard {}
}

use qbz_dac_wizard_bridge::QbzDacWizard;

/// Rust side of the wizard bridge (plain storage; the real state lives in
/// `dac_wizard_qt`).
pub struct QbzDacWizardRust {
    wizard_json: QString,
}

impl Default for QbzDacWizardRust {
    fn default() -> Self {
        Self {
            wizard_json: QString::from(&crate::dac_wizard_qt::initial_json()),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzDacWizard>> = OnceLock::new();

/// Queue a wizard-document publish onto the Qt event loop (no-op before
/// `boot()` registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzDacWizard>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_dac_wizard_bridge::QbzDacWizard {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] dac wizard Qt thread already registered");
        }
    }

    pub fn open(self: Pin<&mut Self>) {
        crate::dac_wizard_qt::open();
    }

    pub fn close(self: Pin<&mut Self>) {
        crate::dac_wizard_qt::close();
    }

    pub fn set_distro(self: Pin<&mut Self>, index: i32) {
        crate::dac_wizard_qt::set_distro(index);
    }

    pub fn set_init(self: Pin<&mut Self>, index: i32) {
        crate::dac_wizard_qt::set_init(index);
    }

    pub fn run_detect(self: Pin<&mut Self>) {
        crate::dac_wizard_qt::run_detect();
    }

    pub fn toggle_dac(self: Pin<&mut Self>, index: i32) {
        crate::dac_wizard_qt::toggle_dac(index);
    }

    pub fn validate_manual(self: Pin<&mut Self>, text: QString) {
        crate::dac_wizard_qt::validate_manual(&text.to_string());
    }

    pub fn gen_configs(self: Pin<&mut Self>) {
        crate::dac_wizard_qt::gen_configs();
    }

    pub fn toggle_config(self: Pin<&mut Self>, index: i32) {
        crate::dac_wizard_qt::toggle_config(index);
    }

    pub fn start_test(self: Pin<&mut Self>) {
        crate::dac_wizard_qt::start_test();
    }

    pub fn stop_test(self: Pin<&mut Self>) {
        crate::dac_wizard_qt::stop_test();
    }

    pub fn test_play_index(self: Pin<&mut Self>, index: i32) {
        crate::dac_wizard_qt::test_play_index(index);
    }

    pub fn verify_own(self: Pin<&mut Self>) {
        crate::dac_wizard_qt::verify_own();
    }

    pub fn poll_test(self: Pin<&mut Self>) {
        crate::dac_wizard_qt::poll_test();
    }
}
