// PmTreePlaylistRow — a playlist row of the TREE mode
// (PlaylistManagerView.slint:947-1013). 40 px, r6, transparent →
// `surface-hover` on body hover, 0.6 opacity while hidden. A row inside a
// folder indents to 36 px, a root row starts at 10 (`:957`).
//
// Affordances (contract D27): open (body), favourite and edit-playlist ONLY —
// no visibility toggle, no mixtape, no move-to-folder, no reorder chevrons. The
// two buttons are HOVER-REVEALED, which the grid card's and the list row's are
// not.
//
// TWO deliberate deviations, both recorded in the contract:
//
//   * D26 — the reference tree carries no `hard-drive` marker for a local
//     playlist although the list row does. Added, with the list row's tint rule
//     (`accent` when offline-only, else `text-muted`).
//   * D27 — the reference binds the buttons' opacity to the ROW TouchArea,
//     which is `parent.width - 90px` and therefore EXCLUDES the buttons: moving
//     the pointer onto a button drops it to opacity 0 while it still accepts
//     the click. Here a `HoverHandler` on the row keeps them lit under the
//     pointer. The row FILL still follows the body area, exactly like the
//     reference.
//
// INTERFACE NOTE (contract §2.5): the contract declares `item` only. `indent`
// is ADDED because the indentation is a property of the TREE ROW
// (`tree[i].indent`, §4.2), not of the playlist — without it every nested row
// would sit at the root gutter and the tree would read flat.

import QtQuick
import "../../kiosk"
import com.blitzfc.qbz
import "../../cards"
import "../../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    /// `managerJson.tree[i].playlist` — one `PlaylistRow` (contract §4.3).
    property var item: ({})
    /// `tree[i].indent` — true for a folder member.
    property bool indent: false

    QbzTheme { id: theme }

    readonly property string itemId: String(root.item.id || "")
    readonly property bool isLocal: root.item.isLocal === true

    height: root.kioskHost ? 64 : 40
    radius: 6
    color: bodyArea.containsMouse ? theme.surfaceHover : "transparent"
    opacity: (root.item.isHidden === true) ? 0.6 : 1.0

    HoverHandler { id: rowHover }

    Loader {
        id: collage
        x: root.indent ? 36 : 10
        anchors.verticalCenter: parent.verticalCenter
        width: 28; height: 28
        sourceComponent: root.kioskHost ? kioskCover : desktopCover
    }
    Component { id: kioskCover; KioskArtwork { source: (root.item.covers || [])[0] || ""; radius: 5 } }
    Component { id: desktopCover;
    PlaylistCollage {
        id: desktopCollage
        x: 0
        anchors.verticalCenter: parent.verticalCenter
        width: 28
        height: 28
        layout: "pm"
        urls: root.item.covers || []
        radius: 5
    }
    }

    Row {
        id: nameRow
        x: collage.x + 28 + 10
        anchors.verticalCenter: parent.verticalCenter
        width: Math.max(0, tracksText.x - 10 - x)
        spacing: 6

        QbzIcon {
            visible: root.isLocal
            anchors.verticalCenter: parent.verticalCenter
            name: "hard-drive"
            width: 13
            height: 13
            tintName: root.item.offlineOnly === true ? "accent" : "muted"
        }
        Text {
            anchors.verticalCenter: parent.verticalCenter
            width: Math.max(0, parent.width - (root.isLocal ? 19 : 0))
            text: root.item.name || ""
            color: theme.textPrimary
            font.pixelSize: theme.fontBody
            elide: Text.ElideRight
        }
    }

    Text {
        id: tracksText
        x: actions.x - 10 - width
        anchors.verticalCenter: parent.verticalCenter
        text: root.item.tracksLine || ""
        color: theme.textMuted
        font.pixelSize: theme.fontLegal
    }

    Row {
        id: actions
        x: root.width - 12 - width
        anchors.verticalCenter: parent.verticalCenter
        spacing: 0
        opacity: root.kioskHost || rowHover.hovered ? 1.0 : 0.0

        PmActionButton {
            width: root.kioskHost ? 44 : 26
            height: root.kioskHost ? 44 : 26
            name: root.item.isFavorite === true ? "heart-filled" : "heart"
            filled: root.item.isFavorite === true
            onClicked: QbzPlaylistManager.toggleFavorite(root.itemId)
        }
        PmActionButton {
            width: root.kioskHost ? 44 : 26
            height: root.kioskHost ? 44 : 26
            name: "pen-line"
            onClicked: QbzPlaylistEdit.open(root.itemId)
        }
    }

    // Body: everything left of the two buttons' 90 px reserve (`:1006`).
    MouseArea {
        id: bodyArea
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: Math.max(0, parent.width - 90)
        z: -1
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: QbzBridge.openPlaylist(root.itemId)
    }
}
