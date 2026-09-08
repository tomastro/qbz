// EqualizerBars — the 4-bar "now playing" equalizer (§6.7 of the
// 2026-08-02 immersive-port contract), port of the EqualizerBars component
// in crates/qbz-ui/ui/immersive/ImmersiveTrackInfo.slint:23-74.
//
// 18x14 box, four 3px bars at 2px spacing (manual x: 0/5/10/15 — a Row
// positioner would own the children's y and the bars must bottom-align by
// their OWN animated height). Base heights 60/100/40/80% of the 14px box,
// each sweeping scaleY 0.3..1.0 on the shared 800ms loop with phase offsets
// 0 / 0.25 / 0.125 / 0.375 (the Svelte animation-delays 0.0/0.2/0.1/0.3 of
// the 0.8s period, as fractions).
//
// The Slint loop is pull-based off animation-tick(); the Qt twin accumulates
// `t` on the shared shell pulse (see the note at the property), so the whole
// equalizer ticks at the shell's 30 Hz instead of the display rate. The wave
// is the Slint formula verbatim:
//   0.3 + 0.7 * (0.5 - 0.5*cos((t+phase mod 1) * 2π))
// (Svelte keyframes 0/100% -> 0.3, 50% -> 1.0). The loop runs only while the
// item is visible AND the immersive is open — every mount site gates
// visibility on npPlaying (ImmersiveTrackInfo.slint:85,:100), so a paused
// player animates nothing.
// Reduce-motion: the seam EXISTS now — `QbzShell.reduceMotion`, built by the
// cortinilla-parity contract (§5.g). This surface is simply not a consumer
// yet; wiring it is a one-liner (throttle the tick when it is true) and its
// own change, not a side effect of someone else's commit.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    /// Bar color follows the active theme unless a host deliberately overrides it.
    property color tint: theme.accent

    /// Proportional size multiplier over the nominal 18x14 box — the
    /// cinematic split card mounts the indicator at 1.5x. Bar geometry
    /// (x 0/5/10/15, width 3) scales with it so the silhouette is identical.
    property real barScale: 1.0

    width: 18 * barScale
    height: 14 * barScale
    implicitWidth: 18 * barScale
    implicitHeight: 14 * barScale

    // Loop phase 0..1, 800ms (ImmersiveTrackInfo.slint:32). Advanced on the
    // SHELL PULSE (QbzShell.pulseMs), not a NumberAnimation: an 800ms
    // infinite animation is serviced by the animation driver at DISPLAY rate
    // (180 Hz on the owner's panel) and every update is a whole-window
    // present. The wave's period is 800 ms, so the 30 Hz pulse samples it
    // ~24 times per loop — indistinguishable. Riding the pulse also means
    // ZERO extra clocks: the bars move in the same frame as everything else
    // the pulse moves. A pulse receiver still has CPU cost, though, so an
    // inactive indicator disconnects structurally instead of waking a JS
    // handler just to return. TrackRow also loads this object only for the
    // one active, visible play cell; a long expanded box set therefore owns
    // one receiver rather than one receiver per track.
    //
    // The `active` gate belongs to the HOST, because this component mounts
    // in always-alive-but-invisible trees (the immersive overlay) where a
    // local `visible` check alone keeps ticking with the overlay closed —
    // and invisible geometry changes still dirty the whole window (the
    // 2026-08-13 SpectrumPanel finding). ImmersiveTrackMeta passes
    // `QbzImmersive.open`; the track row passes its playing-row predicate.
    // `_tickMs` must match settings_qt::shell_pulse_ms (default 33) — a
    // mismatch only mis-speeds the loop, never the present rate.
    property bool active: true
    property real t: 0
    readonly property int _tickMs: 33
    Connections {
        target: root.active && root.visible ? QbzShell : null
        function onPulseMsChanged() {
            root.t = (root.t + root._tickMs / 800.0) % 1.0
        }
    }

    // Slint wave() verbatim (:34-36).
    function wave(phase) {
        var u = (root.t + phase) % 1.0
        return 0.3 + 0.7 * (0.5 - 0.5 * Math.cos(u * 6.283185))
    }

    // bar 1: delay 0.0, base 60% (:42-48)
    Rectangle {
        x: 0
        width: 3 * root.barScale
        y: root.height - height
        height: root.height * 0.6 * root.wave(0.0)
        radius: root.barScale
        color: root.tint
    }
    // bar 2: delay 0.2, base 100% (:50-56)
    Rectangle {
        x: 5 * root.barScale
        width: 3 * root.barScale
        y: root.height - height
        height: root.height * 1.0 * root.wave(0.25)
        radius: root.barScale
        color: root.tint
    }
    // bar 3: delay 0.1, base 40% (:58-64)
    Rectangle {
        x: 10 * root.barScale
        width: 3 * root.barScale
        y: root.height - height
        height: root.height * 0.4 * root.wave(0.125)
        radius: root.barScale
        color: root.tint
    }
    // bar 4: delay 0.3, base 80% (:66-72)
    Rectangle {
        x: 15 * root.barScale
        width: 3 * root.barScale
        y: root.height - height
        height: root.height * 0.8 * root.wave(0.375)
        radius: root.barScale
        color: root.tint
    }
}
