// PmListRow — the LIST-mode playlist row (PlaylistManagerView.slint:622-875).
// 64 px, r6, `surface-card` → `surface-hover` on BODY hover, 0.6 opacity while
// hidden. No zebra striping and no separators.
//
// Affordances (contract D27): the grid card's seven PLUS the `⋯`
// move-to-folder menu, whose button renders only when at least one folder
// exists (`:791`). All of them are always visible — no hover reveal.
//
// Local-row suppressions: no cassette (D28) and no reorder chevrons (D29), with
// the 20 px reorder column still RESERVED when `canReorder` so the row's
// columns do not shift between a Qobuz and a local row.
//
// THE MENU IS NOT OURS TO OPEN (contract D17). One `PmFolderMenu` is mounted at
// the VIEW level — with `reuseItems: true` a per-row menu would give every
// recycled delegate its own Popup — and a delegate cannot see the view's ids,
// so the row ASKS: `folderMenuRequested(anchorItem)`.
//
// Right-press parity (D18): a right press anywhere on the row body opens the
// SAME menu the `⋯` opens, and only when folders exist. That is the port-wide
// invariant QbzContextMenu.qml:20-22 states; the reference has no right-click
// here, so this is the port's convention winning, recorded.
//
// The row body's hit area is ANCHORED to the left of the action strip rather
// than copying the reference's hardcoded `parent.width - 184px` (`:868`), which
// is off by a few pixels whenever the `⋯` is absent or the play-count badge is
// wide. Same intent, correct arithmetic.

import QtQuick
import "../../kiosk"
import com.blitzfc.qbz
import "../../cards"
import "../../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    /// One `PlaylistRow` (contract §4.3).
    property var item: ({})
    /// Absolute index in the visible order.
    property int itemIndex: 0
    property bool canReorder: false
    /// `foldersJson.length` — the `⋯` renders only when > 0 (§5.16).
    property int foldersCount: 0
    /// Asks the VIEW to open its single PmFolderMenu against `anchorItem`.
    signal folderMenuRequested(var anchorItem)

    QbzTheme { id: theme }

    readonly property string itemId: String(root.item.id || "")
    readonly property bool isLocal: root.item.isLocal === true
    /// Reorder column + its trailing 12 px gap, or nothing.
    readonly property int reorderSlot: root.canReorder ? (root.kioskHost ? 56 : 32) : 0

    height: root.kioskHost && root.canReorder ? 88 : 64
    radius: 6
    color: bodyArea.containsMouse ? theme.surfaceHover : theme.surfaceCard
    opacity: (root.item.isHidden === true) ? 0.6 : 1.0

    // ---- reorder column (20 × 40, chevrons stacked, spacing 0) -------------
    Column {
        visible: root.canReorder
        x: 10
        anchors.verticalCenter: parent.verticalCenter
        spacing: 0

        PmActionButton {
            visible: !root.isLocal
            name: "chevron-up"
            btnSize: root.kioskHost ? 44 : 20
            glyph: 13
            onClicked: QbzPlaylistManager.moveUp(root.itemId)
        }
        PmActionButton {
            visible: !root.isLocal
            name: "chevron-down"
            btnSize: root.kioskHost ? 44 : 20
            glyph: 13
            onClicked: QbzPlaylistManager.moveDown(root.itemId)
        }
    }

    // ---- collage ----------------------------------------------------------
    Loader {
        id: collage
        x: 10 + root.reorderSlot
        anchors.verticalCenter: parent.verticalCenter
        width: 48
        height: 48
        sourceComponent: root.kioskHost ? kioskCover : desktopCover
    }
    Component {
        id: kioskCover
        KioskArtwork { source: (root.item.covers || [])[0] || ""; radius: 6 }
    }
    Component {
        id: desktopCover
        PlaylistCollage { anchors.fill: parent; layout: "pm"; urls: root.item.covers || []; radius: 6 }
    }

    // ---- name + meta ------------------------------------------------------
    Column {
        id: textCol
        x: collage.x + 48 + 12
        anchors.verticalCenter: parent.verticalCenter
        width: Math.max(0, (badge.visible ? badge.x : actions.x) - 12 - x)
        spacing: 2

        Row {
            width: parent.width
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
                font.pixelSize: 14
                font.weight: theme.weightMedium
                elide: Text.ElideRight
            }
        }
        Row {
            width: parent.width
            spacing: 6

            Text {
                text: root.item.tracksLine || ""
                color: theme.textMuted
                font.pixelSize: 12
            }
            Text {
                visible: (root.item.durationLine || "") !== ""
                text: "·"
                color: theme.textMuted
                font.pixelSize: 12
            }
            Text {
                visible: (root.item.durationLine || "") !== ""
                text: root.item.durationLine || ""
                color: theme.textMuted
                font.pixelSize: 12
            }
            Text {
                visible: (root.item.localLine || "") !== ""
                text: root.item.localLine || ""
                // A LITERAL, not `theme.success` (#3fae6a) — a different green,
                // and the reference pins this one (`:726`).
                color: "#22c55e"
                font.pixelSize: 12
            }
        }
    }

    // ---- play-count badge -------------------------------------------------
    Rectangle {
        id: badge
        visible: (root.item.playCount || 0) > 0
        x: actions.x - 12 - width
        anchors.verticalCenter: parent.verticalCenter
        width: pcRow.width + 16
        height: 22
        radius: 6
        color: theme.surfaceElevated

        Row {
            id: pcRow
            x: 8
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4

            QbzIcon {
                anchors.verticalCenter: parent.verticalCenter
                name: "chart-no-axes-column"
                width: 12
                height: 12
                tintName: "muted"
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: String(root.item.playCount || 0)
                color: theme.textMuted
                font.pixelSize: 11
            }
        }
    }

    // ---- actions ----------------------------------------------------------
    Row {
        id: actions
        x: root.width - 12 - width
        anchors.verticalCenter: parent.verticalCenter
        spacing: 0

        PmActionButton {
            btnSize: root.kioskHost ? 44 : 26
            name: root.item.isFavorite === true ? "heart-filled" : "heart"
            filled: root.item.isFavorite === true
            onClicked: QbzPlaylistManager.toggleFavorite(root.itemId)
        }
        PmActionButton {
            btnSize: root.kioskHost ? 44 : 26
            name: root.item.isHidden === true ? "eye-off" : "eye"
            onClicked: QbzPlaylistManager.toggleHidden(root.itemId)
        }
        PmActionButton {
            btnSize: root.kioskHost ? 44 : 26
            name: "pen-line"
            onClicked: QbzPlaylistEdit.open(root.itemId)
        }
        PmActionButton {
            btnSize: root.kioskHost ? 44 : 26
            visible: !root.isLocal
            name: "cassette-tape"
            onClicked: QbzMyQbzAdd.open(JSON.stringify([{
                "itemType": "playlist", "source": "qobuz",
                "sourceItemId": root.itemId,
                "title": root.item.name || "",
                "subtitle": "", "artworkUrl": (root.item.covers || [])[0] || "",
                "year": null, "trackCount": root.item.totalCount || 0
            }]))
        }
        PmActionButton {
            id: moreBtn
            btnSize: root.kioskHost ? 44 : 26
            visible: root.foldersCount > 0
            name: "ellipsis"
            onClicked: root.folderMenuRequested(moreBtn)
        }
    }

    // ---- body hit area ----------------------------------------------------
    // Stops short of the badge / action strip on the right, and carries `z: -1`
    // for the LEFT end: the reorder chevrons sit INSIDE this width, and
    // declaration order is z-order in QML, so a last-declared area at the
    // default z would swallow their presses (`propagateComposedEvents` does not
    // rescue `pressed`).
    MouseArea {
        id: bodyArea
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: Math.max(0, (badge.visible ? badge.x : actions.x) - 4)
        z: -1
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onClicked: function (mouse) {
            if (mouse.button === Qt.RightButton) {
                if (root.foldersCount > 0)
                    root.folderMenuRequested(root)
                return
            }
            QbzBridge.openPlaylist(root.itemId)
        }
    }
}
