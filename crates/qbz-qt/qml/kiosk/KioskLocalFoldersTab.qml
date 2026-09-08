// Local Library > Folders, kiosk body.
//
// The FLAT folder representation (`localFoldersJson`), rendered through the
// same bounded album grid every other local surface uses — never the desktop
// tree. The tree is a 272px rail plus a detail pane; at 800×480 that is the
// whole screen spent on chrome, and the tree has no paged model behind it.
//
// This document has no native reader: it is the bounded folder set Rust
// publishes for the tab, so the legacy arm of the grid is the ONLY arm here,
// and it is not the stale-document defect the Albums/Artists/Tracks bodies had
// to fix — nothing else republishes over it.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    property var view: null

    readonly property var rows: root.view ? root.view.folders : []

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    function publishNav() {
        if (root.view)
            root.view.publishNav(Math.max(1, grid.columns), root.rows.length)
    }
    onRowsChanged: root.publishNav()
    Component.onCompleted: root.publishNav()
    Connections {
        target: grid
        function onColumnsChanged() { root.publishNav() }
    }

    KioskLocalAlbumGrid {
        id: grid
        anchors.fill: parent
        visible: !QbzLocal.localFoldersLoading && root.rows.length > 0
        view: root.view
        surface: "folders"
        scrollScope: "local:folders"
        pad: root.view ? root.view.pad : 16
        nativeActive: false
        rows: root.rows
        // A folder card opens through the local album route, exactly as the
        // desktop flat mode does.
        onOpen: function (id) {
            if (root.view)
                root.view.openAlbum(id)
        }
    }

    KioskSkeleton {
        anchors.fill: parent
        kind: "grid"
        columns: Math.max(2, grid.columns)
        pad: grid.pad
        gap: grid.gap
        loading: QbzLocal.localFoldersLoading
        empty: !QbzLocal.localFoldersLoading && root.rows.length === 0
        emptyText: root.t("No folders in your local library yet.")
    }
}
