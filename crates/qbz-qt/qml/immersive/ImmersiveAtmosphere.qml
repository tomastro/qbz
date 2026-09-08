// ImmersiveAtmosphere — the Kawarp-like animated atmosphere underlay (§6.1
// of the 2026-08-02 immersive-port contract), port of
// crates/qbz-ui/ui/immersive/ImmersiveAtmosphere.slint:8-83.
//
// FOUR animated layers of the SAME 128x128 atmosphere bitmap (host-generated:
// tiny artwork sample -> blur -> color adjust -> vignette), each oversized
// past the viewport and drifting on its own sine pair, plus a STATIC fallback
// (the plain cover at 0.35 opacity) when no atmosphere bitmap exists. A
// `dim` overlay and a 180-degree gradient scrim sit on top (:77-82). The
// component only animates/composites — it can be reused outside ImmersiveView.
//
// Mounted by ImmersiveView as layer 2 (§5.1): ALWAYS the underlay (the Slint
// shader-mode==0 gate is constant in v1, ruling 1), source
// QbzImmersive.atmosphereUrl, fallback QbzPlayer.npArtworkPath,
// `animated: QbzPlayer.npPlaying`, dim 0.15 (:1313-1321). Spectrum/WaveBed
// paint opaque #000 over it — intended faithful behavior
// (SpectrumPanel.slint:45-46,118, ImmersiveWaveBedPanel.slint:49).
//
// The drift clock: Slint reads animation-tick() pull-based and FREEZES it at
// 0 while !animated (:16). The Qt twin accumulates a local `tick` (ms) on the
// shared shell pulse (QbzShell.pulseMs — see the cost note below), reset to 0
// whenever `animated` drops, so the static pose is the Slint tick=0 pose
// exactly. All x/y offsets are plain bindings of tick — nothing is animated
// per-layer.
//
// TWO RENDER ARMS (2026-08-13), same detection as SpectrumBand/RoundedImage:
//
//   GPU  — ONE ShaderEffect (assets/shaders/atmosphere.frag) that composites
//          all four crop-drifts, the base, the dim and the scrim in a single
//          opaque pass. The four-Image stack paid ~7 full-window blended
//          nodes on EVERY present — and Qt Quick repaints the whole window on
//          any dirty, so the visualiser's 30 presents/s each paid the
//          background's six blended passes too. One opaque node leaves the
//          alpha pass entirely (front-to-back, early-z).
//   FALLBACK — the original four-Image stack, verbatim, for the software /
//          null scene graphs (Settings "Software" renderer, offscreen/VNC)
//          and for a shader that fails to load: a load failure must never
//          blank the background. This is also the arm an offscreen smoke
//          sees, which is why the fallback is the parity reference and the
//          shader must match IT.

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz

Item {
    id: root

    /// 128x128 atmosphere PNG (file:// URL; "" = not ready). NEVER cleared
    /// on track change by the publisher (flicker fix, §3).
    property string source: ""
    /// Plain cover (file:// URL) shown when `source` is empty.
    property string fallbackSource: ""
    /// Dark overlay opacity (:11, :78).
    property real dim: 0.15
    /// Layer opacities (:12-13).
    property real baseOpacity: 0.95
    property real warpOpacity: 0.48
    /// Gates the layer drift (:14); the mount binds QbzPlayer.npPlaying.
    property bool animated: true

    // Drift clock in ms (:16). Reset to 0 on the animated->static edge so the
    // static pose is the Slint tick=0 pose exactly.
    property real tick: 0
    // Driven by THE SHELL PULSE (QbzShell.pulseMs, shell_bridge.rs) — not a
    // private Timer, and never a NumberAnimation, and the difference is the
    // whole cost of this component.
    //
    // A NumberAnimation is driven by the animation driver: it updates once per
    // ANIMATION FRAME, i.e. at display rate — 180 Hz on the owner's panel. The
    // sine bindings below read `tick`, so every one of those updates moves
    // four oversized layers and dirties the scene. Qt Quick has no dirty-region
    // rendering, so each is a WHOLE-WINDOW repaint, and on a hybrid laptop
    // whose external monitor hangs off the dGPU each repaint is also a frame
    // KWin must composite and scan out on that GPU.
    //
    // A PRIVATE 30 Hz Timer fixed the rate but not the bill: the visualizer's
    // own ~30 Hz clock fires on a different phase, and two unsynchronised
    // clocks still cost two full-window presents per period. Measured on that
    // machine: ~95% GPU with this component up, unmoved by anything that
    // touched what was DRAWN, because the cost was how OFTEN. The shared
    // pulse makes every continuous animator in the shell dirty the scene in
    // the SAME event-loop turn, so the window presents ONCE per period for
    // all of them. That coalescing — not the period — is the fix.
    //
    // These layers drift on 5.7-15.1 s sine periods. 30 Hz is already ~6x more
    // samples than that motion can show; the extra frames bought nothing and
    // cost a full-window composite each.
    //
    // `tickMs` MUST match the pulse period (settings_qt::shell_pulse_ms,
    // default 33): `tick` is a local accumulation advanced on the pulse edge,
    // so a mismatch only mis-speeds the drift, never the repaint rate.
    readonly property int tickMs: 33
    Connections {
        target: QbzShell
        function onPulseMsChanged() {
            if (root.animated)
                root.tick += root.tickMs
        }
    }
    onAnimatedChanged: if (!animated) tick = 0

    clip: true

    // --- The drift, ONE source of truth for both arms -----------------------
    // The eight sine pairs of the .slint (:24-31, :36-43, :48-55, :60-67),
    // hoisted from the four Images so the shader arm's uniforms move on the
    // SAME values the fallback stack positions with.
    readonly property real s1x: Math.sin(root.tick / 7600.0 * 6.283185)
        + 0.35 * Math.sin((root.tick / 3300.0 + 1.9) * 6.283185)
    readonly property real s1y: Math.sin((root.tick / 11200.0 + 1.1) * 6.283185)
    readonly property real s2x: Math.sin((root.tick / 9200.0 + 0.4) * 6.283185)
        + 0.42 * Math.sin((root.tick / 4100.0 + 2.6) * 6.283185)
    readonly property real s2y: Math.sin((root.tick / 14800.0 + 1.7) * 6.283185)
    readonly property real s3x: Math.sin((root.tick / 5700.0 + 1.6) * 6.283185)
    readonly property real s3y: Math.sin((root.tick / 12300.0 + 0.2) * 6.283185)
        + 0.28 * Math.sin((root.tick / 3600.0 + 2.8) * 6.283185)
    readonly property real s4x: Math.sin((root.tick / 15100.0 + 2.4) * 6.283185)
    readonly property real s4y: Math.sin((root.tick / 6900.0 + 1.4) * 6.283185)

    // Same detection as SpectrumBand/RoundedImage: the software and null scene
    // graphs draw NOTHING for a ShaderEffect, so they take the fallback arm.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null
    // A shader that fails to LOAD must not blank the background either (the
    // failure SpectrumBand hit in development), so the status is part of the
    // predicate, not just the backend.
    readonly property bool fxOk: !root._noShaders && fx.status !== ShaderEffect.Error

    // Root surface (:18). Stays mounted under the GPU arm too — an opaque
    // node under an opaque effect is early-z discard, and the static-cover
    // path (:68-76) still needs it.
    Rectangle {
        anchors.fill: parent
        color: "#0a0a0b"
    }

    // --- GPU arm: ONE opaque pass --------------------------------------------
    // The bitmap as a plain sampler (never shown itself).
    Image {
        id: atmoTex
        visible: false
        source: root.source
        // The shader samples the whole 128x128; the layers' oversize+crop
        // math lives in packLayer, not here.
        smooth: true
    }

    // Pack one layer the way PreserveAspectCrop of a SQUARE source composes:
    // cover scale s = max(rw, rh), centred crop, so a fragment at item-pixel
    // px samples uv = (px - (o + R/2)) / s + 0.5 (see atmosphere.frag).
    // Returns (cx, cy, s, opacity).
    function packLayer(dw, dh, ox, oy, o) {
        var rw = root.width + dw
        var rh = root.height + dh
        var s = Math.max(rw, rh)
        return Qt.vector4d(ox + rw / 2, oy + rh / 2, s, o)
    }
    // The four layers, byte-for-byte the geometry of the fallback Images
    // below (oversize, origin, drift amplitude, opacity).
    readonly property vector4d layer1:
        packLayer(520, 420, -260 + s1x * 132, -210 + s1y * 58, root.baseOpacity)
    readonly property vector4d layer2:
        packLayer(620, 500, -310 - s2x * 156, -250 - s2y * 74, root.warpOpacity)
    readonly property vector4d layer3:
        packLayer(700, 540, -350 + s3x * 118, -270 + s3y * 92, 0.24)
    readonly property vector4d layer4:
        packLayer(560, 620, -280 - s4x * 102, -310 + s4y * 128, 0.18)

    ShaderEffect {
        id: fx
        anchors.fill: parent
        visible: root.fxOk && root.source !== ""
        // The composite is opaque (base #0a0a0b, see the shader), so this
        // node renders in the OPAQUE pass — not the 500-node alpha stack the
        // four-Image arm lands in.
        blending: false

        property var tex: atmoTex
        property real resX: width
        property real resY: height
        property real dim: root.dim
        property vector4d l1: root.layer1
        property vector4d l2: root.layer2
        property vector4d l3: root.layer3
        property vector4d l4: root.layer4

        onStatusChanged: {
            if (status === ShaderEffect.Error)
                console.warn("ImmersiveAtmosphere: atmosphere shader failed:", log);
        }

        fragmentShader: "../assets/shaders/atmosphere.frag.qsb"
        // Paired vertex stage (Qt 6.11's built-in default emits no
        // qt_TexCoord0); the GL link requires both stages to declare the
        // SAME uniform block (see atmosphere.vert / scene_pack.vert).
        vertexShader: "../assets/shaders/atmosphere.vert.qsb"
    }

    // --- FALLBACK arm: the original four-Image stack, verbatim ----------------
    // --- Layer 1 (:21-32) -------------------------------------------------
    Image {
        readonly property real sx: root.s1x
        readonly property real sy: root.s1y
        visible: !root.fxOk && root.source !== ""
        source: root.source
        fillMode: Image.PreserveAspectCrop
        width: root.width + 520
        height: root.height + 420
        x: -260 + sx * 132
        y: -210 + sy * 58
        opacity: root.baseOpacity
    }
    // --- Layer 2 (:33-44) -------------------------------------------------
    Image {
        readonly property real sx: root.s2x
        readonly property real sy: root.s2y
        visible: !root.fxOk && root.source !== ""
        source: root.source
        fillMode: Image.PreserveAspectCrop
        width: root.width + 620
        height: root.height + 500
        x: -310 - sx * 156
        y: -250 - sy * 74
        opacity: root.warpOpacity
    }
    // --- Layer 3 (:45-56) -------------------------------------------------
    Image {
        readonly property real sx: root.s3x
        readonly property real sy: root.s3y
        visible: !root.fxOk && root.source !== ""
        source: root.source
        fillMode: Image.PreserveAspectCrop
        width: root.width + 700
        height: root.height + 540
        x: -350 + sx * 118
        y: -270 + sy * 92
        opacity: 0.24
    }
    // --- Layer 4 (:57-67) -------------------------------------------------
    Image {
        readonly property real sx: root.s4x
        readonly property real sy: root.s4y
        visible: !root.fxOk && root.source !== ""
        source: root.source
        fillMode: Image.PreserveAspectCrop
        width: root.width + 560
        height: root.height + 620
        x: -280 - sx * 102
        y: -310 + sy * 128
        opacity: 0.18
    }
    // --- Static fallback (:68-76) ------------------------------------------
    Image {
        visible: root.source === "" && root.fallbackSource !== ""
        source: root.fallbackSource
        fillMode: Image.PreserveAspectCrop
        width: root.width + 220
        height: root.height + 220
        x: -110
        y: -110
        opacity: 0.35
    }

    // dim overlay (:77-79). Baked INTO the shader on the GPU arm; only the
    // fallback stack and the static-cover path paint it as a node.
    Rectangle {
        anchors.fill: parent
        visible: !root.fxOk || root.source === ""
        color: Qt.rgba(0, 0, 0, root.dim)
    }
    // 180-degree scrim (:80-82): #00000066 top -> transparent 35% ->
    // #00000080 bottom (RRGGBBAA converted). Same GPU-arm folding as dim.
    Rectangle {
        anchors.fill: parent
        visible: !root.fxOk || root.source === ""
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#66000000" }
            GradientStop { position: 0.35; color: "transparent" }
            GradientStop { position: 1.0; color: "#80000000" }
        }
    }
}
