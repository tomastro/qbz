// ImmersiveTrackMeta — the homologated immersive track-info block (§6.7 of
// the 2026-08-02 immersive-port contract), port of ImmersiveTrackInfo in
// crates/qbz-ui/ui/immersive/ImmersiveTrackInfo.slint:78-143.
//
// HOMOLOGATED look/spacing across AlbumReactive / Static / Coverflow (and the
// SPLIT left column, §5.6): spacing 6px, the "Now Playing" indicator padded
// 8px below, the quality badge padded 12px above, fonts 28/18/14.
//
// Every row reads QbzPlayer.np* (the Slint block reads NowPlayingState):
//   "Now Playing" + EqualizerBars — VISIBLE ONLY WHILE npPlaying (:85,:100)
//   title 28px/700 elide center (:106-113)
//   artist 18px #ffffffb3 elide center (:116-122)
//   album 14px italic #ffffff80, only when non-empty (:125-132)
//   QualityBadgeFull (tier + detail), self-hides when tier == "" (:135-141)
//
// The Slint root is a VerticalLayout with alignment: center — the Qt twin is
// a Column whose width the MOUNT sets (every B3 mount passes the panel width)
// and whose rows center themselves horizontally.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Column {
    id: root

    QbzTheme { id: theme }

    /// Equalizer tint + "Now Playing" label follow the album palette, not
    /// the theme accent: the theme color has no contrast guarantee over the
    /// ambient backdrop (2026-08-31 visual cleanup — homologated with the
    /// cinematic split card's waveform/seek accent).
    AmbientAccent { id: ambientAccent }
    property color equalizerTint: ambientAccent.value

    /// The block sits directly on the ambient/blurred-cover field, which can
    /// be near-white for light covers — every text row carries the same
    /// restrained native shadow the lyrics surfaces use, so the fixed light
    /// colors stay legible over ANY artwork.
    readonly property color _shadow: "#b0000000"

    spacing: 6

    // "Now Playing" indicator: equalizer bars + label, shown only while
    // playing (:85-103). The Slint HorizontalLayout carries padding-bottom 8
    // — the wrapper Item's +8 height is that padding.
    Item {
        visible: QbzPlayer.npPlaying
        width: parent.width
        height: visible ? npRow.implicitHeight + 8 : 0
        Row {
            id: npRow
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 8
            height: 14
            EqualizerBars {
                tint: root.equalizerTint
                // The gate lives at the mount (EqualizerBars.qml explains
                // why): this host sits inside the always-mounted ImmersiveView.
                active: QbzImmersive.open
                anchors.verticalCenter: parent.verticalCenter
            }
            Text {
                text: QbzSession.tr("Now Playing", QbzSession.trRev)
                color: root.equalizerTint
                font.pixelSize: 12
                font.weight: Font.DemiBold
                font.letterSpacing: 0.5
                style: Text.Raised
                styleColor: root._shadow
                anchors.verticalCenter: parent.verticalCenter
            }
        }
    }

    // Track title (:106-113).
    Text {
        width: parent.width
        text: QbzPlayer.npTitle
        color: "#ffffff"
        font.pixelSize: 28
        font.weight: Font.Bold
        style: Text.Raised
        styleColor: root._shadow
        horizontalAlignment: Text.AlignHCenter
        elide: Text.ElideRight
    }

    // Artist (:116-122). Slint #ffffffb3 (RRGGBBAA) -> #b3ffffff.
    Text {
        width: parent.width
        text: QbzPlayer.npArtist
        color: "#d9ffffff"
        font.pixelSize: 18
        style: Text.Raised
        styleColor: root._shadow
        horizontalAlignment: Text.AlignHCenter
        elide: Text.ElideRight
    }

    // Album (italic, dimmer), only when present (:125-132).
    Text {
        visible: QbzPlayer.npAlbum !== ""
        width: parent.width
        text: QbzPlayer.npAlbum
        color: "#bfffffff"
        font.pixelSize: 14
        font.italic: true
        style: Text.Raised
        styleColor: root._shadow
        horizontalAlignment: Text.AlignHCenter
        elide: Text.ElideRight
    }

    // Quality badge, centered, padding-top 12 (:135-141). Self-hides when
    // tier == "" (QualityBadgeFull's own gate) — the wrapper collapses with
    // it so the 12px pad vanishes too.
    Item {
        visible: QbzPlayer.npQualityTier !== ""
        width: parent.width
        height: visible ? badge.implicitHeight + 12 : 0
        QualityBadgeFull {
            id: badge
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.bottom
            overAmbient: true
            tier: QbzPlayer.npQualityTier
            detail: QbzPlayer.npQualityDetail
        }
    }
}
