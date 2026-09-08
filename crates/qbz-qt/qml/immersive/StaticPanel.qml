// StaticPanel — FOCUS mode 1, the calm centered now-playing card (§6.1 of
// the 2026-08-02 immersive-port contract), port of
// crates/qbz-ui/ui/immersive/StaticPanel.slint:25-111.
//
// NON-reactive: it does NOT read QbzViz.energy at all (no art-scale, no glow
// halo, no VizSettle). The ONLY motion is the 4-bar equalizer inside the
// "Now Playing" indicator — a fixed-period loop, allowed in the static panel
// (Slint header :4-9).
//
// DIFF vs AlbumReactivePanel (dropped): the energy scalars, the glow
// Rectangle, the reactive 100ms grow eases, the *1.25 breathing footprint.
// KEPT: the homologated ImmersiveTrackMeta, the centered layout (pad-top 52 /
// pad-bottom 132), spacing 20 (AlbumReactive's is 40), the static art drop
// shadow 32/#00000080/8, radius 8 (:84-103).
//
// baseArt = max(180, min(vh-364, vw*0.62, 640, native)) (:50-51) — static
// has NO breathing growth so the height budget divides by 1.0; reserve 364 =
// pad-top(52) + spacing(20) + info(160) + player clearance(132). The 132
// clearance is KEPT identical to AlbumReactivePanel (same floating player
// bar); if it changes, both move together (:33-36).

import QtQuick
import QtQuick.Effects
import QtQuick.Window
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    // D2 (contract 04 §4): size-aware large art. `artSource` prefers the
    // size-resolved large feed and falls back to the small one while it
    // resolves (the Slint `artwork-large : artwork` pattern); the probe reads
    // the SAME source so the native cap tracks the image actually shown.
    readonly property string artSource: QbzPlayer.npArtworkPathLarge !== ""
        ? QbzPlayer.npArtworkPathLarge : QbzPlayer.npArtworkPath
    // The baseArt slot expression WITHOUT the native cap — the cap stays the
    // final upscale safety; the request must reflect the SLOT so the feed can
    // serve a variant big enough for it.
    readonly property real artSlot: Math.max(180,
        Math.min(Math.min(root.height - 364, root.width * 0.62), 640))
    // Bucketed in Rust (one re-resolve per variant tier crossed, none per
    // pixel); gated on visible so a hidden panel writes nothing (pulse law).
    function requestArtSize() {
        if (root.visible)
            QbzPlayer.requestNpArtworkSize(Math.round(root.artSlot))
    }
    onArtSlotChanged: requestArtSize()
    onVisibleChanged: requestArtSize()
    Component.onCompleted: requestArtSize()

    // Native source resolution — cap so the cover is never UPSCALED beyond
    // its own size (:46-49). Hidden probe on the same file:// cache path
    // (shared image cache; see AlbumReactivePanel.qml). Empty art -> 0 ->
    // baseArt 180, as Slint.
    Image {
        id: artProbe
        visible: false
        source: root.artSource
    }
    readonly property real srcNative: artProbe.status === Image.Ready
        ? artProbe.implicitWidth : 0
    readonly property real baseArt: Math.max(180,
        Math.min(Math.min(Math.min(root.height - 364,
                                   root.width * 0.62), 640), root.srcNative))
    // Fixed size — no art-scale (:53).

    // No-shader renderer detection, verbatim from SpectrumBand.qml.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null

    // Slint VerticalLayout padding-top 52 / padding-bottom 132 /
    // alignment center / spacing 20 (:62-73).
    Item {
        anchors.fill: parent
        anchors.topMargin: 52
        anchors.bottomMargin: 132
        Column {
            anchors.centerIn: parent
            width: root.width
            spacing: 20

            // Artwork block — FIXED footprint, no *1.25 (:76-79).
            Item {
                width: root.width
                height: root.baseArt

                // Static drop shadow 32/#00000080/8 (:91-93) — the two-layer
                // CSS shadow collapses to one blur, same accepted gap as
                // AlbumReactivePanel.
                Rectangle {
                    width: root.baseArt
                    height: root.baseArt
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

                RoundedImage {
                    width: root.baseArt
                    height: root.baseArt
                    x: (parent.width - width) / 2
                    y: (parent.height - height) / 2
                    radius: 8
                    source: root.artSource
                }
            }

            // Track info (shared, homologated across the panels, :106-109).
            ImmersiveTrackMeta {
                width: root.width
            }
        }
    }
}
