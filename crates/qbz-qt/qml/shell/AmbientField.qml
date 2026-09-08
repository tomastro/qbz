// AmbientField — the app-wide dynamic background (phase 14), mode 1.
//
// The Qt rendering of the Slint ambient scene (crates/qbz-ui/ui/shaders/
// ambient.wgsl, wgpu scene 7). Two arms:
//
//   GPU  — a ShaderEffect over assets/shaders/ambient.frag rendered into a
//          ShaderEffectSource at 30 fps (see "THE SHADER IS RENDERED TO A
//          TEXTURE" below — that schedule is load-bearing, not an optimisation
//          detail), which is a line-for-line GLSL port of the WGSL:
//          two-octave fbm domain warp,
//          four album-coloured METABALLS (r^2/d^2, so the lobes fuse and
//          stretch), the wide `smoothstep(0.45, 3.4)` iso-surface, the
//          `mix(0.42, 1.12)` bright/dark spread, the 1.28 saturation push,
//          the quadratic vertical shade, the 0.92 master and the grain.
//   CPU  — a Canvas fallback for the software scene graph (the Settings
//          "Software" renderer, QT_QUICK_BACKEND=software, offscreen/VNC),
//          detected via GraphicsInfo exactly like theme/RoundedImage.qml.
//
// WHAT CHANGED AND WHY (2026-08-11). The Canvas used to be the ONLY arm, and
// it painted its blobs over a BLACK base, so every pixel outside a blob's
// radius was #000000. That is not a small fidelity gap: the reference's colour
// is the metaball-WEIGHTED album colour, which is defined everywhere, scaled by
// at LEAST 0.42 — a full-window wash that brightens into lobes. Measured on the
// owner's side-by-side, Qt was (0,0,0) at (1600,600) and (1690,880) where Slint
// was (58,51,5) and (29,27,10): a black top-right quadrant against a continuous
// olive field. The Canvas arm now paints that 0.42 floor as its base colour and
// corrects the radius (see below), so even the software path stops going black.
//
// THE RADIUS BUG, worth naming: the WGSL works in aspect-corrected uv, where x
// spans [0, aspect] and y spans [0, 1] — so `rr = 0.34` is 0.34 of the window
// HEIGHT. The Canvas port read it as `0.36 * max(W, H)`, which on a 1700x1400
// window is 612 px against the reference's 476: blobs 29% too big, on top of
// the black base.
//
// STILL ABSENT, deliberately: the audio breathe. The reference multiplies the
// blob radius by `1 + 0.12 * level_smooth`, and Slint keeps its 30 fps FFT
// drain running for mode 1 — but there it has no choice, because that same
// drain is what renders the shader texture the background shows. This arm
// drives itself off the shared shell pulse below, so the tap would be
// switched on app-wide purely for a +-12% radius pulse, which is the wrong
// trade in a port measured on idle CPU. At level 0 the reference's `breathe`
// is exactly 1.0, so the field's geometry here is the reference's geometry.
//
// GATING RULE (owner, 2026-07-28): freeze on NOT VISIBLE, never on lost focus.
// A tiling/mosaic desktop keeps windows visible and unfocused all the time —
// pausing on focus loss would stop the field while the user is looking straight
// at it. Minimized/hidden is the real "nobody can see it".

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz

Item {
    id: root

    // The album triad (hex strings from ambient_qt, recomputed on track
    // change). The field crossfades on its own: both arms redraw continuously,
    // so the new colours melt in over the blob softness.
    property color primary: QbzShell.ambientPrimary
    property color secondary: QbzShell.ambientSecondary
    property color accent: QbzShell.ambientAccent
    // Set false by the host to freeze the clock (inactive / hidden).
    property bool running: true
    // The in-shader legibility dim (multiply toward black). The host lowers
    // it to 0 on LIGHT themes, where darkening is the wrong direction — the
    // polarity-aware veil rectangle above the field takes over there
    // (AppShell, 2026-08-31).
    property real dim: QbzShell.ambientDim

    // True unless the window is minimized or hidden — see the GATING RULE at
    // the top. Focus is deliberately NOT part of this.
    readonly property bool windowShowing: root.Window.window
        ? (root.Window.window.visibility !== Window.Minimized
           && root.Window.window.visibility !== Window.Hidden)
        : true

    // Same detection as SpectrumBand/RoundedImage: the software and null scene
    // graphs draw NOTHING for a ShaderEffect, so they take the Canvas arm.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null
    // A shader that fails to LOAD must not blank the background either (the
    // failure SpectrumBand hit in development), so the status is part of the
    // predicate, not just the backend.
    readonly property bool fxOk: !root._noShaders && fx.status !== ShaderEffect.Error

    // --- Render size (shader_underlay.rs:32-38) ------------------------------
    // The reference renders into an offscreen target that TRACKS the window's
    // physical pixel size (capped at 2560x1440) and shows it with
    // `image-fit: fill`. Rendering INLINE now (see the GPU arm below), the
    // target IS the framebuffer, so the size is simply the item's physical
    // size — below the 2560x1440 cap this is pixel-for-pixel what the
    // reference renders at this window size anyway.
    readonly property real dpr: root.Screen.devicePixelRatio
    readonly property real resW: Math.max(1, Math.round(width * dpr))
    readonly property real resH: Math.max(1, Math.round(height * dpr))

    // The scene clock, in seconds. Accumulated at ~30 fps — the spec's cost
    // control (§6): the field is a background that must never cost more than
    // the data it shows, and the orbits are 10-18 s long, so 30 fps is already
    // far more than the motion needs. The seed keeps two launches from opening
    // on the same pose.
    property real t: Math.random() * 40.0

    // --- ONE PAINT SOURCE, ON THE SHARED PULSE -----------------------------
    //
    // Qt Quick has no dirty-region rendering: ANY dirty node redraws the whole
    // window, so the shell's GPU cost is linear in how many times per second
    // something asks for a repaint — measured on the owner's 4070 at half
    // screen, ~1.2% of GPU per repaint/s (30 Hz -> 36%, ~60 Hz -> 70-82%).
    // WHAT is redrawn barely matters: halving the field's resolution
    // (QBZ_BG_SCALE=0.5), caching the pane in a layer (QBZ_PANE_LAYER=1) and
    // slowing the band's interpolation to 10 Hz (QBZ_VIZ_TICK=100) each moved
    // the number by nothing or almost nothing.
    //
    // 2026-08-13 single-pulse redesign: the field advances on the shell pulse
    // (QbzShell.pulseMs, shell_bridge.rs) — the SAME edge VizSettle and
    // ImmersiveAtmosphere tick on — so every continuous animator in the shell
    // dirties the scene in one event-loop turn and the window presents ONCE
    // per period for all of them. This replaces both earlier sources at once:
    // the private 33 ms fallback timer AND the publish-edge advance (which
    // stopped being "a repaint the window was going to do anyway" the moment
    // VizSettle's publish path went stash-only — an onBarsChanged advance
    // would now be a SECOND, unsynchronised 30 Hz clock, the exact regression
    // the pulse exists to kill).
    readonly property int tickMs: 33

    function advance() {
        // The GPU arm needs NOTHING but the clock bump: `fx.time` binds root.t,
        // so this alone dirties the shader and the next present renders it.
        root.t += root.tickMs / 1000.0
        if (!root.fxOk)
            canvas.requestPaint()
    }

    // The pulse edge. A handler that writes nothing schedules no frame, so
    // the gates (host freeze, unmounted, minimized/hidden — the GATING RULE
    // at the top) live here, not in a Timer's `running`.
    Connections {
        target: QbzShell
        function onPulseMsChanged() {
            if (root.running && root.visible && root.windowShowing)
                root.advance()
        }
    }

    // --- GPU arm ------------------------------------------------------------
    //
    // THE SHADER RENDERS INLINE, NOT INTO A TEXTURE — 2026-08-13, and the
    // history matters because this is the THIRD shape of this arm:
    //
    //  1. Inline (first cut) pinned an RTX 4070 near 100%: pre-pulse the shell
    //     presented 62-180x/s (VizSettle's 16 ms timer, display-rate
    //     animations) and every present re-ran 4 fbm octaves over the window.
    //  2. ShaderEffectSource + live:false fixed THAT — the fbm ran only on
    //     scheduleUpdate() and other frames sampled the texture. But a minimal
    //     repro (100 ms timer + scheduleUpdate, QSG_RENDER_TIMING) measures
    //     TWO presents per update: one renders the FBO, then the changed
    //     texture dirties the scene again. Under the single-pulse regime the
    //     FBO's rate-decoupling buys nothing (the window presents at the pulse
    //     rate anyway) and the doubled present is pure cost — presents/s is
    //     THE GPU term on the owner's hybrid stack (~1.2% GPU per
    //     full-window present/s, area-independent, measured 2026-08-13).
    //  3. Inline again (this cut): ONE present per pulse, fbm once per present
    //     at window resolution. The residual exposure — scroll/hover bursts
    //     re-running the field at interaction rate — is transient by
    //     definition, and the field's fragment cost was measured NOT to be the
    //     dominant term (QBZ_BG_SCALE=0.5 moved the GPU ~5 points).
    //
    // The picture is UNCHANGED — same program, same uniforms.
    ShaderEffect {
        id: fx
        anchors.fill: parent
        visible: root.fxOk
        // The output is opaque (alpha = qt_Opacity = 1), so there is nothing to
        // blend against — this is the bottom-most layer in the window.
        blending: false

        // Matched BY NAME against the uniform block in ambient.frag.
        property color primary: root.primary
        property color secondary: root.secondary
        property color accent: root.accent
        property real time: root.t
        // The absent audio breathe (see the header). The shader keeps the term
        // so wiring a level later is a one-line binding, not a shader edit.
        property real levelSmooth: 0.0
        // The FRAMEBUFFER's size (inline arm): the aspect correction and the
        // grain frequency describe the pixels actually being written.
        property real resX: root.resW
        property real resY: root.resH
        // The dark legibility scrim, applied inside the program rather than as
        // a second full-window Rectangle above it — see the shader. Host-fed:
        // 0 on light themes (the veil rectangle handles legibility there).
        property real dim: root.dim

        onStatusChanged: {
            if (status === ShaderEffect.Error)
                console.warn("AmbientField: shader failed:", log)
        }

        fragmentShader: "../assets/shaders/ambient.frag.qsb"
        // Paired vertex stage — Qt 6.11's built-in default emits no
        // qt_TexCoord0, and the GL link requires both stages to declare the
        // SAME uniform block (see ambient.vert / scene_pack.vert).
        vertexShader: "../assets/shaders/ambient.vert.qsb"
    }

    // --- CPU fallback arm ---------------------------------------------------
    // Plain radial gradients cannot reproduce the metaball tail, so this is an
    // approximation by construction — but it is the SAME approximation the
    // reference makes at its extremes: a base of the album mix at the 0.42
    // floor, with the lobes added on top. What it drops versus the shader is
    // the fbm warp (the lobes are circles, not amoebas) and the grain.
    Canvas {
        id: canvas
        anchors.fill: parent
        visible: !root.fxOk
        renderTarget: Canvas.Image
        // See RoundedImage.qml. This one repaints 30x/s for the whole window
        // lifetime, so it is the canvas most likely to be mid-raster when
        // another is torn down on the same render thread.
        renderStrategy: Canvas.Immediate

        Connections {
            target: root
            function onPrimaryChanged() { canvas.requestPaint() }
            function onSecondaryChanged() { canvas.requestPaint() }
            function onAccentChanged() { canvas.requestPaint() }
        }
        onWidthChanged: requestPaint()
        onHeightChanged: requestPaint()
        onVisibleChanged: if (visible) requestPaint()

        // One lobe: radial gradient, centre colour at `a` alpha fading to
        // transparent (the shader's wide smoothstep falloff).
        function blob(ctx, cx, cy, r, c, a) {
            var g = ctx.createRadialGradient(cx, cy, 0, cx, cy, r)
            g.addColorStop(0.0, Qt.rgba(c.r, c.g, c.b, a))
            g.addColorStop(0.55, Qt.rgba(c.r, c.g, c.b, a * 0.45))
            g.addColorStop(1.0, Qt.rgba(c.r, c.g, c.b, 0.0))
            ctx.fillStyle = g
            ctx.fillRect(cx - r, cy - r, 2 * r, 2 * r)
        }

        onPaint: {
            var ctx = getContext("2d")
            ctx.reset()
            var W = width
            var H = height
            if (W <= 0 || H <= 0)
                return

            var p = root.primary
            var s = root.secondary
            var a = root.accent
            var c4 = Qt.rgba((p.r + a.r) / 2, (p.g + a.g) / 2, (p.b + a.b) / 2, 1.0)

            // THE FLOOR — the whole point of this rewrite. Where the shader has
            // no dominant lobe it still paints the weighted album colour scaled
            // by 0.42 (then the 0.92 master), so the base here is the four-way
            // mix at that same level. Never black.
            var f = 0.42 * 0.92
            ctx.fillStyle = Qt.rgba(
                (p.r + s.r + a.r + c4.r) / 4 * f,
                (p.g + s.g + a.g + c4.g) / 4 * f,
                (p.b + s.b + a.b + c4.b) / 4 * f,
                1.0)
            ctx.fillRect(0, 0, W, H)

            // The shader's orbit constants, with the aspect folded back out —
            // in the WGSL's uv the centres are (expr * aspect, expr) over
            // x in [0, aspect] and y in [0, 1], so as WINDOW FRACTIONS they are
            // the plain expressions.
            var t = root.t * 0.75
            var cA = [0.32 + 0.30 * Math.sin(t * 0.40), 0.42 + 0.30 * Math.cos(t * 0.33)]
            var cB = [0.66 + 0.30 * Math.sin(t * 0.35 + 2.1), 0.56 + 0.32 * Math.cos(t * 0.29 + 1.3)]
            var cC = [0.50 + 0.34 * Math.cos(t * 0.31 + 4.0), 0.36 + 0.30 * Math.sin(t * 0.45 + 3.2)]
            var cD = [0.46 + 0.32 * Math.sin(t * 0.27 + 5.3), 0.64 + 0.28 * Math.cos(t * 0.49 + 0.7)]

            // rr = 0.34 of the field HEIGHT (see the header): the uv space the
            // constant lives in has y spanning exactly 1.
            var rr = 0.34 * H
            ctx.globalCompositeOperation = "lighter"
            blob(ctx, cA[0] * W, cA[1] * H, rr, p, 0.50)
            blob(ctx, cB[0] * W, cB[1] * H, rr * 0.95, s, 0.46)
            blob(ctx, cC[0] * W, cC[1] * H, rr * 0.88, a, 0.44)
            blob(ctx, cD[0] * W, cD[1] * H, rr * 0.82, c4, 0.42)
            ctx.globalCompositeOperation = "source-over"

            // Vertical shade — the shader's `1 - 0.16 * ((|y-0.5|*2)^2)` is a
            // QUADRATIC multiply, so the darkening is nearly nothing across the
            // middle and reaches 16% only at the very edges. The five stops
            // below sample that curve instead of the straight ramp the previous
            // version used (which shaded the whole upper and lower thirds).
            var v = ctx.createLinearGradient(0, 0, 0, H)
            v.addColorStop(0.00, Qt.rgba(0, 0, 0, 0.16))
            v.addColorStop(0.25, Qt.rgba(0, 0, 0, 0.04))
            v.addColorStop(0.50, Qt.rgba(0, 0, 0, 0.0))
            v.addColorStop(0.75, Qt.rgba(0, 0, 0, 0.04))
            v.addColorStop(1.00, Qt.rgba(0, 0, 0, 0.16))
            ctx.fillStyle = v
            ctx.fillRect(0, 0, W, H)
        }
    }
}
