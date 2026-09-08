// Folders tree-mode LEFT RAIL (LocalLibraryView.slint:1641).
//
// Header toolbar: select-mode, collapse-all, open-ephemeral-folder, then the
// search box (stretches). Below it the compact bulk bar (only in select mode
// with a selection — icon buttons, because the rail is narrow), then the
// tree itself.
//
// The tree is a FLAT array windowed by a ListView — the recursive component
// is deliberately not reproduced, and levels are fetched lazily on expand.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root

    property var view: null

    QbzTheme { id: theme }

    Column {
        anchors.fill: parent
        anchors.leftMargin: 12
        anchors.rightMargin: 8
        anchors.topMargin: 12
        anchors.bottomMargin: 8
        spacing: 8

        // ---- Header toolbar ----
        Row {
            width: parent.width
            height: 30
            spacing: 4
            QbzIconButton {
                name: "square-check-big"
                btnSize: 30
                iconSize: 15
                active: root.view.treeSelectMode
                activeBackground: true
                onClicked: root.view.toggleTreeSelectMode()
            }
            QbzIconButton {
                name: "chevron-up"
                btnSize: 30
                iconSize: 15
                onClicked: QbzLocal.treeCollapseAll()
            }
            // Filler: the search collapses to a 30px magnifier, so the slot
            // stays right-aligned and the field opens LEFT over this gap.
            Item {
                // TWO buttons now, not three: "open folder" moved to the
                // header's `Open` menu, which serves every medium.
                width: Math.max(0, parent.width - 2 * 30 - 30 - 3 * 4)
                height: 1
            }
            LocalSearchBox {
                boxWidth: parent.width - 2 * 34
                placeholder: QbzSession.tr("Search folders", QbzSession.trRev)
                onEdited: function (v) {
                    root.view.treeSearch = v
                    treeSearchDebounce.restart()
                }
                Timer {
                    id: treeSearchDebounce
                    interval: 180
                    onTriggered: QbzLocal.treeSearch(root.view.treeSearch)
                }
            }
        }

        // ---- Compact bulk bar (select mode + a selection) ----
        Rectangle {
            visible: root.view.treeSelectMode && QbzLocal.localTreeSelectedCount > 0
            width: parent.width
            height: visible ? 36 : 0
            radius: 8
            color: theme.surfaceElevated
            Row {
                anchors.fill: parent
                anchors.leftMargin: 10
                anchors.rightMargin: 6
                spacing: 2
                Text {
                    width: parent.width - 7 * 30
                    height: parent.height
                    text: QbzLocal.localTreeSelectedCount + " "
                        + QbzSession.tr("sel", QbzSession.trRev)
                    color: theme.textSecondary
                    font.pixelSize: theme.fontLegal
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                }
                Repeater {
                    // ASSET GAP: Slint's "Check all" uses folder-check, which
                    // is not baked in the Qt icon set (see GLUE).
                    model: [
                        { "icon": "square-check-big", "action": "select-all", "tip": QbzSession.tr("Check all", QbzSession.trRev) },
                        { "icon": "list-start", "action": "play-next", "tip": QbzSession.tr("Play next", QbzSession.trRev) },
                        { "icon": "list-plus", "action": "play-later", "tip": QbzSession.tr("Play later", QbzSession.trRev) },
                        { "icon": "list-end", "action": "queue", "tip": QbzSession.tr("Add to queue", QbzSession.trRev) },
                        { "icon": "list-music", "action": "add-to-playlist", "tip": QbzSession.tr("Add to playlist", QbzSession.trRev) },
                        { "icon": "cassette-tape", "action": "add-to-mixtape", "tip": QbzSession.tr("Add to Mixtape/Collection", QbzSession.trRev) },
                        { "icon": "x", "action": "clear", "tip": QbzSession.tr("Clear", QbzSession.trRev) },
                    ]
                    delegate: QbzIconButton {
                        required property var modelData
                        anchors.verticalCenter: parent.verticalCenter
                        name: modelData.icon
                        btnSize: 28
                        iconSize: 15
                        onClicked: QbzLocal.foldersBulkAction(modelData.action)
                    }
                }
            }
        }

        // ---- The tree ----
        Item {
            width: parent.width
            height: parent.height - 38
                - (root.view.treeSelectMode && QbzLocal.localTreeSelectedCount > 0 ? 44 : 0)

            // Lazy level fetch: the rail shows the shape of the tree rows
            // (26px, small leading glyph) instead of a 28px spinner.
            QbzSkeleton {
                variant: "rowList"
                anchors.fill: parent
                visible: QbzLocal.localTreeLoading
                rowH: 26
                rowGap: 0
                rowArtSize: 14
                phase: root.view ? root.view.skelPhase : false
            }
            ListView {
                id: treeList
                anchors.fill: parent
                visible: !QbzLocal.localTreeLoading
                clip: true
                cacheBuffer: 26 * 20
                boundsBehavior: Flickable.StopAtBounds
                model: root.view.tree
                delegate: TreeRow {
                    required property var modelData
                    width: treeList.width
                    node: modelData
                    selected: modelData.path === root.view.selectedFolder
                    selectMode: root.view.treeSelectMode
                    onToggled: QbzLocal.treeToggle(modelData.path, !modelData.expanded)
                    onActivated: root.view.selectFolder(modelData.path)
                    // NO Shift-range here, deliberately: this rail's selection
                    // lives in RUST (QbzLocal.treeToggle*Select owns the set),
                    // not in a QML map, so controls/SelectionModel.qml has
                    // nothing to hold an anchor against. Giving the tree ranges
                    // means teaching the Rust side about an anchor, which is a
                    // change of its own and not part of this port.
                    onToggleSelect: {
                        if (modelData.isFolder) QbzLocal.treeToggleFolderSelect(modelData.path)
                        else QbzLocal.treeToggleTrackSelect(modelData.path)
                    }
                }
            }
            QbzScrollBar {
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                target: treeList
                visible: treeList.contentHeight > treeList.height
            }
        }
    }
}
