// SpectrumBand — the compact 42px spectrum strip above the Large-NPB cover.
// Split out of SidebarNowPlayingDock.qml: the dock owns the layout arithmetic
// and the cover, this file owns the five render modes and their motion.
// QML port of the `spectrum := Rectangle` block in
// crates/qbz-ui/ui/shell/SidebarNowPlayingDock.slint:122-183 (VBar / WaveCol).
//
// F3 RENDER PATH (2026-08-02) — ONE node, ONE draw call per mode. The modes
// used to be Repeaters of 28/48/5 Rectangle delegates: on macOS (Metal RHI)
// the per-frame batch churn of 28-48 height-mutating nodes was a measured
// slice of the ~2.5 ms render-thread frame (62 fps). Each mode is now ONE
// ShaderEffect — a single scene-graph node — whose fragment shader draws
// every bar itself: the bar index comes from the x coordinate, the gap
// geometry is per mode (bars = 1px-gapped columns, waveform = centred
// columns, energy = 5 wide bars), the height comes from scalar uniforms
// b0..bN (QML maps `property real bN` onto a GLSL uniform BY NAME — uniform
// arrays are NOT supported by ShaderEffect, hence N loose properties), and
// the fill is the SAME vertical topColor->bottomColor gradient plus the 1px
// radius the delegates had (rounded-rect SDF with a 1px AA edge, including
// Rectangle's clamp of r to half the short side). Heights are pushed in ONE
// JS pass per VizSettle application — the push hooks the exact same
// from/to/progress triplet the delegates' at(i) bindings used to capture —
// so a shell-pulse edge dirties ONE node instead of 28-48. No Canvas anywhere:
// Canvas is CPU raster and is banned on this path.
//
// THE SHADER IS A .qsb, NOT AN INLINE STRING. Qt 6's ShaderEffect takes the
// vertexShader/fragmentShader properties as URLs of .qsb files produced by
// the qsb tool — an inline GLSL string is parsed as a URL (the leading
// `#version` even becomes a URL fragment) and the effect dies with
// "Failed to deserialize QShader … status Error". The GLSL 440 sources live
// in qml/assets/shaders/spectrum_{bars,wave,energy}.frag (+ the shared
// spectrum.vert — Qt 6.11's built-in default vertex stage emits no
// qt_TexCoord0, so the program link fails without an explicit one) and the
// baked shaders next to them as .qsb (GLSL 440 for the Linux OpenGL RHI,
// MSL 1.2 for Metal, SPIR-V always embedded for Vulkan). Regenerate after
// ANY shader-source edit, from crates/qbz-qt:
//   for m in bars wave energy; do \
//     /usr/lib64/qt6/bin/qsb --glsl "100 es,300 es,310 es,320 es,120,150,440" --hlsl 50 --msl 12 \
//       -o qml/assets/shaders/spectrum_$m.frag.qsb \
//       qml/assets/shaders/spectrum_$m.frag; \
//   done
//   /usr/lib64/qt6/bin/qsb --glsl "100 es,300 es,310 es,320 es,120,150,440" --hlsl 50 --msl 12 -b \
//     -o qml/assets/shaders/spectrum.vert.qsb qml/assets/shaders/spectrum.vert
// (macOS: qsb is at $(brew --prefix qt)/bin/qsb.) A .qsb is keyed to the Qt
// Shader Tools version that baked it — rebake when the system Qt moves.
//
// FALLBACK ARM. ShaderEffect draws nothing where shaders do not exist —
// the software scene-graph backend (the Settings "Software" renderer,
// QT_QUICK_BACKEND=software, the offscreen/VNC platforms), detected exactly
// like theme/RoundedImage.qml via GraphicsInfo.api — or where the shader
// fails to load (ShaderEffect.status === Error; this fired HERE during
// development when the shader was an inline string, and the band kept
// rendering on the Repeater arm — a load failure must never blank the
// band). In both cases the ORIGINAL Repeater delegates take over (`model`
// flips from 0 back to N), so the band renders the same everywhere. The
// fallback is also what an offscreen/VNC smoke sees.
//
// PERF — notes 1-8 predate the shader arm. They now describe the FALLBACK
// Repeaters and the VizSettle feeding, which are byte-for-byte unchanged:
//  1. The FFT streams REPLACE their QList each publish, so a Repeater bound
//     to QbzViz.bars would rebuild every delegate every frame. The Repeaters
//     use STATIC models (column counts) and bind only each bar's HEIGHT.
//  2. Each mode marshals its QList into JS ONCE per publish (a `target`
//     binding), never once per delegate — the 512-float waveform read is the
//     single most expensive thing in the dock.
//  3. ONE shared Gradient for every bar, not one object per delegate. (On
//     the shader arm the gradient is two vec4 uniforms mixed in GLSL; the
//     Gradient object remains for the fallback Repeaters.)
//  4. Motion comes from ONE VizSettle driver per mode, NOT 28 per-bar
//     Behaviors (the Slint dock animates each bar; one driver is the cheap
//     equivalent — see VizSettle.qml for the full argument).
//  5. Only the ACTIVE mode is instantiated, and only while the band is shown:
//     hiding it unloads the delegates, and viz_qt.rs only ever publishes the
//     one stream the visible mode consumes. (This is also what keeps an
//     inactive mode's SHADER from loading: the Loader owns the effect.)
//  6. Capture itself is gated by the dock's vizShouldRun — a hidden band costs
//     zero: the producer parks and the drain thread sleeps.
//  7. ONE JS binding per bar, not two (fallback arm). `y` used to be a DERIVED
//     binding on `height`, so every height change evaluated two bindings per
//     bar; a static anchor (bottom / verticalCenter) does the same arithmetic
//     in C++ and evaluates nothing. On the shader arm the anchor math is one
//     line of GLSL (`px.y - (bandH - h)` / `* 0.5`).
//  8. `at()` indexes a PLAIN JS ARRAY: VizSettle materialises each published
//     QList_f32 frame once per publish (30/s) instead of letting every `at(i)`
//     cross into the sequence wrapper twice per bar per tick. The waveform arm
//     already builds a plain Array in its own `target` binding, so it is not
//     copied twice (VizSettle branches on Array.isArray).

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: band

    QbzTheme { id: theme }

    // Shown/hidden by the cover's eye toggle (dock-owned).
    property bool shown: true
    // Mirrors the dock's capture gate: false = the stream is frozen, so the
    // interpolator must park too.
    property bool capturing: false
    // A stored scope choice survives a Software session, but the live band
    // resolves it to Bars. The Rust renderer report publishes the same
    // fallback; keeping it here makes the drawing arm safe during that
    // report's event-loop turn too.
    readonly property int effectiveMode: !QbzShell.gpuTier
        && QbzShell.largeSpectrumMode >= 3 ? 0 : QbzShell.largeSpectrumMode

    visible: band.shown
    radius: theme.radiusMd
    color: "#59000000"
    // No clip: the shader arm is anchors.fill, and the fallback bars take
    // settle.at(i) hard-clamped to 0..1 (VizSettle.qml:86-91) against a
    // hardcoded 42px band, so nothing can poke out.

    // Album-derived gradient, reusing the ambient triad (ambient_qt.rs) instead
    // of a second album-color pipeline — the Slint dock reads
    // ImmersiveState.spectrum-primary/secondary, which is the same idea.
    readonly property color topColor: QbzShell.ambientPrimary
    readonly property color bottomColor: QbzShell.ambientSecondary

    // No-shader renderer detection, verbatim from theme/RoundedImage.qml (read
    // its ARM SELECTION note): test Software/Null NEGATIVELY — under the RHI an
    // OpenGL backend reports GraphicsInfo.OpenGL and Metal reports Metal, so
    // "not software" is the only test that does not break the Mac path.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null

    // ONE gradient instance shared by every fallback bar (perf note 3).
    Gradient {
        id: barGradient
        GradientStop { position: 0.0; color: band.topColor }
        GradientStop { position: 1.0; color: band.bottomColor }
    }

    // Mode 0 — Bars: 28 MIRRORED columns over the 14 ACTIVE bins (the 16-bin
    // FFT leaves {1, 15} empty; SpectrumPanel.slint parity).
    Loader {
        anchors.fill: parent
        active: band.shown && band.effectiveMode === 0
        sourceComponent: Item {
            id: barsMode
            readonly property var activeBins: [0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
            readonly property real slot: band.width / 28

            // Marshal the QList into JS ONCE per publish (perf note 2). Already
            // clamped 0..1 in Rust (processor.rs map_to_log_bars).
            VizSettle {
                id: barsSettle
                target: QbzViz.bars
                live: band.capturing
            }

            // ONE JS pass per VizSettle re-evaluation, writing the 14 heights
            // into the shader's scalar uniforms (the shader does the 14->28
            // mirroring). The trigger is the same dependency triplet the
            // delegates' at(i) bindings captured: from, to, progress.
            function push() {
                for (var i = 0; i < 14; ++i)
                    barsFx["b" + i] = barsSettle.at(barsMode.activeBins[i]);
            }
            Connections {
                target: barsSettle
                // ONE handler since VizSettle eases instead of stepping: it
                // applies into `cur` and the three-property from/to/progress
                // triple is gone. Three handlers on properties that no longer
                // exist is not a compile error — QML warns at RUNTIME and the
                // Connections simply never fires, which is exactly how this
                // band went dead in all three modes.
                function onCurChanged() { barsMode.push() }
            }

            // Shader arm health: no shaders at all (software backend) or a
            // rejected compile -> the fallback Repeater below renders.
            readonly property bool fxOk: !band._noShaders && barsFx.status !== ShaderEffect.Error

            ShaderEffect {
                id: barsFx
                anchors.fill: parent
                visible: barsMode.fxOk

                property real bandW: width
                property real bandH: height
                property color topColor: band.topColor
                property color bottomColor: band.bottomColor
                property real b0: 0.0
                property real b1: 0.0
                property real b2: 0.0
                property real b3: 0.0
                property real b4: 0.0
                property real b5: 0.0
                property real b6: 0.0
                property real b7: 0.0
                property real b8: 0.0
                property real b9: 0.0
                property real b10: 0.0
                property real b11: 0.0
                property real b12: 0.0
                property real b13: 0.0

                Component.onCompleted: barsMode.push()
                onStatusChanged: {
                    if (status === ShaderEffect.Error)
                        console.warn("SpectrumBand: bars shader failed:", log);
                }

                // .qsb baked from qml/assets/shaders/spectrum_bars.frag — see the header.
                fragmentShader: "../assets/shaders/spectrum_bars.frag.qsb"
                // Paired vertex stage (Qt 6.11's built-in default emits no
                // qt_TexCoord0); the GL link requires both stages to declare
                // the SAME uniform block (see spectrum_bars.vert).
                vertexShader: "../assets/shaders/spectrum_bars.vert.qsb"
            }

            // FALLBACK ARM — the pre-F3 delegates, verbatim. Geometry: x =
            // index*slot+1, width slot-2, bottom-anchored, radius 1.
            Repeater {
                model: barsMode.fxOk ? 0 : 28
                delegate: Rectangle {
                    required property int index
                    // Mirror: columns 14..27 replay 13..0.
                    readonly property int bin: index < 14
                        ? barsMode.activeBins[index]
                        : barsMode.activeBins[27 - index]
                    x: index * barsMode.slot + 1
                    width: barsMode.slot - 2
                    height: Math.max(2, barsSettle.at(bin) * band.height)
                    anchors.bottom: parent.bottom
                    radius: 1
                    gradient: barGradient
                }
            }
        }
    }

    // Mode 1 — Waveform: 48 downsampled L columns, mirrored about the band's
    // vertical centre (the L half is samples 0..255).
    Loader {
        anchors.fill: parent
        active: band.shown && band.effectiveMode === 1
        sourceComponent: Item {
            id: waveMode
            readonly property real slot: band.width / 48

            VizSettle {
                id: waveSettle
                live: band.capturing
                // The |sample| envelope is folded in HERE, once per publish (48
                // ops), instead of an abs+min per column per rendered frame.
                target: {
                    var s = QbzViz.waveform;
                    var out = new Array(48);
                    for (var i = 0; i < 48; ++i) {
                        var v = s[i * 5] || 0;
                        if (v < 0) v = -v;
                        out[i] = v > 1 ? 1 : v;
                    }
                    return out;
                }
            }

            // ONE JS pass per VizSettle re-evaluation (48 uniforms).
            function push() {
                for (var i = 0; i < 48; ++i)
                    waveFx["b" + i] = waveSettle.at(i);
            }
            Connections {
                target: waveSettle
                // ONE handler since VizSettle eases instead of stepping: it
                // applies into `cur` and the three-property from/to/progress
                // triple is gone. Three handlers on properties that no longer
                // exist is not a compile error — QML warns at RUNTIME and the
                // Connections simply never fires, which is exactly how this
                // band went dead in all three modes.
                function onCurChanged() { waveMode.push() }
            }

            readonly property bool fxOk: !band._noShaders && waveFx.status !== ShaderEffect.Error

            ShaderEffect {
                id: waveFx
                anchors.fill: parent
                visible: waveMode.fxOk

                property real bandW: width
                property real bandH: height
                property color topColor: band.topColor
                property real b0: 0.0
                property real b1: 0.0
                property real b2: 0.0
                property real b3: 0.0
                property real b4: 0.0
                property real b5: 0.0
                property real b6: 0.0
                property real b7: 0.0
                property real b8: 0.0
                property real b9: 0.0
                property real b10: 0.0
                property real b11: 0.0
                property real b12: 0.0
                property real b13: 0.0
                property real b14: 0.0
                property real b15: 0.0
                property real b16: 0.0
                property real b17: 0.0
                property real b18: 0.0
                property real b19: 0.0
                property real b20: 0.0
                property real b21: 0.0
                property real b22: 0.0
                property real b23: 0.0
                property real b24: 0.0
                property real b25: 0.0
                property real b26: 0.0
                property real b27: 0.0
                property real b28: 0.0
                property real b29: 0.0
                property real b30: 0.0
                property real b31: 0.0
                property real b32: 0.0
                property real b33: 0.0
                property real b34: 0.0
                property real b35: 0.0
                property real b36: 0.0
                property real b37: 0.0
                property real b38: 0.0
                property real b39: 0.0
                property real b40: 0.0
                property real b41: 0.0
                property real b42: 0.0
                property real b43: 0.0
                property real b44: 0.0
                property real b45: 0.0
                property real b46: 0.0
                property real b47: 0.0

                Component.onCompleted: waveMode.push()
                onStatusChanged: {
                    if (status === ShaderEffect.Error)
                        console.warn("SpectrumBand: waveform shader failed:", log);
                }

                // .qsb baked from qml/assets/shaders/spectrum_wave.frag — see the header.
                fragmentShader: "../assets/shaders/spectrum_wave.frag.qsb"
                // Paired vertex stage — same uniform block in both stages
                // (the GL link requires it; see spectrum_wave.vert).
                vertexShader: "../assets/shaders/spectrum_wave.vert.qsb"
            }

            // FALLBACK ARM — the pre-F3 delegates, verbatim. Geometry: x =
            // index*slot+1, width slot-2, vertically centred, flat topColor.
            Repeater {
                model: waveMode.fxOk ? 0 : 48
                delegate: Rectangle {
                    required property int index
                    x: index * waveMode.slot + 1
                    width: waveMode.slot - 2
                    height: Math.max(2, waveSettle.at(index) * band.height)
                    anchors.verticalCenter: parent.verticalCenter
                    radius: 1
                    color: band.topColor
                }
            }
        }
    }

    // Mode 2 — Energy: the 5 semantic bands (sub-bass .. air).
    Loader {
        anchors.fill: parent
        active: band.shown && band.effectiveMode === 2
        sourceComponent: Item {
            id: energyMode
            readonly property real slot: band.width / 5

            VizSettle {
                id: energySettle
                target: QbzViz.energy
                live: band.capturing
            }

            // ONE JS pass per VizSettle re-evaluation (5 uniforms).
            function push() {
                for (var i = 0; i < 5; ++i)
                    energyFx["b" + i] = energySettle.at(i);
            }
            Connections {
                target: energySettle
                // ONE handler since VizSettle eases instead of stepping: it
                // applies into `cur` and the three-property from/to/progress
                // triple is gone. Three handlers on properties that no longer
                // exist is not a compile error — QML warns at RUNTIME and the
                // Connections simply never fires, which is exactly how this
                // band went dead in all three modes.
                function onCurChanged() { energyMode.push() }
            }

            readonly property bool fxOk: !band._noShaders && energyFx.status !== ShaderEffect.Error

            ShaderEffect {
                id: energyFx
                anchors.fill: parent
                visible: energyMode.fxOk

                property real bandW: width
                property real bandH: height
                property color topColor: band.topColor
                property color bottomColor: band.bottomColor
                property real b0: 0.0
                property real b1: 0.0
                property real b2: 0.0
                property real b3: 0.0
                property real b4: 0.0

                Component.onCompleted: energyMode.push()
                onStatusChanged: {
                    if (status === ShaderEffect.Error)
                        console.warn("SpectrumBand: energy shader failed:", log);
                }

                // .qsb baked from qml/assets/shaders/spectrum_energy.frag — see the header.
                fragmentShader: "../assets/shaders/spectrum_energy.frag.qsb"
                // Paired vertex stage — same uniform block in both stages
                // (the GL link requires it; see spectrum_energy.vert).
                vertexShader: "../assets/shaders/spectrum_energy.vert.qsb"
            }

            // FALLBACK ARM — the pre-F3 delegates, verbatim. Geometry: x =
            // index*slot+3, width slot-6, bottom-anchored, radius 1.
            Repeater {
                model: energyMode.fxOk ? 0 : 5
                delegate: Rectangle {
                    required property int index
                    x: index * energyMode.slot + 3
                    width: energyMode.slot - 6
                    height: Math.max(2, energySettle.at(index) * band.height)
                    anchors.bottom: parent.bottom
                    radius: 1
                    gradient: barGradient
                }
            }
        }
    }

    // Modes 3/4 — real-time scopes. Each Loader owns a single scene-graph
    // line strip; when inactive the item does not exist and Rust also disables
    // that scope's DSP bit.
    Loader {
        anchors.fill: parent
        active: band.shown && band.effectiveMode === 3
        sourceComponent: ScopePanel {
            compact: true
            scopeMode: 0
            traceColor: band.topColor
        }
    }
    Loader {
        anchors.fill: parent
        active: band.shown && band.effectiveMode === 4
        sourceComponent: ScopePanel {
            compact: true
            scopeMode: 1
            traceColor: band.topColor
        }
    }

    // Click the band → cycle all five modes on GPU, the three compatible
    // modes on Software (persisted by the Rust handler).
    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: QbzShell.largeCycleSpectrum()
    }
}
