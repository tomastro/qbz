// Mini-rail folder flyout — the port of shell/SidebarFolderPopup.slint.
//
// In the 64px rail a folder's nested rows collapse to height 0, so expanding
// one in place would be a control that changes nothing on screen. Clicking a
// folder opens THIS panel to its right instead, listing that folder's
// playlists (four visible, the rest scroll).
//
// Extracted out of Sidebar.qml (block 6 of the playlist-manager contract). Two
// things changed in the move, both fixes rather than transcription:
//
//  1. The rows come from a DEDICATED document, QbzShell.sidebarFolderPopupJson
//     (contract §4.7), built in Rust from the sidebar CACHE. The old inline
//     version filtered the flattened `entries`, which only carry a folder's
//     children while that folder is EXPANDED — so it had to force-expand the
//     folder on the way in, and the folder then stayed open once the sidebar
//     was re-opened. The reference has no such side effect.
//  2. The name and the count still come SYNCHRONOUSLY from the clicked entry.
//     The document arrives a LATER event-loop turn (the cxx-qt UI queue), so
//     binding the header to it would show folder A's name while B is opening;
//     `rows` is therefore ignored until the document's own folderId matches
//     the one this panel was opened for.
//
// Geometry is 1:1 with SidebarFolderPopup.slint: 230px wide, 34 + 1 + rows*32
// + 8 tall (the 34 is deliberately 4px more than the 30px header it holds —
// kept, not tightened), surface-main / Radius.sm / 1px border-muted, 4px inner
// padding, 30px header with a 14px accent folder-open glyph, a 1px hairline,
// then 32px rows with a 14px muted list-music glyph.
//
// The list-music glyph is drawn for a LOCAL playlist too: SidebarFolderPopup
// .slint has no local-kind branch (unlike SidebarRow), so the hard-drive mark
// is lost inside the flyout. That is the reference — recorded, not an omission.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../theme"

Popup {
    id: root

    // Set SYNCHRONOUSLY by openFor() from the clicked entry — never from the
    // document (see the header note).
    property string folderId: ""
    property string folderName: ""
    property int folderCount: 0

    /// The sidebar owns `activePlaylistId` (the row highlight); a component
    /// cannot reach another file's ids, so it is told rather than assumed.
    signal playlistActivated(string id)

    width: 230
    padding: 0
    topPadding: 4
    bottomPadding: 4
    // Slint's literal panel height (34 + 1 + list + 8) — 4px taller than the
    // content, kept 1:1 rather than tightened.
    height: 34 + 1 + root.visibleRows * 32 + 8
    // Replaces Slint's full-window scrim TouchArea (SidebarFolderPopup.slint:
    // 56-60); the Popup's own background swallows clicks inside, so no
    // hand-built scrim and no click-through.
    closePolicy: Popup.CloseOnPressOutside | Popup.CloseOnEscape

    // Guarded parse — a raw JSON.parse in a binding throws on the pre-publish
    // frame. The fallback carries the FULL shape, so `.rows.length` is safe.
    readonly property var doc: {
        try {
            return JSON.parse(QbzShell.sidebarFolderPopupJson)
        } catch (e) {
            return ({ "folderId": "", "folderName": "", "count": 0, "rows": [] })
        }
    }
    // Only this folder's document counts. Without the guard, opening folder B
    // while A's document is still the published one lists A's playlists.
    readonly property var rows:
        (root.doc.folderId === root.folderId && root.doc.rows) ? root.doc.rows : []
    // Height off the folder's own count (known at click time) so the panel
    // does not resize when the rows land a tick later; `max` because the
    // entry's count is computed post-search and the document's is not.
    readonly property int visibleRows:
        Math.min(Math.max(root.folderCount, root.rows.length), 4)

    /// Open beside `anchorItem` (the rail row), +6px to its right and aligned
    /// to its top — SidebarFolderPopup's anchor-x / anchor-y.
    ///
    /// `entry` is the SidebarEntry the row carries: {id, name, count, …}.
    function openFor(entry, anchorItem) {
        root.folderId = entry.id
        root.folderName = entry.name
        root.folderCount = entry.count
        // Fill the rows from the cache. No expand side effect (header note 1).
        QbzShell.sidebarOpenFolderPopup(entry.id)
        var p = anchorItem.mapToItem(root.parent, anchorItem.width + 6, 0)
        root.x = p.x
        root.y = p.y
        root.open()
    }

    background: Rectangle {
        color: theme.surfaceMain
        radius: theme.radiusSm
        border.width: 1
        border.color: theme.borderMuted
    }

    contentItem: Column {
        spacing: 0

        // The theme tokens. It lives HERE, inside the content tree, because a
        // QML file has no lexical scope across files and this Popup's own
        // default property is `contentData` — a positioner ignores a plain
        // QtObject in its data, so nothing is laid out for it.
        QbzTheme { id: theme }

        // Header — folder name (30px, 12px side padding, 14px accent
        // folder-open glyph, 13px/600 text-primary).
        Item {
            width: parent.width
            height: 30
            Row {
                anchors.fill: parent
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                spacing: 8
                QbzIcon {
                    name: "folder-open"
                    width: 14
                    height: 14
                    anchors.verticalCenter: parent.verticalCenter
                    tintName: "accent"
                }
                Text {
                    width: parent.width - 22
                    height: parent.height
                    text: root.folderName
                    color: theme.textPrimary
                    font.pixelSize: 13
                    font.weight: theme.weightSemibold
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                }
            }
        }
        Rectangle {
            width: parent.width
            height: 1
            color: theme.borderSubtle
        }

        // Playlist list — four rows visible, the rest scroll (the shared
        // QbzScrollBar mounts past four, like Slint's ListScrollbar).
        //
        // Deliberately NOT a ListView: the panel shows at most four rows of a
        // single folder, the reference is a Flickable over a VerticalLayout,
        // and the recycling a ListView buys is worth nothing at that size.
        Item {
            width: parent.width
            height: root.visibleRows * 32

            Flickable {
                id: fpFlick
                anchors.fill: parent
                clip: true
                contentWidth: width
                contentHeight: fpColumn.implicitHeight
                boundsBehavior: Flickable.StopAtBounds

                Column {
                    id: fpColumn
                    width: parent.width
                    Repeater {
                        model: root.rows
                        delegate: Rectangle {
                            id: fpRow
                            required property var modelData
                            width: fpColumn.width
                            height: 32
                            radius: 5
                            color: fpArea.containsMouse ? theme.surfaceHover : "transparent"
                            Row {
                                anchors.fill: parent
                                anchors.leftMargin: 12
                                anchors.rightMargin: 12
                                spacing: 8
                                QbzIcon {
                                    name: "list-music"
                                    width: 14
                                    height: 14
                                    anchors.verticalCenter: parent.verticalCenter
                                    tintName: "muted"
                                }
                                Text {
                                    width: parent.width - 22
                                    height: parent.height
                                    text: fpRow.modelData.name
                                    color: theme.textSecondary
                                    font.pixelSize: 13
                                    verticalAlignment: Text.AlignVCenter
                                    elide: Text.ElideRight
                                }
                            }
                            MouseArea {
                                id: fpArea
                                anchors.fill: parent
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    // `local:` ids survive untouched —
                                    // openPlaylist routes them to the local
                                    // loader and works offline.
                                    root.playlistActivated(fpRow.modelData.id)
                                    QbzBridge.openPlaylist(fpRow.modelData.id)
                                    root.close()
                                }
                            }
                        }
                    }
                }
            }
            QbzScrollBar {
                visible: Math.max(root.folderCount, root.rows.length) > 4
                    && fpFlick.contentHeight > fpFlick.height
                target: fpFlick
                width: 10
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.bottom: parent.bottom
            }
        }
    }
}
