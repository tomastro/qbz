//! QbzCast — Cast (Chromecast / DLNA) domain bridge. The Qt surface of the
//! Slint `CastState` global + its `CastActions` callbacks (state.slint:4714 and
//! :4727); the service behind it is `src/cast_qt.rs` over the shared
//! `qbz-cast` crate.
//!
//! Pattern, verbatim from `src/home_bridge.rs`: ONE `#[cxx_qt::bridge]`
//! module, a `#[qml_element] #[qml_singleton]` QObject, its own
//! `OnceLock<CxxQtThread>`, a `boot()` invokable that registers the Qt-thread
//! hop and a `pub(crate) fn ui()` that queues mutations onto it. The file
//! lives FLAT in `src/` (QTBUG-93443 — a bridge in a subdirectory does not
//! register) and the module is `qbz_cast_bridge`, NOT `qbz_cast`: a bridge
//! module named after a workspace crate collides with that crate's own name in
//! the generated C++/Rust scaffolding.
//!
//! DEVICE LIST SHAPE: the discovered renderers travel as ONE JSON document
//! (`devicesJson`), the same reason `home_bridge` gives — cxx-qt-lib 0.7.3 has
//! no `QVariantValue` impl for `QMap`/`QList`, so a QVariantList of
//! QVariantMaps is not expressible. QML `JSON.parse()`s it into the exact
//! `CastDevice` record the Slint struct declares:
//!   { id, name, ip, protocol: "chromecast"|"dlna", model,
//!     canPlay, canSetVolume }
//! `deviceCapOptions` is likewise a JSON array of strings (the cap dropdown's
//! labels, pushed from Rust because option 0 embeds the live global
//! streaming-quality label and Rust-pushed models never re-translate on their
//! own).
//!
//! NAMING: the connect/disconnect invokables are `connectDevice` /
//! `disconnectDevice`, not `connect` / `disconnect` — a non-static member of
//! either name on a QObject subclass shadows `QObject::connect` /
//! `QObject::disconnect`.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_cast_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // --- Picker ------------------------------------------------------
        // The picker modal's own visibility. Rust owns it (openPicker /
        // closePicker set it) so the discovery lifecycle and the visible
        // state can never disagree.
        #[qproperty(bool, picker_open)]
        // "chromecast" | "dlna" — which tab's list is shown.
        #[qproperty(QString, selected_tab)]
        // 15 s scan-spinner window (armed by openPicker / refresh).
        #[qproperty(bool, scanning)]
        // JSON array of CastDevice records (see the module header).
        #[qproperty(QString, devices_json)]
        #[qproperty(i32, chromecast_count)]
        #[qproperty(i32, dlna_count)]
        // --- Connection --------------------------------------------------
        #[qproperty(bool, connected)]
        // "chromecast" | "dlna" | "".
        #[qproperty(QString, protocol)]
        // Connected device label.
        #[qproperty(QString, device_name)]
        // --- Transport mirror (cast owns transport while connected) ------
        #[qproperty(bool, is_playing)]
        #[qproperty(f32, position_secs)]
        #[qproperty(f32, duration_secs)]
        // --- Delivered quality (#638 fix 1) ------------------------------
        // MEASURED from the served bytes when probeable, catalog fallback
        // otherwise. Coarse label ("Hi-Res FLAC") + precise detail
        // ("24-bit / 192 kHz").
        #[qproperty(QString, quality_label)]
        #[qproperty(QString, quality_detail)]
        // qbz_models::QualityLimit discriminant: 0 none · 1 preference ·
        // 3 renderer cap · 4 availability. 2 (local device cap) NEVER
        // applies while casting — the local DAC is not in a cast's signal
        // path (precedence rule).
        #[qproperty(i32, quality_limit_cause)]
        // The served bytes EXCEED the requested tier (a locally-existing
        // cache/offline copy under a lower cap) — sent as-is, never
        // resampled; the picker discloses it instead of hiding it.
        #[qproperty(bool, quality_over_cap)]
        // "cache" | "offline" | "" (network).
        #[qproperty(QString, quality_origin)]
        // --- Per-renderer manual quality cap (#638 fix 4) ----------------
        // Offered only when the renderer has a STABLE identity (DLNA UDN
        // always; Chromecast only when the mDNS TXT `id` record exists — a
        // fullname-keyed cap would silently detach on rename).
        #[qproperty(bool, device_cap_available)]
        // "dlna:<UDN>" | "chromecast:<id>" | "".
        #[qproperty(QString, device_cap_key)]
        // JSON array of option labels; 0 follow · 1 hires · 2 cd · 3 mp3.
        #[qproperty(QString, device_cap_options)]
        #[qproperty(i32, device_cap_index)]
        // Live Settings tier label, e.g. "Hi-Res+".
        #[qproperty(QString, global_quality_label)]
        // --- Errors ------------------------------------------------------
        // Raw last-error for the picker's error line (empty = none), same
        // as the Slint `CastState.error`.
        #[qproperty(QString, error)]
        type QbzCast = super::QbzCastRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY
        /// domain singleton; only QbzSession.boot also fires crate::on_boot)
        /// and seeds the quality-cap dropdown labels.
        #[qinvokable]
        fn boot(self: Pin<&mut QbzCast>);

        /// Open the picker AND start discovery. This is the ONLY thing that
        /// arms mDNS/SSDP — nothing scans in the background.
        #[qinvokable]
        fn open_picker(self: Pin<&mut QbzCast>);

        /// Close the picker and stop both discoveries. An active cast
        /// session survives (only the scanning stops).
        #[qinvokable]
        fn close_picker(self: Pin<&mut QbzCast>);

        /// "Search again" — re-arm the scan window + the refresh loop.
        #[qinvokable]
        fn refresh(self: Pin<&mut QbzCast>);

        /// Which tab the device list shows ("chromecast" | "dlna").
        #[qinvokable]
        fn set_tab(self: Pin<&mut QbzCast>, tab: QString);

        /// Connect to a discovered renderer (device id + its protocol).
        #[qinvokable]
        fn connect_device(self: Pin<&mut QbzCast>, device_id: QString, protocol: QString);

        /// Stop the renderer and drop the session.
        #[qinvokable]
        fn disconnect_device(self: Pin<&mut QbzCast>);

        /// Persist the manual quality cap for the connected renderer:
        /// cap key + option index (0 = follow the app setting).
        #[qinvokable]
        fn set_device_quality_cap(self: Pin<&mut QbzCast>, cap_key: QString, index: i32);
    }

    impl cxx_qt::Threading for QbzCast {}
}

use qbz_cast_bridge::QbzCast;

/// Rust side of the cast bridge (plain storage, phase-1 pattern).
pub struct QbzCastRust {
    picker_open: bool,
    selected_tab: QString,
    scanning: bool,
    devices_json: QString,
    chromecast_count: i32,
    dlna_count: i32,
    connected: bool,
    protocol: QString,
    device_name: QString,
    is_playing: bool,
    position_secs: f32,
    duration_secs: f32,
    quality_label: QString,
    quality_detail: QString,
    quality_limit_cause: i32,
    quality_over_cap: bool,
    quality_origin: QString,
    device_cap_available: bool,
    device_cap_key: QString,
    device_cap_options: QString,
    device_cap_index: i32,
    global_quality_label: QString,
    error: QString,
}

impl Default for QbzCastRust {
    fn default() -> Self {
        Self {
            picker_open: false,
            // The Slint default tab.
            selected_tab: QString::from("chromecast"),
            scanning: false,
            // "[]" / "{}" so the picker's JSON.parse never throws on the
            // first frame (the home_bridge convention).
            devices_json: QString::from("[]"),
            chromecast_count: 0,
            dlna_count: 0,
            connected: false,
            protocol: QString::default(),
            device_name: QString::default(),
            is_playing: false,
            position_secs: 0.0,
            duration_secs: 0.0,
            quality_label: QString::default(),
            quality_detail: QString::default(),
            quality_limit_cause: 0,
            quality_over_cap: false,
            quality_origin: QString::default(),
            device_cap_available: false,
            device_cap_key: QString::default(),
            device_cap_options: QString::from("[]"),
            device_cap_index: 0,
            global_quality_label: QString::default(),
            error: QString::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzCast>> = OnceLock::new();

/// Queue a cast-bridge mutation onto the Qt event loop (no-op before boot
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzCast>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_cast_bridge::QbzCast {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] cast Qt thread already registered");
        }
        // Seed the cap dropdown labels + the live global-quality label
        // (queued onto this same thread, so it lands on the next pass).
        crate::cast_qt::push_cap_options();
    }

    pub fn open_picker(mut self: Pin<&mut Self>) {
        self.as_mut().set_picker_open(true);
        // Re-seed the labels: the global streaming-quality setting (embedded
        // in option 0) or the language may have moved since boot.
        crate::cast_qt::push_cap_options();
        crate::cast_qt::open_picker();
    }

    pub fn close_picker(mut self: Pin<&mut Self>) {
        self.as_mut().set_picker_open(false);
        crate::cast_qt::close_picker();
    }

    pub fn refresh(self: Pin<&mut Self>) {
        crate::cast_qt::refresh();
    }

    pub fn set_tab(mut self: Pin<&mut Self>, tab: QString) {
        self.as_mut().set_selected_tab(tab);
    }

    pub fn connect_device(self: Pin<&mut Self>, device_id: QString, protocol: QString) {
        crate::cast_qt::connect(device_id.to_string(), protocol.to_string());
    }

    pub fn disconnect_device(self: Pin<&mut Self>) {
        crate::cast_qt::disconnect();
    }

    pub fn set_device_quality_cap(self: Pin<&mut Self>, cap_key: QString, index: i32) {
        crate::cast_qt::set_device_cap(cap_key.to_string(), index);
    }
}
