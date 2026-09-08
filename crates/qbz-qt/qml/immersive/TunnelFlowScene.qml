// Tunnel Flow (immersive shader scene, mode 8 — Qt-only) — the QML wrapper
// for the C++ `TunnelFlowItem` (cxx/tunnelflow_item.*, the feedback-tunnel
// QQuickRhiItem). Block B1 of the 2026-08-15 immersive-completion contract
// (spec 02-tauri-tunnel-port.md): the legacy Tauri Canvas2D panel
// (TunnelFlowPanel.svelte) rewritten as qml/assets/shaders/tunnel_flow.frag.
//
// Its own file (the PlasmaScene.qml idiom) so the RHI item's contract is one
// screen of QML — and so the C++ type name only has to resolve when this
// file compiles (the Loader in ShaderSceneLayer.qml activates only while
// `root.active && scene === 8`).
//
// THE PULSE LAW: the item repaints ONLY from `pulseTick()`, which
// ShaderSceneLayer calls on the shared shell pulse (QbzShell.pulseMs) with
// the local scene clock and the Viz16 bars it stashed on the viz tick. The
// properties have no NOTIFY and nothing binds them.
//
// THE AUDIO STATE LIVES HERE (spec 02 §2): Tunnel Flow consumes the 16-band
// Viz16 stream, NOT the shader pack. This wrapper ports the Tauri frontend
// accumulators verbatim and runs them ONCE per pulse:
//   - smoothing s = s*0.72 + new*0.28 per band (SMOOTHING = 0.72);
//   - bass = mean(bands 0..3), mid = mean(4..9), high = mean(10..15);
//   - kick detector: bassDelta > 0.05 || highDelta > 0.05 || bass > 0.7 ->
//     kickPulse = clamp01(0.24 + bass*0.52 + high*0.34 + max(0,bassDelta)*1.8),
//     then kickPulse *= 0.9 per frame;
//   - the TAURI phase accumulator phase += 0.012 + bass*0.026 + high*0.014 +
//     kickPulse*0.018 — deliberately SEPARATE from the pack's phase.
// The reference detects the kick on the 30 Hz viz event and decays per
// requestAnimationFrame; here both run on the ~30 Hz pulse — the same
// cadence, one accumulator site.
//
// THE PALETTE rides QbzShaderScene.tunnelPaletteJson (the tunnelflow_qt.rs
// port of extractLinePaletteFromArtwork, published per track): stashed on
// the notify (zero-dirty — nothing binds the stash), applied on the pulse
// edge like every other uniform.

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz

TunnelFlowItem {
    id: root

    // mirrorVertically: mirror on Vulkan/D3D, NOT on OpenGL and NOT on Metal.
    // This line used to read "Vulkan/Metal/D3D" — Metal had never been run and
    // was assumed to follow Vulkan because it is not OpenGL; on the Mac it
    // flipped. The empirical matrix, and why the rule stays a negation, are in
    // LineBedScene.qml.
    // The feedback passes are self-consistent in texture space; this only
    // affects the composite.
    // Metal sides with OpenGL (2026-08-19) — the full matrix and the reason
    // this stays a negation are in LineBedScene.qml.
    mirrorVertically: GraphicsInfo.api !== GraphicsInfo.OpenGL
                      && GraphicsInfo.api !== GraphicsInfo.Metal

    // Fade-in 240 ms on activation (spec 02 §5; TunnelFlowPanel.svelte
    // :1005-1024) — a TRANSIENT one-shot, not a continuous animator, so the
    // pulse law allows it.
    opacity: 0
    Component.onCompleted: fadeIn.start()
    NumberAnimation {
        id: fadeIn
        target: root
        property: "opacity"
        from: 0
        to: 1
        duration: 240
    }

    // --- The JS audio accumulators (see the header) -------------------------
    property var smooth: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    property real tfPhase: 0
    property real kickPulse: 0
    property real prevBass: 0
    property real prevHigh: 0

    // The palette stash (zero-dirty, the pendingPack pattern).
    property var paletteStash: null

    Connections {
        target: QbzShaderScene
        function onTunnelPaletteJsonChanged() {
            try {
                root.paletteStash = JSON.parse(QbzShaderScene.tunnelPaletteJson)
            } catch (e) {
                // A malformed palette keeps the previous colors.
            }
        }
    }

    // Called by ShaderSceneLayer on the pulse edge ONLY. `timeS` is the
    // layer's local pulse clock (seconds); `bars` is the stashed Viz16 array
    // or null when the viz drain published nothing new (player paused) — the
    // repaint still runs so the feedback decay carries the motion (the idle
    // drift: phase advances 0.012/pulse minimum, spec 02 §6).
    function pulseTick(timeS, bars) {
        var s = root.smooth
        if (bars !== null && bars.length >= 16) {
            for (var i = 0; i < 16; i++)
                s[i] = s[i] * 0.72 + bars[i] * 0.28
            // The kick detector fires on FRESH viz data only (the reference
            // runs it in the viz-event handler): with the feed parked the
            // deltas are stale and a held bass > 0.7 would retrigger forever.
            var bass = (s[0] + s[1] + s[2] + s[3]) / 4
            var high = (s[10] + s[11] + s[12] + s[13] + s[14] + s[15]) / 6
            var bassDelta = bass - root.prevBass
            var highDelta = high - root.prevHigh
            root.prevBass = bass
            root.prevHigh = high
            if (bassDelta > 0.05 || highDelta > 0.05 || bass > 0.7) {
                root.kickPulse = Math.max(0, Math.min(1,
                    0.24 + bass * 0.52 + high * 0.34 + Math.max(0, bassDelta) * 1.8))
            }
        }
        var bass = (s[0] + s[1] + s[2] + s[3]) / 4
        var mid = (s[4] + s[5] + s[6] + s[7] + s[8] + s[9]) / 6
        var high = (s[10] + s[11] + s[12] + s[13] + s[14] + s[15]) / 6

        // The palette, applied on the pulse edge like the uniforms.
        var pal = root.paletteStash
        if (pal !== null && pal.length >= 4) {
            palette0 = pal[0]
            palette1 = pal[1]
            palette2 = pal[2]
            palette3 = pal[3]
        }

        // Uniforms use the CURRENT phase/kick (the reference draws, then
        // advances the accumulators at the end of render()).
        time = timeS
        phase = root.tfPhase
        root.bass = bass
        root.mid = mid
        root.high = high
        kick = root.kickPulse

        root.tfPhase += 0.012 + bass * 0.026 + high * 0.014 + root.kickPulse * 0.018
        root.kickPulse *= 0.9
        update()
    }
}
