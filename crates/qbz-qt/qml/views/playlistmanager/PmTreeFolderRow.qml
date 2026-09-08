// PmTreeFolderRow — a folder row of the TREE mode
// (PlaylistManagerView.slint:879-945). 44 px, r8, transparent →
// `surface-hover` on body hover.
//
// The two click targets are DIFFERENT here, unlike the card and the chip
// (contract D7): the body toggles the folder open/closed, the pencil opens the
// folder editor.
//
// ONE fixed asymmetry (contract D26): the reference gives a hidden folder no
// `opacity: 0.6` in the tree although the card and the chip both do. Applied
// here — the same folder must not read as hidden in one view mode and normal in
// another.
//
// The count on this row is the POST-FILTER visible member count, while the
// card's and the chip's is TOTAL membership (§5.7). Two different questions,
// deliberately; do not unify them.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    /// `managerJson.tree[i].folder` — a folder object whose `count` is the
    /// post-filter member count (contract §4.2).
    property var folder: ({})
    property bool expanded: false

    QbzTheme { id: theme }

    readonly property string folderId: String(root.folder.id || "")

    height: root.kioskHost ? 64 : 44
    radius: 8
    color: bodyArea.containsMouse ? theme.surfaceHover : "transparent"
    opacity: (root.folder.isHidden === true) ? 0.6 : 1.0

    QbzIcon {
        id: chevron
        x: 8
        anchors.verticalCenter: parent.verticalCenter
        name: root.expanded ? "chevron-down" : "chevron-right"
        width: 15
        height: 15
        tintName: "muted"
    }

    PmFolderIcon {
        id: tile
        x: chevron.x + 15 + 10
        anchors.verticalCenter: parent.verticalCenter
        size: 28
        iconPreset: root.folder.iconPreset || "folder"
        iconColor: root.folder.iconColor || ""
        hasColor: root.folder.hasColor === true
        hasCustomImage: root.folder.hasCustomImage === true
        customImagePath: root.folder.customImagePath || ""
    }

    Text {
        x: tile.x + 28 + 10
        anchors.verticalCenter: parent.verticalCenter
        width: Math.max(0, countText.x - 10 - x)
        text: root.folder.name || ""
        color: theme.textPrimary
        font.pixelSize: theme.fontBody
        font.weight: theme.weightSemibold
        elide: Text.ElideRight
    }

    Text {
        id: countText
        x: pencil.x - 10 - width
        anchors.verticalCenter: parent.verticalCenter
        text: root.folder.count === undefined ? "0" : String(root.folder.count)
        color: theme.textMuted
        font.pixelSize: theme.fontLegal
    }

    PmActionButton {
        id: pencil
        btnSize: root.kioskHost ? 44 : 26
        x: root.width - 12 - width
        anchors.verticalCenter: parent.verticalCenter
        name: "pen-line"
        onClicked: QbzFolderEdit.openEditor(root.folderId)
    }

    // Body: everything left of the pencil's 40 px reserve (`:938`).
    MouseArea {
        id: bodyArea
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: Math.max(0, parent.width - (root.kioskHost ? 56 : 40))
        z: -1
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: QbzPlaylistManager.toggleTreeFolder(root.folderId)
    }
}
