// PmToolbar — the Playlist Manager's right-hand tool cluster
// (PlaylistManagerView.slint:1123-1277): search · filter dropdown · sort
// dropdown (with the #657 direction caret) · folder/flat toggle · view-mode
// cycle · Import · New folder. Spacing 8, everything 34 px tall.
//
// "Import" is the one slot with no counterpart in the reference — see its own
// comment below.
//
// EVERY input it renders is declared on THIS root (contract §2.5). QML gives a
// component no access to another file's ids, so a binding written against the
// view's `root` would resolve to `undefined` here — the filter button would
// read "All" forever and the cycle button would get an empty icon name, both
// silently.
//
// Reuse, with the rejected alternatives named:
//   * search — `controls/QbzLineEdit.qml` in its `searchMode` arm. Accepted
//     sub-2 px deltas inherited from the port's ONE consolidated search box:
//     glyph 14 vs 15, padding L9/R5 vs L10/R6, spacing 7 vs 8, text 12 vs
//     13 px, clear disc 24 vs 22. Re-tuning them here would fragment exactly
//     what QbzLineEdit.qml:17-19 consolidated.
//   * the four buttons — `controls/QbzToolButton.qml` with its `large` arm
//     (contract D14). Rejected: `ViewModeToggle` (a two-segment grid/list pair
//     that cannot express three modes or the next-icon affordance),
//     `QbzSegToggle` / `QbzTabBar` (no segmented control and no tabs exist
//     here), `QbzIconButton` (transparent idle, r8, no accent-fill state) and
//     `QbzToggleButton` (its active fill is `surfaceHover` on a transparent
//     idle — the exact inverse).
//   * the two dropdown panels — `controls/QbzContextMenu.qml` with
//     `openBelowLeft(trigger)`, which places at `x: 0, y: height + 4`; with a
//     34 px trigger that is EXACTLY the reference's `x: 0; y: 38px`.
//
// SEARCH TEXT IS QML-LOCAL (contract D8). It is seeded ONCE from
// `managerJson.search` so a session query survives re-entry, and it is never
// bound to the document: every keystroke re-serialises the whole document, and
// a late publish landing mid-typing would fight the user. Rust debounces the
// rebuild by 150 ms; every other toolbar action publishes immediately.
//
// NO ANIMATIONS anywhere in this half — the reference snaps. Do not add a
// `Behavior on color` to the buttons.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root

    /// Seeds the QML-local field ONCE, on mount (D8).
    property string searchSeed: ""
    property string filter: "all"
    property string sort: "name"
    property bool sortAsc: true
    property string viewMode: "grid"
    property bool folderMode: true

    /// The cycle button shows the NEXT mode's icon (`:1080-1087`). The
    /// unreachable `!folderMode && viewMode === "tree"` state shows a grid icon
    /// while yielding "list" — dead code in the reference, ported as written.
    readonly property string nextViewIcon:
          root.viewMode === "grid" ? "list"
        : root.viewMode === "list" ? (root.folderMode ? "network" : "layout-grid")
        : "layout-grid"
    readonly property string nextViewMode:
          root.folderMode
            ? (root.viewMode === "grid" ? "list"
               : root.viewMode === "list" ? "tree" : "grid")
            : (root.viewMode === "list" ? "grid" : "list")

    /// The live field text. Seeded from `searchSeed`, then owned here.
    property string searchText: ""
    Component.onCompleted: root.searchText = root.searchSeed

    QbzTheme { id: theme }

    implicitWidth: tools.implicitWidth
    implicitHeight: 34
    width: implicitWidth
    height: implicitHeight

    // A plain Row, never a RowLayout: the New-folder slot appears and
    // disappears with folder mode, and `Row` skips invisible children so the
    // right-anchored cluster shifts as one block (the reference is
    // right-anchored too — `:1124`).
    Row {
        id: tools
        height: parent.height
        spacing: 8

        QbzLineEdit {
            anchors.verticalCenter: parent.verticalCenter
            width: 168
            searchMode: true
            placeholder: QbzSession.tr("Search playlists…", QbzSession.trRev)
            // Bound to the LOCAL state, which is also what its focus-loss
            // re-seed Binding reads — binding it to the document would put the
            // last published query back under the user's cursor.
            text: root.searchText
            onEdited: function (v) {
                root.searchText = v
                QbzPlaylistManager.searchChanged(v)
            }
        }

        QbzToolButton {
            id: filterBtn
            anchors.verticalCenter: parent.verticalCenter
            large: true
            fillActive: true
            name: "list-filter"
            label: root.filter === "visible"
                ? QbzSession.tr("Visible", QbzSession.trRev)
                : root.filter === "hidden"
                    ? QbzSession.tr("Hidden", QbzSession.trRev)
                    : QbzSession.tr("All", QbzSession.trRev)
            onClicked: filterMenu.openBelowLeft(filterBtn)
        }

        QbzToolButton {
            id: sortBtn
            anchors.verticalCenter: parent.verticalCenter
            large: true
            fillActive: true
            name: "arrow-up-down"
            // The closed trigger shows NO direction indicator — the caret lives
            // inside the open menu, on the active row only (§5.17).
            label: root.sort === "recent"
                ? QbzSession.tr("Recent", QbzSession.trRev)
                : root.sort === "playcount"
                    ? QbzSession.tr("Play Count", QbzSession.trRev)
                    : root.sort === "tracks"
                        ? QbzSession.tr("Track Count", QbzSession.trRev)
                        : root.sort === "custom"
                            ? QbzSession.tr("Custom", QbzSession.trRev)
                            : QbzSession.tr("Name A-Z", QbzSession.trRev)
            onClicked: sortMenu.openBelowLeft(sortBtn)
        }

        // Folder / flat. The ONLY toolbar button with an active (accent) state.
        QbzToolButton {
            anchors.verticalCenter: parent.verticalCenter
            large: true
            fillActive: true
            label: ""
            name: root.folderMode ? "folder-open" : "rows-3"
            active: root.folderMode
            onClicked: QbzPlaylistManager.toggleFolderMode()
        }

        // View-mode cycle.
        QbzToolButton {
            anchors.verticalCenter: parent.verticalCenter
            large: true
            fillActive: true
            label: ""
            name: root.nextViewIcon
            onClicked: QbzPlaylistManager.setViewMode(root.nextViewMode)
        }

        // Import a public playlist (Spotify / Apple / Deezer / Tidal / YouTube).
        //
        // ADDITION, not a port — the reference toolbar has no such button
        // (owner request 2026-08-16). Until now the importer had exactly two
        // doors, both inside the sidebar's playlist popup (Sidebar.qml:881 and
        // HeaderBar.qml:695), so the one view whose whole job is managing
        // playlists could not reach it.
        //
        // Placed BEFORE "New folder" on purpose: that slot disappears in flat
        // mode, and a Row skips invisible children, so a button after it would
        // slide sideways every time the mode toggles.
        //
        // No offline gate here. The other two doors dim their row, but the
        // modal recomputes the gate on open (playlist_import_qt.rs:348-352) and
        // shows a disabled confirm with the reason — which beats a toolbar
        // button that silently vanishes when the network drops.
        QbzToolButton {
            anchors.verticalCenter: parent.verticalCenter
            large: true
            fillActive: true
            name: "import"
            label: QbzSession.tr("Import", QbzSession.trRev)
            onClicked: QbzPlaylistImport.open()
        }

        // New folder — folder mode only (`:1269`).
        QbzToolButton {
            visible: root.folderMode
            anchors.verticalCenter: parent.verticalCenter
            large: true
            fillActive: true
            name: "folder-plus"
            label: QbzSession.tr("New folder", QbzSession.trRev)
            onClicked: QbzFolderEdit.openNewFolder()
        }
    }

    // ---- the two dropdown panels -----------------------------------------
    // Every row handler ends with `close()`: Slint's PopupWindow closes on any
    // inside click, Qt's `CloseOnPressOutside` does not (contract D15). This is
    // the single most common silent divergence in this block.

    QbzContextMenu {
        id: filterMenu
        menuWidth: 160
        contentSpacing: 1

        Repeater {
            model: [
                { "id": "all", "label": QbzSession.tr("All", QbzSession.trRev) },
                { "id": "visible", "label": QbzSession.tr("Visible", QbzSession.trRev) },
                { "id": "hidden", "label": QbzSession.tr("Hidden", QbzSession.trRev) }
            ]
            delegate: PmMenuRow {
                required property var modelData
                width: filterMenu.menuWidth - 10
                label: modelData.label
                selected: root.filter === modelData.id
                trailing: "check"
                onActivated: {
                    QbzPlaylistManager.setFilter(modelData.id)
                    filterMenu.close()
                }
            }
        }
    }

    QbzContextMenu {
        id: sortMenu
        menuWidth: 180
        contentSpacing: 1

        Repeater {
            model: [
                { "id": "name", "label": QbzSession.tr("Name A-Z", QbzSession.trRev) },
                { "id": "recent", "label": QbzSession.tr("Recent", QbzSession.trRev) },
                { "id": "playcount", "label": QbzSession.tr("Play Count", QbzSession.trRev) },
                { "id": "tracks", "label": QbzSession.tr("Track Count", QbzSession.trRev) },
                { "id": "custom", "label": QbzSession.tr("Custom", QbzSession.trRev) }
            ]
            delegate: PmMenuRow {
                required property var modelData
                width: sortMenu.menuWidth - 10
                label: modelData.label
                selected: root.sort === modelData.id
                // The 13 px accent chevron replaces the check on the ACTIVE
                // row; re-selecting it is what flips the direction, in Rust.
                trailing: "caret"
                sortAsc: root.sortAsc
                onActivated: {
                    QbzPlaylistManager.setSort(modelData.id)
                    sortMenu.close()
                }
            }
        }
    }
}
