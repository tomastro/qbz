// PmPageHead — everything above the playlist body, mounted as the ONE
// `ListView.header` (contract D1): the counter/hint row, the loading spinner,
// the whole FOLDERS section, the PLAYLISTS header and BOTH empty states.
//
// It rides in the header rather than in a second scroller because the page is a
// single scroll surface in the reference (one Flickable over the lot,
// `:1281`) — and a ListView header is instantiated once, which is also why the
// FOLDERS section is deliberately NOT windowed (stated decision: the reference
// does not window it either and this file's own "many folders" threshold is 8).
//
// It INSETS ITSELF: the reference page is
// `VerticalLayout { padding-left: 32; padding-right: 32; padding-top: 6;
// padding-bottom: 100 }` (`:1300-1303`), so the content sits at x 32 with a
// 6 px lead — without that the counter row is flush against the toolbar. The
// bottom 100 lives in the ListView's footer, minus the last delegate's own
// trailing gap.
//
// `implicitHeight` is EXPLICIT and includes the 6 px: a zero-height
// `ListView.header` is silent, and this one's height is content-derived.
//
// Empty state: a plain left-aligned 15 px muted `Text`, NOT
// `controls/QbzEmptyState.qml` — that control is a centred Column with an 18 px
// `fontSection` title, so reusing it would silently change the size, the weight
// and the alignment. One of the few places where reuse is the parity break.
//
// There is no error state anywhere: the loader swallows every failure, so a
// network outage renders as "No playlists found." That is the reference
// (contract §5.13 / §10 Q4).

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root
    property bool kioskHost: false
    property Flickable viewport: null

    property bool loading: false
    /// `QbzPlaylistManager.foldersJson`, parsed (contract §4.1).
    property var folders: []
    property string viewMode: "grid"
    property bool folderMode: true
    property bool foldersCollapsed: false
    property string currentFolderId: ""
    property string currentFolderName: ""
    property bool canReorder: false
    /// `managerJson.folderCount` — equal to `folders.length` by construction,
    /// and what the COUNTER row reads (the section header reads the array).
    property int folderCount: 0
    property int playlistCount: 0
    /// `playlists.length`, or `tree.length` in tree mode — what decides the
    /// empty state.
    property int rowsInBody: 0

    QbzTheme { id: theme }

    readonly property bool foldersVisible: !root.loading && root.folderMode
        && root.currentFolderId === ""
        && root.viewMode !== "tree" && (root.folders || []).length > 0

    implicitHeight: 6 + col.height
    height: implicitHeight

    Column {
        id: col
        x: 32
        y: 6
        width: Math.max(0, root.width - 64)
        spacing: 0

        // ---- counter + reorder hint (h 24) --------------------------------
        Item {
            width: parent.width
            height: 24

            Text {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                // ONE string with TWO placeholders (`:1314`), not two strings.
                // `String.replace` with a string pattern replaces only the
                // FIRST occurrence, so the two calls chain.
                text: root.currentFolderId !== ""
                    ? (root.playlistCount === 1
                        ? QbzSession.tr("1 playlist", QbzSession.trRev)
                        : QbzSession.tr("{} playlists", QbzSession.trRev)
                            .replace("{}", root.playlistCount))
                    : QbzSession.tr("{} folders, {} playlists", QbzSession.trRev)
                        .replace("{}", root.folderCount)
                        .replace("{}", root.playlistCount)
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
            }
            Text {
                visible: root.canReorder
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: QbzSession.tr("Use the arrows to reorder playlists",
                                    QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
            }
        }

        // Unconditional 14 px (`:1332`).
        Item { width: 1; height: 14 }

        // ---- loading spinner ----------------------------------------------
        Item {
            visible: root.loading
            width: parent.width
            height: visible ? 36 : 0

            QbzSpinner {
                anchors.centerIn: parent
                size: 36
            }
        }

        // Grid/list folder drill-down. The manager keeps the selected view
        // mode; this compact breadcrumb is the explicit route back to the
        // folders + root-playlists surface.
        Item {
            visible: !root.loading && root.currentFolderId !== ""
            width: parent.width
            height: visible ? 40 : 0

            QbzToolButton {
                id: folderBack
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                name: "chevron-left"
                label: QbzSession.tr("Back", QbzSession.trRev)
                large: true
                onClicked: QbzPlaylistManager.closeFolder()
            }
            Text {
                anchors.left: folderBack.right
                anchors.leftMargin: 12
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: root.currentFolderName
                color: theme.textPrimary
                font.pixelSize: theme.fontBody
                font.weight: theme.weightSemibold
                elide: Text.ElideRight
            }
        }
        Item {
            visible: !root.loading && root.currentFolderId !== ""
            width: 1
            height: visible ? 14 : 0
        }

        // ---- FOLDERS section ----------------------------------------------
        // The WHOLE section disappears with zero folders — no header, no
        // placeholder (`:1343`). It is also the only collapsible section, and
        // collapsing does not rebuild anything.
        Column {
            visible: root.foldersVisible
            width: parent.width
            spacing: 12

            QbzSectionHeader {
                title: QbzSession.tr("Folders ({})", QbzSession.trRev)
                    .replace("{}", (root.folders || []).length)
                // The section header's count is the ARRAY length (`:1346`),
                // while the counter row above reads `folderCount`. Same number,
                // two bindings, exactly as upstream.
                titleSecondary: true
                collapsible: true
                collapsed: root.foldersCollapsed
                // DEFAULTS TO TRUE — without this the carousel page chevrons
                // would render on a page that has no carousel.
                showChevrons: false
                onToggled: QbzPlaylistManager.toggleFoldersCollapsed()
            }

            Item {
                id: kioskFolders
                visible: root.kioskHost && !root.foldersCollapsed
                width: parent.width
                height: visible ? root.folders.length * 64 : 0
                readonly property real viewTop: root.viewport ? root.viewport.contentY - mapToItem(root.viewport.contentItem,0,0).y : 0
                readonly property int first: Math.max(0, Math.floor(viewTop / 64) - 1)
                readonly property int last: Math.min(root.folders.length, Math.ceil((viewTop + (root.viewport ? root.viewport.height : 0)) / 64) + 1)
                Repeater {
                    model: kioskFolders.visible ? root.folders.slice(kioskFolders.first, Math.max(kioskFolders.first,kioskFolders.last)) : []
                    delegate: Rectangle {
                        required property var modelData
                        required property int index
                        width: kioskFolders.width
                        height: 64
                        y: (kioskFolders.first + index) * 64
                        color: theme.surfaceElevated
                        Text { x: 12; width: parent.width - 100; anchors.verticalCenter: parent.verticalCenter; text: parent.modelData.name || ""; color: theme.textPrimary; font.pixelSize: 18; elide: Text.ElideRight }
                        MouseArea { anchors.fill: parent; anchors.rightMargin: 80; onClicked: QbzPlaylistManager.openFolder(String(parent.modelData.id || "")) }
                        SettingsButton { anchors.right: parent.right; kioskHost: true; minWidth: 80; btnHeight: 64; text: QbzSession.tr("Edit",QbzSession.trRev); onClicked: QbzFolderEdit.openEditor(String(parent.modelData.id || "")) }
                    }
                }
            }

            // GRID mode — the big folder cards, hand-wrapped (Slint has no
            // flex-wrap and neither does a plain QML positioner at fixed pitch).
            Item {
                id: folderGrid
                visible: !root.kioskHost && !root.foldersCollapsed && root.viewMode === "grid"
                width: parent.width
                readonly property int cols: Math.max(1, Math.floor((width + 16) / 176))
                readonly property int rows:
                    Math.ceil((root.folders || []).length / Math.max(1, cols))
                height: visible && rows > 0
                    ? rows * 150 + Math.max(0, rows - 1) * 16 : 0

                Repeater {
                    model: root.kioskHost ? [] : (root.folders || [])
                    delegate: PmFolderCard {
                        required property var modelData
                        required property int index
                        x: (index % folderGrid.cols) * 176
                        y: Math.floor(index / folderGrid.cols) * 166
                        folder: modelData
                        onOpenRequested: QbzPlaylistManager.openFolder(String(modelData.id || ""))
                        onEditRequested: QbzFolderEdit.openEditor(String(modelData.id || ""))
                    }
                }
            }

            // LIST mode — the compact chips, uniform 184 px cells.
            Item {
                id: folderChips
                visible: !root.kioskHost && !root.foldersCollapsed && root.viewMode === "list"
                width: parent.width
                readonly property int cols: Math.max(1, Math.floor((width + 8) / 192))
                readonly property int rows:
                    Math.ceil((root.folders || []).length / Math.max(1, cols))
                height: visible && rows > 0
                    ? rows * 52 + Math.max(0, rows - 1) * 8 : 0

                Repeater {
                    model: root.kioskHost ? [] : (root.folders || [])
                    delegate: PmFolderChip {
                        required property var modelData
                        required property int index
                        x: (index % folderChips.cols) * 192
                        y: Math.floor(index / folderChips.cols) * 60
                        width: 184
                        folder: modelData
                        onOpenRequested: QbzPlaylistManager.openFolder(String(modelData.id || ""))
                        onEditRequested: QbzFolderEdit.openEditor(String(modelData.id || ""))
                    }
                }
            }

            // The trailing spacer is a CHILD of a spacing-12 column, so the real
            // gap before the PLAYLISTS header is 36, not 24 (`:1388`).
            Item { width: 1; height: 24 }
        }

        // ---- PLAYLISTS header (never in tree mode) -------------------------
        QbzSectionHeader {
            visible: !root.loading && root.viewMode !== "tree"
            title: QbzSession.tr("Playlists ({})", QbzSession.trRev)
                .replace("{}", root.playlistCount)
            titleSecondary: true
            showChevrons: false
        }
        Item {
            visible: !root.loading && root.viewMode !== "tree"
            width: 1
            height: visible ? 14 : 0
        }

        // ---- the two empty states ------------------------------------------
        Text {
            visible: !root.loading && root.viewMode !== "tree" && root.rowsInBody === 0
            text: QbzSession.tr("No playlists found.", QbzSession.trRev)
            color: theme.textMuted
            font.pixelSize: theme.fontBody
        }
        Text {
            visible: !root.loading && root.viewMode === "tree" && root.rowsInBody === 0
            text: QbzSession.tr("No playlists found.", QbzSession.trRev)
            color: theme.textMuted
            font.pixelSize: theme.fontBody
        }
    }
}
