//! QbzViz — audio-visualizer domain bridge (phase 23 pattern; see main.rs).
//!
//! Publishes the three FFT streams the Large-NPB dock consumes, as typed
//! `QList<f32>` (NOT a JSON document like the other domains): these refresh
//! at 30 Hz, so serializing/parsing per frame would burn CPU for nothing.
//!
//! PERF CONTRACT for the QML side: each drain tick REPLACES the list, so the
//! property changes identity ~30x/s. Never bind a `Repeater.model` straight
//! to one of these — the delegates would be torn down and rebuilt every
//! frame. Give the Repeater a STATIC model (a column count) and bind only
//! each bar's HEIGHT to `QbzViz.bars[i]`; then a tick re-evaluates 28 height
//! bindings and instantiates nothing. This is the QML equivalent of the
//! Slint side's persistent `VecModel` + `set_row_data` (qbz/src/visualizer.rs).
//! The lengths are FIXED (16 / 5 / 512) so an index binding is always safe.
//!
//! The publish rate is the producer's `TARGET_FPS` (30) and IS the band's
//! frame rate since the 2026-08-13 single-pulse redesign: `VizSettle.qml`
//! stashes each published frame and applies the latest one on the shared
//! shell pulse (`QbzShell.pulseMs`), so the spectrum moves at the data rate
//! and the window presents once per period for every animator at once.
//! Consequences for this side: a late publish degrades to a hold, never a
//! jump, and raising `TARGET_FPS` would buy no smoothness — only CPU —
//! because the Rust EMA already band-limits these streams to ~6 Hz.
//!
//! Props: bars (16 log-spaced FFT bins), energy (5 semantic bands),
//! waveform (256 L + 256 R samples). Invokables: boot + set_enabled.
//!
//! Protected-audio note: everything here is downstream of `qbz-audio`'s
//! read-only ring buffer. The tap starts DISABLED and captures nothing until
//! `set_enabled(true)`; no device/stream init is touched. See CLAUDE.md
//! "Audio backend — PROTECTED".

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;

#[cxx_qt::bridge]
pub mod qbz_viz {
    extern "C++" {
        include!("cxx-qt-lib/qlist.h");
        type QList_f32 = cxx_qt_lib::QList<f32>;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // --- FFT streams (viz_qt.rs drain, ~30 fps while enabled) ---------
        // 16 log-spaced bars. The dock's Bars mode renders 28 MIRRORED
        // columns from the 14 ACTIVE bins (the producer leaves {1, 15}
        // empty) — SpectrumPanel.slint parity.
        #[qproperty(QList_f32, bars)]
        // 5 semantic energy bands: sub-bass, bass, mids, presence, air.
        #[qproperty(QList_f32, energy)]
        // 256 L samples followed by 256 R (VizFrame::Wave256x2). The dock's
        // Waveform mode downsamples the L half (indices 0..235 step 5).
        #[qproperty(QList_f32, waveform)]
        // Packed x/y mid-side points for the goniometer.
        #[qproperty(QList_f32, goniometer)]
        // Pitch-locked mono scope samples.
        #[qproperty(QList_f32, oscilloscope)]
        #[qproperty(f32, stereo_correlation)]
        type QbzViz = super::QbzVizRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY
        /// domain singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzViz>);

        /// Capture gate. Drives the tap's `set_enabled` + the 30 fps drain,
        /// exactly like the Slint `VisualizerState.set-enabled` handler:
        /// OFF means the producer parks and the drain stops, so a hidden or
        /// disabled visualizer costs nothing. The dock calls this on its eye
        /// toggle, on Large enter/leave, and when the window deactivates.
        #[qinvokable]
        fn set_enabled(self: Pin<&mut QbzViz>, on: bool);
    }

    impl cxx_qt::Threading for QbzViz {}
}

use cxx_qt_lib::QList;
use qbz_viz::QbzViz;

type QListF32 = QList<f32>;

/// Fixed stream lengths — the drain overwrites in place and never resizes.
pub(crate) const BARS_LEN: usize = 16;
pub(crate) const ENERGY_LEN: usize = 5;
pub(crate) const WAVEFORM_LEN: usize = 512;
pub(crate) const SCOPE_LEN: usize = 512;

fn zeros(n: usize) -> QListF32 {
    let mut list = QListF32::default();
    for _ in 0..n {
        list.append(0.0);
    }
    list
}

/// Rust side of the visualizer bridge (plain storage, phase-1 pattern).
pub struct QbzVizRust {
    bars: QListF32,
    energy: QListF32,
    waveform: QListF32,
    goniometer: QListF32,
    oscilloscope: QListF32,
    stereo_correlation: f32,
}

impl Default for QbzVizRust {
    fn default() -> Self {
        Self {
            bars: zeros(BARS_LEN),
            energy: zeros(ENERGY_LEN),
            waveform: zeros(WAVEFORM_LEN),
            goniometer: zeros(SCOPE_LEN),
            oscilloscope: zeros(SCOPE_LEN),
            stereo_correlation: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (session_bridge.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzViz>> = OnceLock::new();

/// Queue a visualizer-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzViz>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_viz::QbzViz {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] viz Qt thread already registered");
        }
    }

    pub fn set_enabled(self: Pin<&mut Self>, on: bool) {
        crate::viz_qt::set_enabled(on);
    }
}
