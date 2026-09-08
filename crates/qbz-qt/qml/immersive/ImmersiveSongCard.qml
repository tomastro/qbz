// ImmersiveSongCard — the minimal bottom-right metadata card (§6.7 of the
// 2026-08-02 immersive-port contract), port of
// crates/qbz-ui/ui/immersive/ImmersiveSongCard.slint:15-114 (divergence D6:
// built fresh — shell/SongCard.qml is the bar's INTERACTIVE meta-link card
// with AudioStamp; this twin is NON-interactive with the E chip).
//
// Mounted by ImmersiveView as layer 4 over Wave Bed, full Lyrics, both scopes
// and every shader scene, bottom-right with 24px insets. It does NOT fade with
// the auto-hide chrome. Content-sized: the card hugs its content
// and the mount pins it bottom-right, so a wider card grows LEFTWARD.
//
//   meta column (right-aligned): title 15px semibold capped 420px + elide
//     (:33-45) + "E" explicit chip 16px (:47-60); subline album italic ·
//     artist 12px, NEVER truncated (:63-84); QualityBadgeFull showIcon:true
//     (:89-93).
//   cover thumb (RIGHT): 72x72 radius 6, shadow 16/#00000080/4 (:97-105).
//
// EXPLICIT SOURCE: Slint reads NowPlayingState.explicit; QbzPlayer publishes
// NO np_explicit (player_bridge.rs:31-114). The queue document's `current`
// row carries the SAME datum (QueueTrack::parental_warning ->
// queue_qt.rs:47,194), so the chip reads QbzQueue.queueJson with the
// guarded try/catch + track-id guard of PlayerBar.qml's npFavorite/npSource
// (the id guard keeps a stale queue document from lending the PREVIOUS
// track's chip to this one).

import QtQuick
import QtQuick.Effects
import QtQuick.Window
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    // No-shader renderer detection, verbatim from SpectrumBand.qml (effects
    // draw NOTHING on the software/VNC path — the layer is dropped there and
    // the plain shadow rect remains).
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null

    readonly property bool npExplicit: {
        try {
            var d = JSON.parse(QbzQueue.queueJson)
            return !!(d && d.current && d.current.id === QbzPlayer.npTrackId
                      && d.current.explicit)
        } catch (e) {
            return false
        }
    }

    // Content-sized (:18-19): meta column width + 12px spacing + 72px cover.
    readonly property real metaW: Math.max(titleRow.implicitWidth,
                                           subRow.implicitWidth,
                                           badge.implicitWidth)
    readonly property real metaH: titleRow.height + 3 + subRow.height
                                  + 3 + badge.implicitHeight
    implicitWidth: metaW + 12 + 72
    implicitHeight: Math.max(metaH, 72)
    width: implicitWidth
    height: implicitHeight

    // --- Meta column (LEFT), every row right-aligned ----------------------
    // Title + inline E chip (:30-61). Slint HorizontalLayout alignment: end.
    Row {
        id: titleRow
        x: root.metaW - width
        spacing: 6
        height: Math.max(titleText.implicitHeight, 16)
        Text {
            id: titleText
            // Capped + elided (:34); implicitWidth is the natural content
            // width, so this is NOT a binding loop.
            width: Math.min(implicitWidth, 420)
            text: QbzPlayer.npTitle
            color: theme.textPrimary
            font.pixelSize: 15
            font.weight: Font.DemiBold
            elide: Text.ElideRight
            horizontalAlignment: Text.AlignRight
        }
        // Explicit "E" chip — 1:1 with TrackRow's badge (:47-60).
        Rectangle {
            visible: root.npExplicit
            width: visible ? 16 : 0
            height: 16
            radius: 3
            color: theme.surfaceElevated
            Text {
                anchors.centerIn: parent
                text: "E"
                color: theme.textMuted
                font.pixelSize: 9
                font.weight: Font.DemiBold
            }
        }
    }

    // Subline: album (italic) · artist — FULL, never truncated (:63-84).
    Row {
        id: subRow
        x: root.metaW - width
        y: titleRow.height + 3
        spacing: 4
        height: 14
        Text {
            visible: QbzPlayer.npAlbum !== ""
            text: QbzPlayer.npAlbum
            color: "#80ffffff"
            font.pixelSize: 12
            font.italic: true
        }
        Text {
            visible: QbzPlayer.npAlbum !== "" && QbzPlayer.npArtist !== ""
            text: "·"
            color: "#66ffffff"
            font.pixelSize: 12
        }
        Text {
            visible: QbzPlayer.npArtist !== ""
            text: QbzPlayer.npArtist
            color: "#99ffffff"
            font.pixelSize: 12
        }
    }

    // Quality badge (full mode, icon shown), right-aligned (:86-94).
    QualityBadgeFull {
        id: badge
        x: root.metaW - width
        y: titleRow.height + 3 + subRow.height + 3
        // NO overAmbient here (owner, 2026-08-31 round 3): this card only
        // mounts over dark surfaces (Wave Bed, lyrics, scopes, shader
        // scenes) and the dark chip read as a bigger badge. The themed look
        // was already right.
        tier: QbzPlayer.npQualityTier
        detail: QbzPlayer.npQualityDetail
        showIcon: true
    }

    // --- Cover thumb (RIGHT) — 72x72 radius 6, shadow 16/#00000080/4 ------
    // The drop shadow is a dark rounded rect blurred by a MultiEffect (the
    // Qt twin of Slint's drop-shadow-*; no shaders -> plain offset rect).
    Rectangle {
        x: root.width - 72
        y: 4
        width: 72
        height: 72
        radius: 6
        color: "#80000000"
        layer.enabled: !root._noShaders
        layer.effect: MultiEffect {
            blurEnabled: true
            blurMax: 16
            blur: 1.0
        }
    }
    RoundedImage {
        x: root.width - 72
        width: 72
        height: 72
        radius: 6
        source: QbzPlayer.npArtworkPath
    }
}
