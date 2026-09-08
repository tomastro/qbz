// KioskCard — the lightweight album card for the kiosk browse shelves
// (shell/KioskCard.slint). Deliberately SIMPLER than cards/AlbumCard.qml:
// square art + a quality badge + title + artist, tap-to-open. No hover-reveal
// actions, no context menu, no source/ribbon/award chrome, no pin badge, no
// favourite heart, no selection checkbox, no hover scale or scrim — that
// weight is what made the reused desktop views too heavy for a Pi. The only
// interaction is a full-surface tap.
//
// `album` is a plain JS object carrying { id, title, artist, artwork,
// qualityTier }. `artwork` is the RESOLVED local cover path: the Slint reads
// AlbumCardItem.artwork, which the artwork pipeline pushes into the shared
// model in place; the Qt documents carry only the remote `artUrl`, so the
// HOSTING VIEW resolves it and hands the local path down under `artwork`.
//
// Height is INTRINSIC (art + 7 + title line + 7 + artist line). The grid slot
// allocates artSize + 46 and the column's top alignment leaves the slack at
// the bottom, exactly as the Slint VerticalLayout's `alignment: start` does.
//
// ARTWORK: KioskArtwork, not theme/RoundedImage. The desktop path measures the
// original before it can ask Rust for a derivative, i.e. it can decode a 600px
// cover into a 96px kiosk cell first — the exact thing contract §5.2 says
// "usa RoundedImage" does not close. KioskArtwork pins `sourceSize` to this
// tile's physical box instead and mounts no probe. See its header.

import QtQuick
import "../theme"

Rectangle {
    id: root

    property var album: ({})
    property real artSize: 128
    /// Circular art + centred text. No kiosk view sets it today; it is a
    /// declared parameter of the primitive in the Slint (KioskCard.slint:15)
    /// and drives the art radius and both text alignments.
    property bool round: false
    /// Keyboard/gamepad nav: the hosting view sets this when the shell focus
    /// index lands on this card. A 3px accent ring + tint, readable at
    /// 10-foot distance.
    property bool navFocused: false
    signal clicked(string id)

    /// The cover is DECODED and on screen. A host that lays a KioskSkeleton
    /// over a cell gates it on this, never on "the path is non-empty": the
    /// path landing only means the decode has started.
    readonly property bool artReady: art.ready

    QbzTheme { id: theme }

    readonly property string _title: root.album && root.album.title ? root.album.title : ""
    readonly property string _artist: root.album && root.album.artist ? root.album.artist : ""
    readonly property string _artwork: root.album && root.album.artwork ? root.album.artwork : ""
    readonly property string _tier: root.album && root.album.qualityTier ? root.album.qualityTier : ""

    implicitWidth: root.artSize
    width: root.artSize
    implicitHeight: body.implicitHeight
    border.width: root.navFocused ? 3 : 0
    border.color: theme.accent
    radius: theme.radiusSm
    color: root.navFocused
        ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.12)
        : "transparent"

    Column {
        id: body
        width: root.artSize
        spacing: 7

        Rectangle {
            id: artTile
            width: root.artSize
            height: root.artSize
            radius: root.round ? root.artSize / 2 : theme.radiusSm
            color: theme.surfaceElevated
            clip: true

            KioskArtwork {
                id: art
                anchors.fill: parent
                source: root._artwork
                radius: artTile.radius
                fit: "crop"
            }

            // Quality badge — bottom-left, 6px inset on both axes, only for
            // hi-res / lossless.
            Rectangle {
                id: badge
                visible: root._tier !== ""
                x: 6
                y: artTile.height - badge.height - 6
                height: 19
                width: badgeLabel.implicitWidth + 12
                color: "#04070cd9"
                radius: 4

                Text {
                    id: badgeLabel
                    anchors.left: badge.left
                    anchors.leftMargin: 6
                    anchors.verticalCenter: badge.verticalCenter
                    text: root._tier === "hires" ? "Hi-Res" : "CD"
                    color: root._tier === "hires" ? "#e6b24d" : theme.textSecondary
                    font.pixelSize: 10
                    font.weight: theme.weightBold
                }
            }
        }

        Text {
            width: root.artSize
            text: root._title
            color: theme.textPrimary
            font.pixelSize: 14
            font.weight: theme.weightSemibold
            elide: Text.ElideRight
            wrapMode: Text.NoWrap
            maximumLineCount: 1
            horizontalAlignment: root.round ? Text.AlignHCenter : Text.AlignLeft
        }

        Text {
            width: root.artSize
            text: root._artist
            color: theme.textMuted
            font.pixelSize: 13
            elide: Text.ElideRight
            wrapMode: Text.NoWrap
            maximumLineCount: 1
            horizontalAlignment: root.round ? Text.AlignHCenter : Text.AlignLeft
        }
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: false
        onClicked: root.clicked(root.album && root.album.id ? root.album.id : "")
    }
}
