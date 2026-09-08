// KioskArtistCard — lightweight round artist card for the kiosk
// Search/Library/LocalLibrary artist tabs (shell/KioskArtistCard.slint).
// Round avatar + centred name, tap to open. No overlay row, no context menu,
// no follow button — the deliberate absences of the whole kiosk card family.
//
// The Slint takes primitive fields so ONE component serves both producer
// shapes; the Qt port takes the row object and reads both spellings for the
// same reason: search artists arrive as { id, title, artwork } (SlimItem) and
// favourite artists as { id, name, image } (FavoriteArtistItem). `artwork` /
// `image` carry the RESOLVED local avatar path the hosting view supplies, not
// the remote url.
//
// The CARD corner is 8px even though the avatar inside is a full circle.
//
// ARTWORK: KioskArtwork, for the reason its header gives — the avatar asks for
// the smallest provider bucket that covers its physical circle instead of
// decoding an original to find out how big it was.

import QtQuick
import "../theme"

Rectangle {
    id: root

    property var artist: ({})
    property real artSize: 128
    /// Keyboard/gamepad nav — same ring idiom as KioskCard.
    property bool navFocused: false
    signal clicked(string id)

    /// The avatar is DECODED and on screen — the KioskCard.artReady contract.
    readonly property bool artReady: art.ready

    QbzTheme { id: theme }

    readonly property string _id: root.artist && root.artist.id ? root.artist.id : ""
    readonly property string _name: root.artist
        ? (root.artist.name ? root.artist.name : (root.artist.title ? root.artist.title : ""))
        : ""
    readonly property string _avatar: root.artist
        ? (root.artist.artwork ? root.artist.artwork : (root.artist.image ? root.artist.image : ""))
        : ""

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
        spacing: 8

        Rectangle {
            id: avatarTile
            width: root.artSize
            height: root.artSize
            radius: root.artSize / 2
            color: theme.surfaceElevated
            clip: true

            KioskArtwork {
                id: art
                anchors.fill: parent
                source: root._avatar
                radius: avatarTile.radius
                fit: "crop"
            }
        }

        Text {
            width: root.artSize
            text: root._name
            color: theme.textPrimary
            font.pixelSize: 13
            font.weight: theme.weightSemibold
            horizontalAlignment: Text.AlignHCenter
            elide: Text.ElideRight
            wrapMode: Text.NoWrap
            maximumLineCount: 1
        }
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: false
        onClicked: root.clicked(root._id)
    }
}
