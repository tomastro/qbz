// ShaderSceneLayer — the immersive GPU shader scenes (block A1 of the
// 2026-08-15 immersive-completion contract, spec 01-shader-scenes-port.md).
// The Qt counterpart of the Slint bottom-most `Image` bound to
// `shader-texture` (ImmersiveView.slint:1293-1300): when
// `QbzShaderScene.scene > 0` this layer REPLACES the atmosphere and all the
// FOCUS/SPLIT panels (their `visible` bindings gain `scene === 0` in
// ImmersiveView), while the chrome (header, player bar, song card) stays on
// top.
//
// Scenes shipped: 1 Plasma (A2 — the feedback QQuickRhiItem, its own
// loader below), 2 Tunnel, 3 Aurora, 4 Spectral Ribbon (A3 — the
// spectrogram QQuickRhiItem + axes overlay), 5 Line Bed (A4), 7 Ambient,
// 8 Tunnel Flow (B1 — the Qt-only Tauri port, feedback QQuickRhiItem fed by
// the Viz16 stream, NOT the shader pack) (6 Liquid Spectrum is parked in
// Slint too). Each ShaderEffect scene
// is its own Component, instantiated ONLY
// while active (the Loader): an invisible ShaderEffect that still ticks
// costs presents, so the inactive scenes don't exist at all.
//
// THE PULSE LAW (00-CONTRACT §3 — non-negotiable): every clock here advances
// on the shared shell pulse (QbzShell.pulseMs) via Connections, gated on
// `active` (visible && immersive open && scene > 0 && window showing — the
// AmbientField.qml:44-47 freeze rule). No Timer, no NumberAnimation, no
// FrameAnimation, no Behavior on data-fed properties anywhere in this file.
//
// THE DATA FLOW (spec 01 §1/§3). The viz drain (viz_qt.rs) derives the audio
// pack HOST-SIDE (bands8 pairing, level, level_smooth EMA, beat/transient
// envelopes, phase with the 4096 wrap, spectral_peak EMA) and publishes it as
// ONE batched `QbzShaderScene.packJson` per tick. This layer STASHES the
// parsed document on publish (nothing binds the stash — no scene dirty) and
// APPLIES it to the uniform properties on the pulse edge (the VizSettle
// pattern), so the shader's uniforms move in the same event-loop turn as
// every other animator and the window presents ONCE per pulse period with a
// scene active. `time` is a LOCAL pulse clock (the Slint `time` is
// seconds-since-start; the pack deliberately does not carry it).
//
// THE TIER GATE (spec 01 §4 — ONE source of truth): the picker rows and the
// `g` key read `QbzShell.shaderScenesAvailable`, seeded from the Rust
// renderer probe (renderer_qt::gpu_tier) — the exact analogue of Slint
// seeding `shader-scenes-available` from the wgpu tier. This layer owns only
// the shader-LOAD half: a ShaderEffect reporting Error latches
// `shaderFailed`, un-instantiates the loader and resets the scene to Off, so
// a load failure never blanks the screen (the ImmersiveAtmosphere.qml:123-128
// rule).
//
// RENDER SIZE (shader_underlay.rs:37-39 parity): the effect renders INLINE
// at the item's physical size (the same conclusion AmbientField reached —
// inline is ONE present per pulse, no FBO double-present), and the resX/resY
// uniforms are the item size × DPR CAPPED at 2560×1440 like the reference's
// offscreen target ceiling.

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz

Item {
    id: root

    readonly property int scene: QbzShaderScene.scene

    // --- The tier gate -------------------------------------------------------
    // ONE SOURCE OF TRUTH for the picker/`g` gate: QbzShell.shaderScenesAvailable,
    // seeded from the Rust renderer probe (renderer_qt::gpu_tier — the gate
    // this feature was waiting on, shell_bridge.rs). This layer adds ONLY the
    // shader-LOAD half for its own instantiation safety: a scene effect that
    // reports Error latches `shaderFailed`, which un-instantiates the loader
    // and hands the background back to the atmosphere (a load failure must
    // never blank the screen — the ImmersiveAtmosphere.qml:123-128 rule).
    // The picker flag is NEVER fed from here.
    property bool shaderFailed: false
    readonly property bool available: QbzShell.shaderScenesAvailable
        && !root.shaderFailed

    function onShaderError(name, logText) {
        console.warn("ShaderSceneLayer: " + name + " shader failed:", logText)
        shaderFailed = true
        // Hand the background back to the atmosphere instead of showing black.
        QbzShaderScene.scene = 0
    }

    // --- The freeze gate -----------------------------------------------------
    // True while the scene is actually on screen. Parent visibility does not
    // reach `visible` (that is the LOCAL flag), so the open state is part of
    // the predicate explicitly (ImmersiveView is self-gated on open anyway).
    readonly property bool windowShowing: root.Window.window
        ? (root.Window.window.visibility !== Window.Minimized
           && root.Window.window.visibility !== Window.Hidden)
        : true
    readonly property bool active: root.visible && QbzImmersive.open
        && root.scene > 0 && root.available && root.windowShowing

    // --- Render size (see the header) ---------------------------------------
    readonly property real dpr: root.Screen.devicePixelRatio
    readonly property real resW: Math.min(2560, Math.max(1, Math.round(width * dpr)))
    readonly property real resH: Math.min(1440, Math.max(1, Math.round(height * dpr)))

    // --- Clocks ---------------------------------------------------------------
    // `tickMs` MUST match the pulse period (settings_qt::shell_pulse_ms,
    // default 33) — the AmbientField/ImmersiveAtmosphere convention.
    readonly property int tickMs: 33
    // The scene clock in seconds. Seeded like AmbientField so two launches
    // don't open on the same pose; never reset on re-activation (Slint's
    // `time` is seconds-since-start, so a scene switch never rewinds either).
    property real timeS: Math.random() * 40.0

    // The APPLIED pack — the scene uniforms. Written ONLY on the pulse edge
    // (see the header); the scene effects bind these. `transientAmp` is the
    // pack's `transient` field — `transient` itself is a RESERVED QML keyword
    // (qmlcachegen rejects it), so the QML-side identifier changes and the
    // shader uniform is named to match.
    property real phase: 0
    property real beat: 0
    property real level: 0
    property real levelSmooth: 0
    property real transientAmp: 0
    /// The onset measured against its own density (viz_qt.rs) — 0 at the local
    /// floor, 1 at a full-scale hit above it. `beat` alone flattens into a
    /// shallow ripple on dense material; this is what still reads as a hit.
    property real beatAc: 0
    property vector4d energyLo: Qt.vector4d(0, 0, 0, 0)
    property vector4d energyHi: Qt.vector4d(0, 0, 0, 0)
    property vector4d bandsLo: Qt.vector4d(0, 0, 0, 0)
    property vector4d bandsHi: Qt.vector4d(0, 0, 0, 0)

    // The STASH. Nothing binds it, so the publish-edge write below costs no
    // scene dirty — the VizSettle pattern (a second, unsynchronised 30 Hz
    // application clock is the exact regression the pulse exists to kill).
    property var pendingPack: null
    // A4: the Line Bed depth ring stash — same pattern, QByteArray this
    // time. The C++ item reads it on the pulse edge via pulseTick().
    property var pendingHeights: null
    // A3: the Spectral Ribbon frame stash — same pattern again (the 517-byte
    // [col][reset][row] blob; layout pinned with ribbon_qt.rs).
    property var pendingRibbon: null
    // B1: the Tunnel Flow Viz16 stash — the 16-band QList<real> QbzViz
    // publishes every viz tick while immersive is open. Same zero-dirty
    // pattern: TunnelFlowScene.qml runs its smoothing/kick/phase
    // accumulators from the pulse edge, never from this notify.
    property var pendingBars: null

    // The pack handed to the RhiItem scenes (A2 Plasma; A4 Line Bed takes
    // pendingHeights instead). Built on the pulse edge from the APPLIED
    // properties — one JS object per pulse, never bound.
    function rhiPack() {
        return {
            "time": root.timeS,
            "beat": root.beat,
            "level": root.level,
            "levelSmooth": root.levelSmooth,
            "energyLo": root.energyLo,
            "energyHi": root.energyHi,
            // Parsed here since the port landed and never handed to a scene.
            "transientAmp": root.transientAmp,
            "bandsLo": root.bandsLo,
            "bandsHi": root.bandsHi,
            "beatAc": root.beatAc,
            "primary": QbzShell.ambientPrimary,
            "secondary": QbzShell.ambientSecondary,
            "accent": QbzShell.ambientAccent
        }
    }

    Connections {
        target: QbzShaderScene
        function onPackJsonChanged() {
            try {
                root.pendingPack = JSON.parse(QbzShaderScene.packJson)
            } catch (e) {
                // A malformed pack keeps the previous uniforms.
            }
        }
        function onLinebedHeightsChanged() {
            // Stash only (no binding consumes this — zero scene dirty).
            root.pendingHeights = QbzShaderScene.linebedHeights
        }
        function onRibbonFrameChanged() {
            // Stash only (same zero-dirty rule as the ring stash).
            root.pendingRibbon = QbzShaderScene.ribbonFrame
        }
        function onSceneChanged() {
            // A4/A3 publish gates: the viz drain only pushes/publishes the
            // scene feeds while their scene is actually on screen (the Rust
            // side reads an atomic; the drain thread cannot read the
            // QObject property). Boot order note: this handler also covers
            // every later write — menu rows, cycleScene, the open-reset,
            // the shader-error latch below.
            QbzShaderScene.setLinebedActive(QbzShaderScene.scene === 5)
            QbzShaderScene.setRibbonActive(QbzShaderScene.scene === 4)
        }
    }

    Connections {
        target: QbzViz
        function onBarsChanged() {
            // B1: stash only (no binding consumes this — zero scene dirty,
            // the pendingPack pattern). Tunnel Flow reads the Viz16 stream,
            // not the shader pack.
            root.pendingBars = QbzViz.bars
        }
    }

    Connections {
        target: QbzShell
        function onPulseMsChanged() {
            if (!root.active)
                return
            root.timeS += root.tickMs / 1000.0
            // A4: Line Bed — apply the stashed ring and repaint ONCE per
            // pulse (deliberately before the pendingPack early-return: the
            // item ticks at pulse rate while active even when the audio
            // feed is parked, exactly like timeS advances for the
            // ShaderEffect scenes).
            if (lineBedLoader.item) {
                lineBedLoader.item.pulseTick(root.pendingHeights)
                root.pendingHeights = null
            }
            // B1: Tunnel Flow — same pulse-edge repaint (the linebed
            // cadence comment above covers this too); the wrapper computes
            // the Viz16 smoothing/kick/phase accumulators itself.
            if (tunnelFlowLoader.item) {
                tunnelFlowLoader.item.pulseTick(root.timeS, root.pendingBars)
                root.pendingBars = null
            }
            // A2: Plasma — same pulse-edge repaint; the pack rides the
            // applied properties (no 200 KB ring, so no stash/gate).
            if (plasmaLoader.item)
                plasmaLoader.item.pulseTick(root.rhiPack())
            // A3: Spectral Ribbon — apply the stashed frame + the ceiling
            // EMA and repaint ONCE per pulse (the linebed cadence comment
            // above covers this too).
            if (ribbonLoader.item) {
                ribbonLoader.item.pulseTick(root.pendingRibbon, root.energyHi)
                root.pendingRibbon = null
            }
            var p = root.pendingPack
            if (p === null)
                return
            root.pendingPack = null
            root.phase = p.phase
            root.beat = p.beat
            root.level = p.level
            root.levelSmooth = p.levelSmooth
            root.transientAmp = p.transient
            root.beatAc = p.beatAc || 0
            root.energyLo = Qt.vector4d(p.energyLo[0], p.energyLo[1], p.energyLo[2], p.energyLo[3])
            root.energyHi = Qt.vector4d(p.energyHi[0], p.energyHi[1], p.energyHi[2], p.energyHi[3])
            root.bandsLo = Qt.vector4d(p.bandsLo[0], p.bandsLo[1], p.bandsLo[2], p.bandsLo[3])
            root.bandsHi = Qt.vector4d(p.bandsHi[0], p.bandsHi[1], p.bandsHi[2], p.bandsHi[3])
        }
    }

    // --- The scenes -------------------------------------------------------------
    // ONLY the active scene is instantiated (see the header). The uniforms map
    // 1:1 to the spec 01 §1 block fields, matched BY NAME against the shader's
    // reflection data — `primary/secondary/accent` ride the shell's ambient
    // palette (the same ambient_qt source the Slint side feeds
    // shader_underlay::set_palette with), `time` is the local clock, and the
    // rest come from the applied pack.

    Component {
        id: tunnelScene
        ShaderEffect {
            blending: false // opaque output (alpha = qt_Opacity = 1)
            property color primary: QbzShell.ambientPrimary
            property color secondary: QbzShell.ambientSecondary
            property color accent: QbzShell.ambientAccent
            property vector4d energyLo: root.energyLo
            property vector4d energyHi: root.energyHi
            property vector4d bandsLo: root.bandsLo
            property vector4d bandsHi: root.bandsHi
            property real time: root.timeS
            property real phase: root.phase
            property real beat: root.beat
            property real level: root.level
            property real resX: root.resW
            property real resY: root.resH
            property real levelSmooth: root.levelSmooth
            property real transientAmp: root.transientAmp
            onStatusChanged: {
                if (status === ShaderEffect.Error)
                    root.onShaderError("tunnel", log)
            }
            fragmentShader: "../assets/shaders/tunnel.frag.qsb"
            // Paired vertex stage (Qt 6.11's built-in default emits no
            // qt_TexCoord0). PAIRED, not shared: the GL program link requires
            // both stages to declare the SAME uniform block (see
            // scene_pack.vert), so tunnel/aurora no longer ride spectrum.vert.
            vertexShader: "../assets/shaders/scene_pack.vert.qsb"
        }
    }

    Component {
        id: auroraScene
        ShaderEffect {
            blending: false
            property color primary: QbzShell.ambientPrimary
            property color secondary: QbzShell.ambientSecondary
            property color accent: QbzShell.ambientAccent
            property vector4d energyLo: root.energyLo
            property vector4d energyHi: root.energyHi
            property vector4d bandsLo: root.bandsLo
            property vector4d bandsHi: root.bandsHi
            property real time: root.timeS
            property real phase: root.phase
            property real beat: root.beat
            property real level: root.level
            property real resX: root.resW
            property real resY: root.resH
            property real levelSmooth: root.levelSmooth
            property real transientAmp: root.transientAmp
            onStatusChanged: {
                if (status === ShaderEffect.Error)
                    root.onShaderError("aurora", log)
            }
            fragmentShader: "../assets/shaders/aurora.frag.qsb"
            vertexShader: "../assets/shaders/scene_pack.vert.qsb"
        }
    }

    Component {
        id: ambientScene
        // Ambient-as-scene (mode 7, spec 01 §2.6): the ALREADY-PORTED
        // ambient.frag (the app-wide background's GPU arm) mounted at the
        // immersive underlay slot with the scene uniform naming. Unlike the
        // app background this mount gets the audio breathe (levelSmooth from
        // the pack — the Slint ambient scene breathes too) and NO legibility
        // dim (dim is a shell-background scrim; the reference's ambient.wgsl
        // has no such term).
        ShaderEffect {
            blending: false
            property color primary: QbzShell.ambientPrimary
            property color secondary: QbzShell.ambientSecondary
            property color accent: QbzShell.ambientAccent
            property real time: root.timeS
            property real levelSmooth: root.levelSmooth
            property real resX: root.resW
            property real resY: root.resH
            property real dim: 0.0
            onStatusChanged: {
                if (status === ShaderEffect.Error)
                    root.onShaderError("ambient", log)
            }
            fragmentShader: "../assets/shaders/ambient.frag.qsb"
            vertexShader: "../assets/shaders/ambient.vert.qsb"
        }
    }

    Loader {
        anchors.fill: parent
        active: root.active
        sourceComponent: root.scene === 2 ? tunnelScene
            : root.scene === 3 ? auroraScene
            : root.scene === 7 ? ambientScene
            : null
    }

    // A4: the Line Bed scene (mode 5) — the QQuickRhiItem, mounted via its
    // QML wrapper. Its OWN loader: the C++ item is not a ShaderEffect and
    // shares nothing with the sourceComponent switch above. The `active`
    // gate IS the pulse-law gate (visible && immersive open && scene === 5
    // && tier — root.active), so the item exists only while it is on
    // screen; every repaint inside it is driven by the pulse handler above.
    Component {
        id: linebedScene
        LineBedScene {
        }
    }

    Loader {
        id: lineBedLoader
        anchors.fill: parent
        active: root.active && root.scene === 5
        sourceComponent: linebedScene
    }

    // A2: the Plasma scene (mode 1) — the feedback-fluid QQuickRhiItem,
    // same mount idiom as Line Bed (its own loader, gated on the pulse-law
    // `active`).
    Component {
        id: plasmaScene
        PlasmaScene {
        }
    }

    Loader {
        id: plasmaLoader
        anchors.fill: parent
        active: root.active && root.scene === 1
        sourceComponent: plasmaScene
    }

    // A3: the Spectral Ribbon scene (mode 4) — the spectrogram
    // QQuickRhiItem PLUS its green-axes overlay (the Slint
    // ImmersiveSpectralOverlay, ImmersiveView.slint:1304-1309), same mount
    // idiom as Line Bed.
    Component {
        id: ribbonScene
        Item {
            // The pulse handler calls `ribbonLoader.item.pulseTick(...)` —
            // the item is THIS wrapper, so the tick must be forwarded to the
            // RibbonItem inside (a plain Item has no pulseTick; the direct
            // call on the wrapper threw TypeError every pulse and the
            // spectrogram never received a frame — owner smoke 2026-08-15).
            function pulseTick(f, peak) {
                ribbonItem.pulseTick(f, peak)
            }
            RibbonScene {
                id: ribbonItem
                anchors.fill: parent
            }
            SpectralOverlay {
                anchors.fill: parent
            }
        }
    }

    Loader {
        id: ribbonLoader
        anchors.fill: parent
        active: root.active && root.scene === 4
        sourceComponent: ribbonScene
    }

    // B1: the Tunnel Flow scene (mode 8, Qt-only — spec 02) — the feedback
    // QQuickRhiItem, same mount idiom as Plasma. The `active` gate IS the
    // pulse-law gate; the item exists only while on screen. The black base
    // is inherent (the scene owns the background while active) and the
    // 240 ms fade-in lives in the wrapper (a transient one-shot, allowed).
    Component {
        id: tunnelFlowScene
        TunnelFlowScene {
        }
    }

    Loader {
        id: tunnelFlowLoader
        anchors.fill: parent
        active: root.active && root.scene === 8
        sourceComponent: tunnelFlowScene
    }
}
