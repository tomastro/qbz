//! QbzShaderScene — immersive shader-scenes domain bridge (viz_bridge.rs
//! exemplar: OnceLock<CxxQtThread>, `ui()` no-op pre-boot, `boot()`
//! invokable, `impl Default` for construction-seeded values).
//!
//! Block A1 of the immersive-completion contract
//! (`qbz-nix-docs/qt-frontend/2026-08-15-immersive-completion/00-CONTRACT.md`
//! §5, spec `01-shader-scenes-port.md` §1/§4). The Qt counterpart of the
//! Slint `ImmersiveState.shader-mode` + `shader-scenes-available` pair
//! (`ImmersiveView.slint:1293-1405`, menu rows `:654-711`, `g` ring
//! `:1245-1256`).
//!
//! NAME CHECK (spec 01 §4): `src/scene_bridge.rs` is TAKEN by the ArtistScene
//! domain ("artists from the same place", `QbzScene`) — hence
//! `shader_scene_bridge.rs` / `QbzShaderScene`.
//!
//! Scene numbering follows the Slint modes verbatim
//! (`shader_underlay.rs:330-337`): 0 Off, 1 Plasma (shipped by A2 — the
//! feedback `QQuickRhiItem` in cxx/plasma_item.cpp), 2 Tunnel, 3 Aurora,
//! 4 Spectral Ribbon (shipped by A3 — the spectrogram `QQuickRhiItem` in
//! cxx/ribbon_item.cpp fed by `ribbon_qt.rs`),
//! 5 Line Bed (shipped by A4 — the `QQuickRhiItem` in cxx/linebed_item.cpp
//! fed by `linebed_qt.rs`), 6 Liquid Spectrum (PARKED in Slint too), 7
//! Ambient. B1 adds 8 Tunnel Flow — a QT-ONLY scene (the Tauri Canvas2D
//! tunnel ported to the feedback `QQuickRhiItem` in cxx/tunnelflow_item.cpp,
//! spec 02-tauri-tunnel-port.md), menu-only like Ambient-in-Slint: it is NOT
//! in the `g` ring (spec 02 §5). The shipped set is now 1/2/3/4/5/7/8, so
//! the `g` ring is 0→1→2→3→4→5→7→0 (the Slint ring is mod-6 over modes 0..5;
//! the parked scenes are unreachable here and their menu rows DO NOT appear —
//! per the contract's cheap→hard block cut). Ambient (7) is reachable from
//! the ring here; in Slint it is menu-only. That is the one deliberate
//! divergence and it is logged in the block report.
//!
//! A4 (Line Bed): the ready 256x200 f32 depth ring publishes ONCE per viz
//! tick as `linebed_heights` (a QByteArray — ONE notify per tick, the pack's
//! batching pattern; the 512-float Spectral512 stream itself is deliberately
//! never marshalled to QML). The publish is gated on `scene == 5` via the
//! `set_linebed_active` mirror (QML reports the scene edge; the drain reads
//! the atomic — 200 KB at 30 Hz for a scene nobody is looking at is what
//! the pulse law forbids). boot() also forces the C++ item's QML type
//! registration — the `extern "C"` call is what pulls the linebed object
//! file out of the static lib at link time (the registration itself already
//! ran earlier, at QGuiApplication construction, via
//! Q_COREAPP_STARTUP_FUNC; the boot call is the link anchor and a guarded
//! no-op fallback).
//!
//! The audio pack (`pack_json`) is ONE batched update per viz drain tick —
//! a single QString property, not a dozen individual qproperties: every
//! property notify dirties its bindings, and 20 notifies at 30 Hz is 20x the
//! binding churn for the same picture (spec 01 §3). The QML side
//! (ShaderSceneLayer.qml) stashes the parsed document on publish and APPLIES
//! it on the shared shell pulse (`QbzShell.pulseMs`) — the VizSettle pattern —
//! so the scene's uniforms move in the same event-loop turn as every other
//! animator and the window presents ONCE per pulse period (THE PULSE LAW,
//! 00-CONTRACT §3).
//!
//! The palette is NOT part of the pack: it already lives on
//! `QbzShell.ambientPrimary/Secondary/Accent` (shell_bridge.rs, published on
//! track change by playback_qt.rs from the same `ambient_qt` source the
//! Slint side feeds `shader_underlay::set_palette` with). The QML layer binds
//! those three directly (spec 01 §1 — no new Rust for palette).
//!
//! The tier gate is NOT a property of this bridge: it already exists as
//! `QbzShell.shaderScenesAvailable` (shell_bridge.rs, seeded from the Rust
//! renderer probe `renderer_qt::gpu_tier()` — documented there as "the gate
//! the immersive shader scenes were waiting on"). That is the ONE source of
//! truth the FOCUS menu rows and the `g` handler read, exactly like Slint
//! seeds `shader-scenes-available` from the wgpu tier (spec 01 section 4).
//! The shader-LOAD half of the gate (a ShaderEffect reporting Error on a
//! GPU-tier box) lives in ShaderSceneLayer.qml, which owns the effects: on
//! failure it latches itself off and resets `scene` to Off, handing the
//! background back to the atmosphere — but it never feeds the picker flag.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::{QByteArray, QString};

/// Full-shape default for the audio pack — every field present at zero, so
/// the QML layer can parse it before the first real publish without a guard.
const PACK_EMPTY: &str = "{\"phase\":0,\"beat\":0,\"level\":0,\"levelSmooth\":0,\
\"transient\":0,\"energyLo\":[0,0,0,0],\"energyHi\":[0,0,0,0],\
\"bandsLo\":[0,0,0,0],\"bandsHi\":[0,0,0,0]}";

/// B1: the Tunnel Flow palette default — the Tauri fallback
/// (`DEFAULT_LINE_PALETTE`, TunnelFlowPanel.svelte:63-68), so the scene has
/// a valid palette before the first artwork extraction lands.
const TUNNEL_PALETTE_DEFAULT: &str = "[\"#ff6a6a\",\"#ffcd5c\",\"#68dcaa\",\"#6eb0ff\"]";

#[cxx_qt::bridge]
pub mod qbz_shader_scene {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qbytearray.h");
        type QByteArray = cxx_qt_lib::QByteArray;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // The active scene (Slint `shader-mode`): 0 Off, 1 Plasma (A2),
        // 2 Tunnel, 3 Aurora, 4 Spectral Ribbon (A3), 5 Line Bed (A4), 6
        // parked, 7 Ambient, 8 Tunnel Flow (B1). Written from QML (menu
        // rows), from Rust (cycle_scene) and by the immersive open funnel.
        //
        // PERSISTED SINCE 2026-08-19, a deliberate divergence from Slint
        // (which keeps shader-mode session-only and force-resets it to Off on
        // every open). "Remember the last immersive view" covered the
        // view_mode / mode / split_panel triple and NOT the scene, so the
        // half of the immersive surface a user is most likely to leave it on —
        // Plasma, the Ribbon, the Line Bed, Tunnel Flow — was the half that
        // never came back. Owner call: the setting says "last view", and a
        // scene IS one of the views the menu offers.
        //
        // Every write funnels through `set_scene`, which persists unless the
        // RESTORING latch is up (the restore itself must not re-persist, the
        // §3.1 rule the triple already follows).
        #[qproperty(i32, scene, READ, WRITE = set_scene, NOTIFY)]
        // The batched audio pack (the §1 uniform contract minus time/
        // resolution/palette), one JSON publish per viz drain tick. READ +
        // NOTIFY only — QML never writes it.
        #[qproperty(QString, pack_json, READ, NOTIFY)]
        // A4: the Line Bed depth ring, 256x200 f32 (row 0 = newest) as raw
        // bytes — one publish per viz tick while scene == 5. READ + NOTIFY
        // only; the C++ item's QML wrapper (LineBedScene.qml) applies it on
        // the pulse edge, never on this notify (the VizSettle pattern).
        #[qproperty(QByteArray, linebed_heights, READ, NOTIFY)]
        // A3: the Spectral Ribbon frame — [col u32 LE][reset u8][512-byte
        // row], one publish per viz tick while scene == 4 (layout pinned
        // with ribbon_qt.rs and cxx/ribbon_item.cpp). READ + NOTIFY only;
        // RibbonScene.qml applies it on the pulse edge.
        #[qproperty(QByteArray, ribbon_frame, READ, NOTIFY)]
        // B1: the Tunnel Flow palette (scene 8) — four hex colors as a JSON
        // array (`["#ff6a6a",...]`, the tunnelflow_qt.rs port of the Tauri
        // extractLinePaletteFromArtwork), ONE batched publish per track
        // change from ambient_qt::update_for_artwork. READ + NOTIFY only;
        // TunnelFlowScene.qml stashes the parse and applies it on the pulse
        // edge.
        #[qproperty(QString, tunnel_palette_json, READ, NOTIFY)]
        type QbzShaderScene = super::QbzShaderSceneRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY
        /// domain singleton; only QbzSession.boot also fires crate::on_boot).
        /// Also anchors the Line Bed item's QML type registration (see the
        /// module header).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzShaderScene>);

        /// The `g` ring over the shipped scenes: 0→1→2→3→4→5→7→0 (Off →
        /// Plasma → Tunnel → Aurora → Spectral Ribbon → Line Bed → Ambient
        /// → Off). Anything outside the
        /// shipped set (a parked mode number that could never be selected)
        /// falls back to Off. The availability gate lives at the call site
        /// (the QML key handler), like Slint's `if shader-scenes-available`
        /// guard.
        #[qinvokable]
        fn cycle_scene(self: Pin<&mut QbzShaderScene>);

        /// A4 publish gate: ShaderSceneLayer reports scene==5 edges here so
        /// the viz drain only pushes/publishes the 200 KB depth ring while
        /// the scene is actually on screen (the pulse law). Writes an
        /// atomic the drain reads; never touches the object itself.
        #[qinvokable]
        fn set_linebed_active(self: Pin<&mut QbzShaderScene>, active: bool);

        /// A3 publish gate, same shape as set_linebed_active: scene==4
        /// edges gate the 517-byte ribbon frame publish.
        #[qinvokable]
        fn set_ribbon_active(self: Pin<&mut QbzShaderScene>, active: bool);
    }

    // The custom WRITE target of the `scene` property. NOT a qinvokable — QML
    // reaches it by writing `QbzShaderScene.scene`, never by name. (cxx-qt
    // custom-setter pattern, the immersive_bridge `set_open` precedent: the
    // property's auto setter is replaced by this method, which owns the store,
    // the notify and the persist.)
    unsafe extern "RustQt" {
        fn set_scene(self: Pin<&mut QbzShaderScene>, value: i32);
    }

    impl cxx_qt::Threading for QbzShaderScene {}
}

use qbz_shader_scene::QbzShaderScene;

/// Rust side of the shader-scene bridge (plain storage, phase-1 pattern).
pub struct QbzShaderSceneRust {
    scene: i32,
    pack_json: QString,
    linebed_heights: QByteArray,
    ribbon_frame: QByteArray,
    tunnel_palette_json: QString,
}

impl Default for QbzShaderSceneRust {
    fn default() -> Self {
        Self {
            scene: 0,
            pack_json: QString::from(PACK_EMPTY),
            linebed_heights: QByteArray::default(),
            ribbon_frame: QByteArray::default(),
            tunnel_palette_json: QString::from(TUNNEL_PALETTE_DEFAULT),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (viz_bridge.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzShaderScene>> = OnceLock::new();

/// Queue a shader-scene-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzShaderScene>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

/// The viz drain's batched pack publish (viz_qt.rs, ~30 Hz while immersive is
/// open and the tap runs). ONE property write = ONE notify per tick. The
/// property is READ+NOTIFY (no WRITE, so no generated setter): store +
/// notify by hand, the apply_open pattern.
pub(crate) fn publish_pack(json: String) {
    ui(move |mut s| {
        use cxx_qt::CxxQtType as _;
        s.as_mut().rust_mut().pack_json = QString::from(&json);
        s.as_mut().pack_json_changed();
    });
}

// ---------------------------------------------------------------------------
// A4: the Line Bed depth ring (scene 5)
// ---------------------------------------------------------------------------

/// True while the Line Bed scene is on screen. Mirrored from QML
/// (`set_linebed_active`, fired on scene edges by ShaderSceneLayer) because
/// the viz drain thread cannot read the QObject property: this atomic is
/// what gates the 200 KB ring publish (the pulse law — an invisible scene
/// writes nothing).
static LINEBED_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The drain-side read of the gate (viz_qt.rs).
pub(crate) fn linebed_active() -> bool {
    LINEBED_ACTIVE.load(Ordering::Relaxed)
}

/// One ring publish per viz tick (the pack's batching pattern): store the
/// QByteArray + ONE notify. The QML side stashes and applies on the pulse
/// edge, so this notify never dirties the scene by itself.
pub(crate) fn publish_linebed(bytes: &[u8]) {
    let buf = QByteArray::from(bytes);
    ui(move |mut s| {
        use cxx_qt::CxxQtType as _;
        s.as_mut().rust_mut().linebed_heights = buf;
        s.as_mut().linebed_heights_changed();
    });
}

// ---------------------------------------------------------------------------
// A3: the Spectral Ribbon frame (scene 4)
// ---------------------------------------------------------------------------

/// True while the Spectral Ribbon scene is on screen — the linebed gate's
/// twin (the viz drain thread cannot read the QObject property, so QML
/// mirrors the scene edge into this atomic; an invisible scene publishes
/// nothing — the pulse law).
static RIBBON_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The drain-side read of the gate (viz_qt.rs).
pub(crate) fn ribbon_active() -> bool {
    RIBBON_ACTIVE.load(Ordering::Relaxed)
}

/// One frame publish per viz tick (the pack's batching pattern): store the
/// QByteArray + ONE notify. The QML side stashes and applies on the pulse
/// edge, so this notify never dirties the scene by itself.
pub(crate) fn publish_ribbon(bytes: Vec<u8>) {
    let buf = QByteArray::from(bytes.as_slice());
    ui(move |mut s| {
        use cxx_qt::CxxQtType as _;
        s.as_mut().rust_mut().ribbon_frame = buf;
        s.as_mut().ribbon_frame_changed();
    });
}

// ---------------------------------------------------------------------------
// B1: the Tunnel Flow palette (scene 8)
// ---------------------------------------------------------------------------

/// One batched palette publish per track change (ambient_qt::
/// update_for_artwork, which already opens the cached cover): store the JSON
/// array + ONE notify. Track-change cadence, not per-tick — no scene gate
/// needed (the pulse law is about the 30 Hz streams).
pub(crate) fn publish_tunnel_palette(json: String) {
    ui(move |mut s| {
        use cxx_qt::CxxQtType as _;
        s.as_mut().rust_mut().tunnel_palette_json = QString::from(&json);
        s.as_mut().tunnel_palette_json_changed();
    });
}

/// The pref key holding the last scene. Lives in the shared `ui_prefs.json`
/// alongside `immersive_last_view_mode` / `_mode` / `_split_panel`, as a JSON
/// NUMBER for the same reason those three are: the file is co-owned with the
/// Slint app, whose typed struct declares its own immersive keys i32. This one
/// is NEW and therefore unknown to that struct — serde ignores it on read, and
/// the worst case if the Slint build ever rewrites the document is that the key
/// is dropped and this falls back to Off. Acceptable: `qbz-ui` is on its way
/// out, and the failure mode is "the scene is not remembered", not a poisoned
/// document.
const SCENE_PREF: &str = "immersive_last_shader_scene";

/// Up while the open funnel is writing the REMEMBERED scene back. `set_scene`
/// checks it so the restore does not re-persist what it just read (§3.1: the
/// restore writes go to the property, never through the persisting path).
static RESTORING: AtomicBool = AtomicBool::new(false);

/// Whether a scene number is one this build can actually show. Parked modes
/// (6) and junk must never be restored: the layer would mount nothing and the
/// menu would show no row active, which reads as a broken immersive.
fn is_shipped(scene: i32) -> bool {
    matches!(scene, 1 | 2 | 3 | 4 | 5 | 7 | 8)
}

/// Immersive open: apply the scene this session should start on.
///
/// Slint force-resets to Off here (`main.rs:10300-10301`) because it never
/// remembered the scene at all. This port does, so the reset is now the
/// FALLBACK arm and the remembered scene is the default one:
///
/// - `immersive_default_view == "remember"` and the stored scene is a shipped
///   one and the GPU tier allows shaders -> restore it.
/// - anything else (a PINNED default view, a parked/absent scene, a box below
///   the shader tier) -> Off, exactly as before.
///
/// The tier gate is not optional: `renderer_qt::gpu_tier()` is the same probe
/// behind `QbzShell.shaderScenesAvailable`, and off-tier the scenes render
/// BLACK. Restoring one there would open immersive onto a black rectangle with
/// no menu row to explain it.
///
/// Called from the immersive open funnel (immersive_bridge.rs, apply_open true
/// edge). No-op when the target already matches, so the common path emits no
/// notify.
pub(crate) fn apply_on_immersive_open() {
    let target = remembered_scene();
    crate::viz_qt::set_immersive_scene_active(target != 0);
    ui(move |mut s| {
        if s.scene == target {
            return;
        }
        RESTORING.store(true, Ordering::SeqCst);
        s.as_mut().set_scene(target);
        RESTORING.store(false, Ordering::SeqCst);
    });
}

/// The scene an open edge should land on — pure, so the unit test can pin the
/// gating without a QObject or a prefs file.
fn resolve_open_scene(default_view: &str, stored: i32, tier_ok: bool) -> i32 {
    if default_view != "remember" || !tier_ok || !is_shipped(stored) {
        return 0;
    }
    stored
}

fn remembered_scene() -> i32 {
    resolve_open_scene(
        &crate::settings_qt::pref_str("immersive_default_view", "remember"),
        crate::settings_qt::pref_i32(SCENE_PREF, 0),
        crate::renderer_qt::gpu_tier(),
    )
}

/// The `g` ring map, free-standing so the unit test can pin it without a
/// QObject: 0→1→2→3→4→5→7→0, everything else → 0.
fn ring_next(scene: i32) -> i32 {
    match scene {
        0 => 1, // Off -> Plasma (A2)
        1 => 2, // Plasma -> Tunnel
        2 => 3, // Tunnel -> Aurora
        3 => 4, // Aurora -> Spectral Ribbon (A3)
        4 => 5, // Spectral Ribbon -> Line Bed (A4)
        5 => 7, // Line Bed -> Ambient
        _ => 0, // Ambient (and anything unexpected) -> Off
    }
}

// ---------------------------------------------------------------------------
// A2: the Plasma item registration anchor
// ---------------------------------------------------------------------------

extern "C" {
    /// Defined in `cxx/plasma_item.cpp`. Registers `PlasmaItem` with the QML
    /// type system (idempotent — guarded C++ side).
    fn qbz_plasma_register_qml_type();
    /// Defined in `cxx/tunnelflow_item.cpp`. Registers `TunnelFlowItem` with
    /// the QML type system (idempotent — guarded C++ side).
    fn qbz_tunnelflow_register_qml_type();
}

/// Call the C++ registration — the linebed_qt.rs idiom: the C++ lives in a
/// STATIC LIB, so the linker only pulls its object file to resolve an
/// undefined symbol; the call from `boot` is that reference (the
/// registration itself already ran at QGuiApplication construction via
/// Q_COREAPP_STARTUP_FUNC).
pub(crate) fn register_plasma_qml_item() {
    // SAFETY: no arguments, touches only Qt's global type registry (we are
    // on the GUI thread — boot() runs from QML), idempotent.
    unsafe { qbz_plasma_register_qml_type() };
}

/// B1: the same static-lib link anchor for the Tunnel Flow item
/// (cxx/tunnelflow_item.cpp).
pub(crate) fn register_tunnelflow_qml_item() {
    // SAFETY: no arguments, touches only Qt's global type registry (we are
    // on the GUI thread — boot() runs from QML), idempotent.
    unsafe { qbz_tunnelflow_register_qml_type() };
}

impl qbz_shader_scene::QbzShaderScene {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] shader-scene Qt thread already registered");
        }
        // A4: anchor the Line Bed C++ object file into the link (see the
        // module header) and double-check the QML type registration. The
        // registration itself already ran at QGuiApplication construction
        // (Q_COREAPP_STARTUP_FUNC in cxx/linebed_item.cpp, BEFORE the
        // engine loads any QML); this call is guarded and idempotent.
        crate::viz_qt::linebed_qt::register_qml_item();
        // A2: same static-lib link anchor for the Plasma item
        // (cxx/plasma_item.cpp).
        register_plasma_qml_item();
        // A3: same anchor for the Spectral Ribbon item
        // (cxx/ribbon_item.cpp).
        crate::viz_qt::ribbon_qt::register_qml_item();
        // B1: same anchor for the Tunnel Flow item
        // (cxx/tunnelflow_item.cpp).
        register_tunnelflow_qml_item();
    }

    /// The `g` ring over the shipped scenes (0→2→3→5→7→0 — A4 added Line
    /// Bed between Aurora and Ambient, the Slint menu order). Anything else
    /// (parked modes 1/4/6, out-of-range junk) snaps to Off — the ring is
    /// defined over what EXISTS, not over the Slint mode space.
    pub fn cycle_scene(mut self: Pin<&mut Self>) {
        let next = ring_next(self.scene);
        self.as_mut().set_scene(next);
    }

    /// The custom WRITE target of `scene` — the funnel EVERY write passes
    /// through: the QML menu rows (`QbzShaderScene.scene = N`), the shader-
    /// error latch in ShaderSceneLayer.qml, `cycle_scene` above and the open
    /// funnel's restore. It stores, notifies, and persists.
    ///
    /// The persist is skipped while RESTORING is up, and it is also skipped
    /// when the default-view pref is PINNED — the triple's own rule
    /// (`immersive_bridge::set_view`): a pinned default wins on the next open,
    /// so writing under it would store a value nothing will ever read while
    /// silently overwriting the user's real last scene.
    ///
    /// The shader-error latch persisting Off is intended, not collateral: a
    /// scene whose effect failed to compile on this box must not be the one
    /// the next open restores.
    pub fn set_scene(mut self: Pin<&mut Self>, value: i32) {
        use cxx_qt::CxxQtType as _;
        if self.scene == value {
            return;
        }
        self.as_mut().rust_mut().scene = value;
        self.as_mut().scene_changed();
        crate::viz_qt::set_immersive_scene_active(value != 0);
        if RESTORING.load(Ordering::SeqCst) {
            return;
        }
        if crate::settings_qt::pref_str("immersive_default_view", "remember") == "remember" {
            crate::settings_qt::save_pref(SCENE_PREF, serde_json::Value::Number(value.into()));
        }
    }

    /// The A4 publish gate (see LINEBED_ACTIVE). Runs on the Qt thread but
    /// touches only the atomic.
    pub fn set_linebed_active(self: Pin<&mut Self>, active: bool) {
        LINEBED_ACTIVE.store(active, Ordering::Relaxed);
    }

    /// The A3 publish gate (see RIBBON_ACTIVE). Runs on the Qt thread but
    /// touches only the atomic.
    pub fn set_ribbon_active(self: Pin<&mut Self>, active: bool) {
        RIBBON_ACTIVE.store(active, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pack default is full-shape JSON: every field the QML layer reads
    /// is present (and zero) before the first viz publish.
    #[test]
    fn pack_default_is_full_shape() {
        let doc: serde_json::Value = serde_json::from_str(PACK_EMPTY).unwrap();
        for key in ["phase", "beat", "level", "levelSmooth", "transient"] {
            assert!(doc.get(key).is_some(), "missing {key}");
        }
        for key in ["energyLo", "energyHi", "bandsLo", "bandsHi"] {
            assert!(
                doc.get(key)
                    .and_then(|v| v.as_array())
                    .is_some_and(|a| a.len() == 4),
                "missing/short {key}"
            );
        }
    }

    /// The open edge restores the remembered scene only when all three gates
    /// agree: the default-view pref is "remember", the box is on the shader
    /// tier, and the stored number is a scene this build ships.
    #[test]
    fn open_scene_gates() {
        assert_eq!(resolve_open_scene("remember", 5, true), 5);
        assert_eq!(resolve_open_scene("remember", 8, true), 8);
        // A pinned default view wins over the remembered scene.
        assert_eq!(resolve_open_scene("lyrics", 5, true), 0);
        // Below the shader tier the scenes render black — never restore one.
        assert_eq!(resolve_open_scene("remember", 5, false), 0);
        // Parked (6) and junk never come back.
        assert_eq!(resolve_open_scene("remember", 6, true), 0);
        assert_eq!(resolve_open_scene("remember", 42, true), 0);
        assert_eq!(resolve_open_scene("remember", -1, true), 0);
        // Off stays off.
        assert_eq!(resolve_open_scene("remember", 0, true), 0);
    }

    /// The `g` ring over the shipped scenes: 0→1→2→3→4→5→7→0 (A2 added
    /// Plasma, A3 the Ribbon, A4 Line Bed); parked modes and junk snap to
    /// Off.
    #[test]
    fn g_ring_visits_the_shipped_scenes() {
        assert_eq!(ring_next(0), 1);
        assert_eq!(ring_next(1), 2);
        assert_eq!(ring_next(2), 3);
        assert_eq!(ring_next(3), 4);
        assert_eq!(ring_next(4), 5);
        assert_eq!(ring_next(5), 7);
        assert_eq!(ring_next(7), 0);
        for junk in [6, -1, 42] {
            assert_eq!(ring_next(junk), 0, "junk mode {junk} must snap to Off");
        }
    }
}
