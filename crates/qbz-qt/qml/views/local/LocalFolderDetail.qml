// Folders tree-mode RIGHT PANE — the selected folder's detail
// (LocalLibraryView.slint:1918).
//
// Header: circular Play + folder name + recursive track count + the
// persistent subfolder search + the grid/list toggle. Then the subfolder
// section (cover cards on a 140px-minimum column grid, or compact 34px
// rows), then the folder's DIRECT tracks, then the "no tracks" note.
//
// Subfolder/track counts are per-folder and bounded, so this pane mounts its
// children like the Slint does (one Flickable page) — the windowing views
// are for the library-scale surfaces.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root

    property var view: null

    QbzTheme { id: theme }

    readonly property var detail: view ? view.folderDetail : null

    // The host reports this pane's covers as ONE window (the set is small and
    // fully mounted — no windowing here). That report fires when the DOCUMENT
    // changes, which is not enough on its own: switching Folders back to tree
    // mode, or returning to the tab, re-shows a pane whose document has not
    // changed since, and the covers had been evicted meanwhile. Re-report
    // whenever the pane comes back on screen, and let go when it leaves.
    Component.onCompleted: if (view && visible) view.reportFolderDetailWindow()
    Component.onDestruction: if (view) view.releaseWindow("folder-detail")
    onVisibleChanged: {
        if (!view) return
        if (visible) view.reportFolderDetailWindow()
        else view.releaseWindow("folder-detail")
    }
    Connections {
        target: root.view
        function onArtworkRefresh() {
            if (root.visible) root.view.reportFolderDetailWindow()
            else root.view.releaseWindow("folder-detail")
        }
    }

    LocalNote {
        visible: root.view.selectedFolder === ""
        text: QbzSession.tr("Select a folder to see its contents.", QbzSession.trRev)
    }
    // The folder detail is header + subfolder cards + track rows; the pane
    // fetch replaces a 28px spinner with that shape (a header band, then a
    // card grid). Two composites = two animators.
    Column {
        visible: QbzLocal.localDetailLoading
        anchors.fill: parent
        anchors.leftMargin: 28
        anchors.rightMargin: 28
        anchors.topMargin: 16
        spacing: 14
        QbzSkeleton {
            variant: "text"
            width: Math.min(parent.width, 420)
            lines: 2
            lineHeight: 20
            phase: root.view ? root.view.skelPhase : false
        }
        QbzSkeleton {
            variant: "cardGrid"
            width: parent.width
            height: parent.height - 70
            cellW: 152
            cellH: 194
            cardW: 140
            cardH: 182
            phase: root.view ? root.view.skelPhase : false
        }
    }

    Flickable {
        id: pane
        anchors.fill: parent
        visible: root.view.selectedFolder !== "" && !QbzLocal.localDetailLoading
            && root.detail !== null
        clip: true
        contentWidth: width
        contentHeight: page.height + 100
        boundsBehavior: Flickable.StopAtBounds

        Column {
            id: page
            x: 28
            y: 16
            width: parent.width - 56
            spacing: 14

            // ---- Header ----
            Row {
                width: parent.width
                spacing: 12
                QbzCircleAction {
                    name: "play-fill"
                    primary: true
                    anchors.verticalCenter: parent.verticalCenter
                    onClicked: QbzLocal.playFolder(root.view.selectedFolder)
                }
                Column {
                    // The search collapses to a 30px magnifier and opens LEFT
                    // over this column, so the name block owns that space.
                    width: parent.width - 40 - 30 - 60 - 3 * 12
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 2
                    Text {
                        width: parent.width
                        text: root.detail ? root.detail.name : ""
                        color: theme.textPrimary
                        font.pixelSize: theme.fontSection
                        font.weight: theme.weightSemibold
                        elide: Text.ElideRight
                    }
                    Text {
                        width: parent.width
                        text: (root.detail ? root.detail.trackCount : 0) + " "
                            + QbzSession.tr("tracks", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: theme.fontLegal
                        elide: Text.ElideRight
                    }
                }
                LocalSearchBox {
                    anchors.verticalCenter: parent.verticalCenter
                    boxWidth: 200
                    placeholder: QbzSession.tr("Search albums...", QbzSession.trRev)
                    onEdited: function (v) { root.view.folderDetailSearch = v }
                }
                QbzSegToggle {
                    anchors.verticalCenter: parent.verticalCenter
                    segments: [{ "id": "grid", "icon": "layout-grid" },
                               { "id": "list", "icon": "list" }]
                    mode: root.view.folderDetailView
                    onSetMode: function (v) { root.view.folderDetailView = v }
                }
            }

            // ---- Subfolders ----
            Column {
                width: parent.width
                spacing: 12
                visible: root.detail && root.detail.subfolders.length > 0
                Text {
                    text: QbzSession.tr("Folders", QbzSession.trRev)
                    color: theme.textSecondary
                    font.pixelSize: 12
                    font.weight: theme.weightSemibold
                    font.letterSpacing: 0.5
                }
                Text {
                    visible: root.view.detailSubfolders.length === 0
                    width: parent.width
                    topPadding: 8
                    text: QbzSession.tr("No folders match your search.", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: theme.fontBody
                    horizontalAlignment: Text.AlignHCenter
                }
                // Grid — the Slint's `subgrid`: 140px minimum column, 12px
                // gap, card height = column width + 42.
                Item {
                    id: subgrid
                    visible: root.view.folderDetailView === "grid"
                        && root.view.detailSubfolders.length > 0
                    readonly property int gap: 12
                    readonly property int minCw: 140
                    readonly property int cols: Math.max(1, Math.floor((width + gap) / (minCw + gap)))
                    readonly property real cw: (width - (cols - 1) * gap) / cols
                    readonly property real ch: cw + 42
                    readonly property int rws: Math.ceil(root.view.detailSubfolders.length / cols)
                    width: parent.width
                    height: visible && rws > 0 ? rws * ch + (rws - 1) * gap : 0
                    Repeater {
                        model: root.view.detailSubfolders
                        delegate: FolderSubcard {
                            required property var modelData
                            required property int index
                            x: (index % subgrid.cols) * (subgrid.cw + subgrid.gap)
                            y: Math.floor(index / subgrid.cols) * (subgrid.ch + subgrid.gap)
                            width: subgrid.cw
                            height: subgrid.ch
                            item: modelData
                            view: root.view
                            artSource: root.view.artMap[modelData.artKey] || ""
                            onOpened: root.view.selectFolder(modelData.path)
                        }
                    }
                }
                // List — compact 34px rows.
                Column {
                    visible: root.view.folderDetailView === "list"
                        && root.view.detailSubfolders.length > 0
                    width: parent.width
                    spacing: 2
                    Repeater {
                        model: root.view.detailSubfolders
                        delegate: Rectangle {
                            id: subRow
                            required property var modelData
                            width: parent.width
                            height: 34
                            radius: 6
                            color: subArea.containsMouse ? theme.surfaceHover : "transparent"
                            Row {
                                anchors.fill: parent
                                anchors.leftMargin: 8
                                anchors.rightMargin: 10
                                spacing: 8
                                QbzIcon {
                                    name: "folder"
                                    width: 14
                                    height: 14
                                    anchors.verticalCenter: parent.verticalCenter
                                    tintName: "muted"
                                }
                                Text {
                                    width: parent.width - 60
                                    height: parent.height
                                    text: subRow.modelData.name
                                    color: theme.textPrimary
                                    font.pixelSize: 13
                                    verticalAlignment: Text.AlignVCenter
                                    elide: Text.ElideRight
                                }
                                Text {
                                    height: parent.height
                                    text: subRow.modelData.trackCount
                                    color: theme.textMuted
                                    font.pixelSize: 11
                                    verticalAlignment: Text.AlignVCenter
                                }
                            }
                            MouseArea {
                                id: subArea
                                anchors.fill: parent
                                hoverEnabled: true
                                acceptedButtons: Qt.LeftButton | Qt.RightButton
                                cursorShape: Qt.PointingHandCursor
                                onClicked: function (mouse) {
                                    if (mouse.button === Qt.RightButton) {
                                        subMenu.openAtCursor(subArea, mouse.x, mouse.y)
                                        return
                                    }
                                    root.view.selectFolder(subRow.modelData.path)
                                }
                            }
                            // Same folder queue menu as the tree rows.
                            CardMenu {
                                id: subMenu
                                menuWidth: 200
                                entries: [
                                    { "label": QbzSession.tr("Play", QbzSession.trRev), "icon": "play-fill", "action": "play" },
                                    { "label": QbzSession.tr("Play next", QbzSession.trRev), "icon": "list-start", "action": "next" },
                                    { "label": QbzSession.tr("Play later", QbzSession.trRev), "icon": "list-plus", "action": "later" },
                                    { "label": QbzSession.tr("Add to queue", QbzSession.trRev), "icon": "list-end", "action": "queue" },
                                ]
                                onPicked: function (a) {
                                    if (a === "play") QbzLocal.playFolder(subRow.modelData.path)
                                    else QbzLocal.enqueue("folder", subRow.modelData.path, a)
                                }
                            }
                        }
                    }
                }
            }

            // ---- The folder's DIRECT tracks ----
            Column {
                width: parent.width
                spacing: 6
                visible: root.detail && root.detail.tracks.length > 0
                Text {
                    text: QbzSession.tr("Tracks", QbzSession.trRev)
                    color: theme.textSecondary
                    font.pixelSize: 12
                    font.weight: theme.weightSemibold
                    font.letterSpacing: 0.5
                }
                Repeater {
                    model: root.detail ? root.detail.tracks : []
                    delegate: LocalTrackRow {
                        required property var modelData
                        required property int index
                        width: page.width
                        view: root.view
                        item: modelData
                        number: modelData.number > 0 ? modelData.number : index + 1
                        showArtwork: root.view.trackArtwork
                        artSource: root.view.artMap[modelData.artKey] || ""
                        onPlayRequested: QbzLocal.playFolderTrack(
                            root.view.selectedFolder, modelData.id)
                        onEnqueueRequested: function (m) {
                            QbzLocal.enqueue("track", modelData.id, m)
                        }
                    }
                }
            }

            // Folder with neither tracks nor subfolders.
            Text {
                visible: root.detail && root.detail.tracks.length === 0
                    && root.detail.subfolders.length === 0
                width: parent.width
                topPadding: 24
                text: QbzSession.tr("This folder has no tracks.", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontBody
                horizontalAlignment: Text.AlignHCenter
            }
        }
    }
    QbzScrollBar {
        anchors.right: parent.right
        anchors.rightMargin: 4
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        target: pane
        visible: pane.visible && pane.contentHeight > pane.height
    }
}
