// SpectrumPanel — FOCUS mode 3, the 28-column mirrored spectrum (§6.1 of
// the 2026-08-02 immersive-port contract), port of
// crates/qbz-ui/ui/immersive/SpectrumPanel.slint:110-251.
//
// OPAQUE #000 background (:118) — intended faithful behavior: the panel
// HIDES the atmosphere underlay behind it (§5.1, SpectrumPanel.slint:45-46).
//
// GEOMETRY (:120-131): 28 visual columns = 14 ACTIVE bins [0,2,3,...,14]
// (the 16-bin FFT leaves {1, 15} empty) x 2 mirror about the vertical
// center. slot = vw/28, bar = slot-4, x = col*slot + 2, baseY = vh*0.85
// (bars grow UP), maxH = vh*0.7. Columns 0..13 read activeBins[col], columns
// 14..27 replay them mirrored (activeBins[27-col]); both read the SAME
// source bin. The discrete-cubelet texture of the Svelte original is dropped
// — ONE solid gradient bar per column, exactly as the Slint port resolved
// the retained-node churn (:62-70).
//
// FFT CONSUMPTION (trap 6): heights bind through ONE VizSettle on
// QbzViz.bars (bars arrive pre-smoothed + pre-clamped from the Rust EMA —
// no second smoothing here, :34-42). The inter-frame settle is VizSettle's
// job (the Slint `animate bar-height 90ms` equivalent, :84).
//
// COLORS: vertical gradient QbzShell.ambientPrimary (base) ->
// ambientSecondary (tip) — the Slint reads ImmersiveState
// spectrum-primary/secondary; Qt's ambient triad is the same album-derived
// pipeline (SpectrumBand.qml:109-113 precedent). Slint's gradient is 0deg
// (base -> tip), so the Qt stops are INVERTED: position 0 (top) = secondary,
// position 1 (bottom) = primary. ONE shared Gradient instance for every bar.
//
// NO PEAK CAPS (peak-hold belongs to SpectralRibbon, :31). NO opacity on any
// container holding the positioned bars.

import QtQuick
import QtQuick.Effects
import QtQuick.Window
import com.blitzfc.qbz
import "../controls"
import "../shell"
import "../theme"

Rectangle {
    id: root

    color: "#000000"

    // 14 active SOURCE bins after excluding {1, 15} (:131).
    readonly property var activeBins: [0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
    readonly property real slot: root.width / 28
    readonly property real maxBarHeight: root.height * 0.7

    // ONE VizSettle for the whole stream (trap 6, :146-148).
    VizSettle {
        id: barsSettle
        target: QbzViz.bars
        live: root.visible && QbzImmersive.open
    }

    // ONE gradient shared by every bar (perf note 3 of SpectrumBand.qml).
    Gradient {
        id: barGradient
        GradientStop { position: 0.0; color: QbzShell.ambientSecondary }
        GradientStop { position: 1.0; color: QbzShell.ambientPrimary }
    }

    // --- Bars (drawn FIRST, behind the song card, :133-149) ----------------
    // STATIC Repeater model (28 columns); only each bar's HEIGHT binds to the
    // stream. A missing/short frame reads 0 — flat baseline, no
    // index-out-of-range (:134-137).
    Repeater {
        model: 28
        delegate: Rectangle {
            required property int index
            readonly property int bin: index < 14
                ? root.activeBins[index] : root.activeBins[27 - index]
            x: index * root.slot + 2
            width: root.slot - 4
            height: barsSettle.at(bin) * root.maxBarHeight
            anchors.bottom: parent.bottom
            // baseY = vh*0.85 -> 15% bottom margin.
            anchors.bottomMargin: root.height * 0.15
            gradient: barGradient
        }
    }

    // --- Song card (drawn SECOND, in front of the bars, :151-249) ----------
    // Artwork smaller than the Static/Album card (secondary to the bars):
    // max(160, min(vh*0.45, vw*0.5, 360)) (:161-162). Player clearance 132
    // KEPT IDENTICAL to the siblings (:157). NO "Now Playing" indicator here
    // (the Spectrum info block is its own, :205-249).
    readonly property real artSize: Math.max(160,
        Math.min(Math.min(root.height * 0.45, root.width * 0.5), 360))

    // D2 (contract 04 §4): size-aware large art — this card draws up to
    // 360 CSS px, past what the thumbnail-tier small feed covers. Same
    // bucketed request + fallback binding as StaticPanel/AlbumReactivePanel.
    readonly property string artSource: QbzPlayer.npArtworkPathLarge !== ""
        ? QbzPlayer.npArtworkPathLarge : QbzPlayer.npArtworkPath
    function requestArtSize() {
        if (root.visible)
            QbzPlayer.requestNpArtworkSize(Math.round(root.artSize))
    }
    onArtSizeChanged: requestArtSize()
    onVisibleChanged: requestArtSize()
    Component.onCompleted: requestArtSize()

    Item {
        anchors.fill: parent
        anchors.topMargin: 70
        anchors.bottomMargin: 132
        Column {
            anchors.centerIn: parent
            width: root.width
            spacing: 20

            // Artwork + halo shadow 36/#00000099/8 (:175-202).
            Item {
                width: root.width
                height: root.artSize
                Rectangle {
                    width: root.artSize
                    height: root.artSize
                    x: (parent.width - width) / 2
                    y: (parent.height - height) / 2 + 8
                    radius: 8
                    color: "#99000000"
                    layer.enabled: !root._noShaders
                    layer.effect: MultiEffect {
                        blurEnabled: true
                        blurMax: 36
                        blur: 1.0
                    }
                }
                RoundedImage {
                    width: root.artSize
                    height: root.artSize
                    x: (parent.width - width) / 2
                    y: (parent.height - height) / 2
                    radius: 8
                    source: root.artSource
                }
            }

            // Track info (:205-249). Explicit badge OMITTED — Slint has no
            // `explicit` in NowPlayingState here either (:209-210).
            Column {
                width: root.width
                spacing: 6
                Text {
                    width: parent.width
                    text: QbzPlayer.npTitle
                    color: "#ffffff"
                    font.pixelSize: 28
                    font.weight: Font.Bold
                    horizontalAlignment: Text.AlignHCenter
                    elide: Text.ElideRight
                }
                Text {
                    width: parent.width
                    text: QbzPlayer.npArtist
                    color: "#b3ffffff"
                    font.pixelSize: 18
                    horizontalAlignment: Text.AlignHCenter
                    elide: Text.ElideRight
                }
                Text {
                    visible: QbzPlayer.npAlbum !== ""
                    width: parent.width
                    text: QbzPlayer.npAlbum
                    color: "#80ffffff"
                    font.pixelSize: 14
                    font.italic: true
                    horizontalAlignment: Text.AlignHCenter
                    elide: Text.ElideRight
                }
                // Quality badge, self-hides when tier == "", padding-top 12.
                Item {
                    visible: QbzPlayer.npQualityTier !== ""
                    width: parent.width
                    height: visible ? badge.implicitHeight + 12 : 0
                    QualityBadgeFull {
                        id: badge
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.bottom: parent.bottom
                        tier: QbzPlayer.npQualityTier
                        detail: QbzPlayer.npQualityDetail
                    }
                }
            }
        }
    }

    // No-shader renderer detection, verbatim from SpectrumBand.qml.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null
}
