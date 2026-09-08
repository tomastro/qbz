// AlbumReactivePanel — FOCUS mode 0, the energy-reactive now-playing card
// (§6.1 of the 2026-08-02 immersive-port contract), port of
// crates/qbz-ui/ui/immersive/AlbumReactivePanel.slint:43-178.
//
// REACTIVE SCALARS (contract formulas, Svelte lineage in the Slint header):
//   globalEnergy = avg(energy[0..5])            (:50-55)
//   bassEnergy   = avg(energy[0], energy[1])    (:56-59)
//   artScale     = 1 + global * 0.25            (:62)
//   glowSpread   = 15 + bass * 200  (px -> blur)(:64)
//   glowOpacity  = min(0.1 + global * 1.5, 1)   (:66)
//   baseArt = max(180, min((vh-384)/1.25, vw*0.62, 660, native))  (:88-89)
// The reserve constant 384 = pad-top(52) + spacing(40) + info(~160) +
// player clearance(132) — keep it in sync with the paddings below (:106-112).
//
// FFT CONSUMPTION (trap 6): energy arrives through ONE VizSettle on
// QbzViz.energy — NEVER a Repeater.model on a QbzViz list. The scalars apply
// DIRECTLY at the pulse rate: the 100ms eases the Slint panel uses to
// reproduce the Svelte EMA (:138-139,:160) were dropped 2026-08-13 — a
// Behavior retriggered at 30 Hz animates at display rate, and every
// animation frame is a whole-window present (measured: 132 presents/s, 83%
// GPU, immersive open). The Rust EMA already provides the smoothing.
//
// SPEAKER-CONE PULSE: the art grows from its CENTER via animated
// width/height; x/y are PLAIN bindings ((parent - size)/2), NEVER animated —
// animating x/y puts the centering offset on its own timeline and the box
// drifts sideways mid-pulse (the Slint header documents the bug, :126-131).
//
// GLOW FIDELITY: Slint drop-shadow has blur only (CSS's blur+spread already
// collapsed there). The Qt twin renders the halo as the glow-colored rounded
// rect under ONE MultiEffect blur. MultiEffect's blur ceiling (blurMax 64)
// caps the halo below Slint's 15+bass*200 px — the halo reads tighter under
// heavy bass; blur+opacity dominate the look (the same accepted gap the
// Slint port flags, :32-34). No shaders (software/VNC) -> the layer drops
// and the plain rects render unblurred.

import QtQuick
import QtQuick.Effects
import QtQuick.Window
import com.blitzfc.qbz
import "../shell"
import "../theme"

Item {
    id: root

    // --- Energy (ONE VizSettle per stream, trap 6) -------------------------
    VizSettle {
        id: energySettle
        target: QbzViz.energy
        // Park the driver while the panel is not on screen or the overlay is
        // closed (the B1 close edge also stops the publishes; this is the
        // QML half of the gate).
        live: root.visible && QbzImmersive.open
    }
    readonly property real globalEnergy: {
        // Read the SETTLE, never QbzViz.energy: a binding that reads the
        // bridge list re-evaluates on every 30 Hz publish — OFF the pulse —
        // and this one feeds geometry (artScale/glow), so each publish was a
        // whole-window present of its own (measured: the immersive ran at
        // 57 presents/s with 1 ms-apart pairs, 2026-08-13).
        // `cur` since VizSettle eases instead of stepping — same job (have we
        // been handed a frame yet?) and the same dependency, because `cur` is
        // the property the pulse reassigns.
        if (energySettle.cur.length < 5)
            return 0.0
        return (energySettle.at(0) + energySettle.at(1) + energySettle.at(2)
                + energySettle.at(3) + energySettle.at(4)) / 5.0
    }
    readonly property real bassEnergy: {
        if (energySettle.cur.length < 2)
            return 0.0
        return (energySettle.at(0) + energySettle.at(1)) / 2.0
    }

    // --- Reactive formulas (:62-66) ----------------------------------------
    // NO 100ms Behaviors here (2026-08-13): a Behavior retriggered by 30 Hz
    // data animates at DISPLAY rate, and every animation frame is a
    // whole-window present — measured with the immersive open: ~132
    // presents/s and 83% GPU, versus ~33/s when these apply directly. The
    // Rust EMA (attack 0.7 / decay 0.65) already band-limits the stream to
    // ~6 Hz, so the values step at the pulse rate and still read smooth.
    // The Slint `animate 100ms` is unaffordable in Qt Quick — its ease only
    // repaints the rect; ours presents the window.
    readonly property real artScale: 1.0 + root.globalEnergy * 0.25
    property real glowSpread: 15.0 + root.bassEnergy * 200.0
    property real glowOpacity: Math.min(0.1 + root.globalEnergy * 1.5, 1.0)

    // --- Base art box (:84-92) ---------------------------------------------
    // D2 (contract 04 §4): size-aware large art. `artSource` prefers the
    // size-resolved large feed and falls back to the small one while it
    // resolves (the Slint `artwork-large : artwork` pattern); the probe reads
    // the SAME source so the native cap tracks the image actually shown.
    readonly property string artSource: QbzPlayer.npArtworkPathLarge !== ""
        ? QbzPlayer.npArtworkPathLarge : QbzPlayer.npArtworkPath
    // The baseArt slot expression WITHOUT the native cap — the request must
    // reflect the SLOT so the feed can serve a variant big enough for it.
    readonly property real artSlot: Math.max(180,
        Math.min(Math.min((root.height - 384) / 1.25, root.width * 0.62), 660))
    // Bucketed in Rust (one re-resolve per variant tier crossed, none per
    // pixel); gated on visible so a hidden panel writes nothing (pulse law).
    function requestArtSize() {
        if (root.visible)
            QbzPlayer.requestNpArtworkSize(Math.round(root.artSlot))
    }
    onArtSlotChanged: requestArtSize()
    onVisibleChanged: requestArtSize()
    Component.onCompleted: requestArtSize()

    // Native source resolution — cap the RESTING art so the cover is never
    // UPSCALED beyond its own size (owner: oversized blurry art looked
    // nefasto); the audio breathing still grows it momentarily. Slint reads
    // the decoded image's width; Qt's twin is a hidden probe Image on the
    // same file:// cache path (the shared image cache makes the second
    // decode free). Unloaded/empty art -> 0 -> baseArt 180, as Slint.
    Image {
        id: artProbe
        visible: false
        source: root.artSource
    }
    readonly property real srcNative: artProbe.status === Image.Ready
        ? artProbe.implicitWidth : 0
    readonly property real baseArt: Math.max(180,
        Math.min(Math.min(Math.min((root.height - 384) / 1.25,
                                   root.width * 0.62), 660), root.srcNative))
    readonly property real artSize: root.baseArt * root.artScale

    // No-shader renderer detection, verbatim from SpectrumBand.qml.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null

    // --- Layout: artwork block + track info, centered as a group -----------
    // Slint VerticalLayout padding-top 52 / padding-bottom 132 /
    // alignment center / spacing 40 (:101-112).
    Item {
        anchors.fill: parent
        anchors.topMargin: 52
        anchors.bottomMargin: 132
        Column {
            anchors.centerIn: parent
            width: root.width
            spacing: 40

            // Artwork block, sized to the largest reactive footprint so the
            // growth doesn't clip (:115-118).
            Item {
                width: root.width
                height: root.baseArt * 1.25

                // Static black art shadow 32/#00000080/8 (:157-159).
                // NO 100ms width/height Behaviors (2026-08-13) — a Behavior
                // retriggered by 30 Hz data animates at display rate, and
                // each animation frame is a whole-window present. The art
                // pulse steps at the pulse rate; see the note by the
                // reactive formulas above.
                Rectangle {
                    width: root.artSize
                    height: root.artSize
                    x: (parent.width - width) / 2
                    y: (parent.height - height) / 2 + 8
                    radius: 8
                    color: "#80000000"
                    layer.enabled: !root._noShaders
                    layer.effect: MultiEffect {
                        blurEnabled: true
                        blurMax: 32
                        blur: 1.0
                    }
                }

                // Glow halo (:123-140): same size/position as the art, glow
                // color + reactive opacity, blurred. x/y NOT animated (see
                // the header's speaker-cone note). No size Behaviors either
                // (see the reactive-formulas note).
                Rectangle {
                    width: root.artSize
                    height: root.artSize
                    x: (parent.width - width) / 2
                    y: (parent.height - height) / 2
                    radius: 8
                    color: QbzImmersive.glowColor
                    opacity: root.glowOpacity
                    layer.enabled: !root._noShaders
                    layer.effect: MultiEffect {
                        blurEnabled: true
                        blurMax: 64
                        blur: Math.min(1, root.glowSpread / 64)
                    }
                }

                // Artwork (:147-170). Grows from center via width/height
                // bindings applied at the pulse rate; x/y plain bindings
                // (lockstep, zero drift). No Behaviors (see above).
                Item {
                    width: root.artSize
                    height: root.artSize
                    x: (parent.width - width) / 2
                    y: (parent.height - height) / 2
                    RoundedImage {
                        anchors.fill: parent
                        radius: 8
                        source: root.artSource
                    }
                }
            }

            // Track info (shared, homologated across the panels, :173-176).
            ImmersiveTrackMeta {
                width: root.width
            }
        }
    }
}
