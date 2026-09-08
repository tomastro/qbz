// PlaylistManagerView — the Playlist Manager, the port of
// playlist/PlaylistManagerView.slint (1498 lines). Route id
// "playlistmanager", reached from Sidebar > Playlists ⋯ > Manage playlists.
//
// A fixed 56 px title/toolbar bar over ONE scrolling surface with three view
// modes (grid / list / tree) and two toggles (folder mode, folders collapsed).
// Every row arrives ready to render from Rust — this file does no per-row map
// lookup, no pluralisation and no filtering.
//
// ── THE SCROLL ARCHITECTURE (contract D1) ─────────────────────────────────
// ONE `ListView` in all three modes, with the whole page chrome as its
// `header`, a footer spacer, and one delegate per ROW: a row of grid cards, a
// list row, or a tree row. NOT a GridView inside a Flickable — nesting
// Flickables gives nested flicking and two scrollbars, and buys no recycling
// (an inner view sized to its full content has a viewport equal to its content,
// so every delegate instantiates anyway). One ListView is also the only shape
// that keeps the scroll position across a view-mode switch, which the
// reference's single Flickable does for free.
//
// `ListView.spacing` is 0 and every gap lives INSIDE a delegate's height,
// because ListView.spacing would also apply between the header and row 0, where
// the reference has 14 px, not 16. The footer is therefore `100 − gap` per mode
// (84 / 96 / 98): the last delegate contributes its own trailing gap.
//
// `model: 0` while `pmLoading` is what reproduces the reference's `!loading`
// gate on the whole body; the header carries its own `!loading` gates.
//
// ── NEVER WRAPPED IN QbzOfflinePlaceholder (contract §5.12) ───────────────
// Seven views wrap themselves and one (PlayHistoryView) explicitly refuses
// because it reads a local store. This is the second refusal, and the stronger
// one: the manager keeps working offline, and for a user with no Qobuz account
// the LOCAL playlists it lists are the only library they have. An implementer
// following the majority would hide exactly the view that user needs.
//
// The two counts in the counter row come from the document; the folder cards'
// `count` is TOTAL membership while a tree folder row's is the POST-FILTER
// member count (§5.7) — two different questions from one field name, on
// purpose.
//
// Scroll-position restore is an INHERITED GAP: the reference restores and
// reports `viewport-y` for scope "playlist-manager" (`:1285-1296`); no view in
// this port restores scroll and `nav_qt.rs` has no equivalent seam. Recorded
// (contract D30), not silently dropped.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"
import "playlistmanager"

Rectangle {
    id: root
    property bool kioskHost: false

    // Round to the AppShell content-frame bezel: QML clips are rectangular, so
    // the frame's rounding never reaches the view — the view's own fill must
    // round (the literal in all 14 painting view roots).
    radius: 12
    color: root.ambientOn ? "transparent" : theme.surfaceMain
    readonly property bool ambientOn: theme.ambientOn

    QbzTheme { id: theme }

    // ============================== documents =============================
    // Both parses are GUARDED — a raw JSON.parse in a binding throws on the
    // pre-publish frame.

    readonly property var doc: {
        try {
            return JSON.parse(QbzPlaylistManager.managerJson)
        } catch (e) {
            return ({})
        }
    }
    /// The CANONICAL folder array, its own property (contract D5): the sidebar
    /// needs it before — and without — the manager document.
    readonly property var folders: {
        try {
            return JSON.parse(QbzPlaylistManager.foldersJson)
        } catch (e) {
            return []
        }
    }

    // `!== false`, NOT `=== true`, on the two that default TRUE upstream: a `{}`
    // first frame would otherwise flash flat mode and a descending sort on every
    // mount.
    readonly property string searchSeed: root.doc.search || ""
    readonly property string filter: root.doc.filter || "all"
    readonly property string sort: root.doc.sort || "name"
    readonly property bool sortAsc: root.doc.sortAsc !== false
    readonly property string viewMode: root.doc.viewMode || "grid"
    readonly property bool folderMode: root.doc.folderMode !== false
    readonly property bool foldersCollapsed: root.doc.foldersCollapsed === true
    readonly property string currentFolderId: root.doc.currentFolderId || ""
    readonly property string currentFolderName: root.doc.currentFolderName || ""
    /// Computed in RUST (`sort == "custom" && sortAsc && query empty`, D9);
    /// QML must never re-derive it.
    readonly property bool canReorder: root.doc.canReorder === true
    readonly property int folderCount: root.doc.folderCount || 0
    readonly property int playlistCount: root.doc.playlistCount || 0
    readonly property var playlists: root.doc.playlists || []
    /// EMPTY unless folderMode && viewMode === "tree".
    readonly property var tree: root.doc.tree || []

    // ---- geometry --------------------------------------------------------
    readonly property int contentW: Math.max(0, pmList.width - 64)
    readonly property int gridCols: Math.max(1, Math.floor((root.contentW + 16) / (root.kioskHost ? 216 : 176)))
    readonly property int cardH: root.kioskHost ? (root.canReorder ? 320 : 270) : (root.canReorder ? 250 : 220)
    readonly property int rowsInBody:
        root.viewMode === "tree" ? root.tree.length : root.playlists.length
    readonly property int rowCount:
          QbzPlaylistManager.pmLoading ? 0
        : root.viewMode === "grid"
            ? Math.ceil(root.playlists.length / Math.max(1, root.gridCols))
        : root.rowsInBody
    /// The page's 100 px bottom padding, minus the trailing gap the last
    /// delegate already carries.
    readonly property int footerH:
        root.viewMode === "grid" ? 84 : root.viewMode === "list" ? 96 : 98

    // `navigate()` records the route and raises the spinner but does NOT load;
    // this is the single load, and it is also the back/forward re-entry path
    // (nav_qt::back()/forward() republish `currentView` and run no per-view
    // load, so `navigate()` never ran in that case).
    Component.onCompleted: QbzPlaylistManager.reload()

    // ============================ fixed header ============================
    Item {
        id: headerBar
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: root.kioskHost ? 64 : 56

        // The reference paints this bar an opaque `surface-main` while the view
        // root goes transparent (`:1089`, `:1097`) — under the dynamic
        // background that is a solid band across the ambient field. Every Qt
        // view's fixed header takes the A30 treatment instead.
        Rectangle {
            anchors.fill: parent
            // Top-rounded: full-bleed at y=0 of the content pane, whose own
            // rounding cannot reach a child (Qt's clip is a rectangular
            // scissor). See the longer note in HomeView.
            topLeftRadius: theme.radiusMd
            topRightRadius: theme.radiusMd
            color: root.ambientOn ? theme.surfaceMainA30 : theme.surfaceMain
        }

        // x 48, not the reference's 32: this port has NO per-view NavButtons
        // (nav history is the global HeaderBar, the treatment
        // BlacklistManagerView.qml:23-26 and settings/SettingsView.qml:24-25
        // already document; ADR-004 is satisfied there). 48 is the number
        // views/PlaylistBrowseView.qml:143 chose for the byte-identical Slint
        // header, so the port's two 56 px headers align.
        Text {
            visible: !root.kioskHost
            x: 48
            y: 25 - height / 2
            width: Math.max(0, tools.x - 48 - 16)
            text: QbzSession.tr("Playlist manager", QbzSession.trRev)
            color: theme.textPrimary
            font.pixelSize: theme.fontSection
            font.weight: theme.weightBold
            elide: Text.ElideRight
        }

        Row {
            visible: root.kioskHost
            x: 16
            height: 64
            spacing: 12
            Text { width: 190; height: 64; text: QbzSession.tr("Playlist manager",QbzSession.trRev); color: theme.textPrimary; font.pixelSize: 20; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
            QbzLineEdit { kioskHost: true; width: Math.max(120,root.width-306); anchors.verticalCenter: parent.verticalCenter; searchMode: true; text: root.searchSeed; placeholder: QbzSession.tr("Search playlists…",QbzSession.trRev); onEdited: function(v) { QbzPlaylistManager.searchChanged(v) } }
            SettingsButton { id: kioskActions; kioskHost: true; minWidth: 64; btnHeight: 64; text: "⋯"; onClicked: kioskMenu.openBelowRight(kioskActions) }
        }

        PmToolbar {
            id: tools
            visible: !root.kioskHost
            x: Math.max(0, parent.width - width - 32)
            y: 25 - height / 2
            searchSeed: root.searchSeed
            filter: root.filter
            sort: root.sort
            sortAsc: root.sortAsc
            viewMode: root.viewMode
            folderMode: root.folderMode
        }
    }

    CardMenu {
        id: kioskMenu
        kioskHost: root.kioskHost
        menuWidth: Math.min(root.width-24,320)
        entries: [
            {label: QbzSession.tr("Import",QbzSession.trRev),action:"import"},
            {label: QbzSession.tr("New folder",QbzSession.trRev),action:"new-folder"},
            {label: QbzSession.tr("Folders",QbzSession.trRev),action:"folders"},
            {label: QbzSession.tr("Grid",QbzSession.trRev),action:"view:grid"},
            {label: QbzSession.tr("List",QbzSession.trRev),action:"view:list"},
            {label: QbzSession.tr("Tree",QbzSession.trRev),action:"view:tree"},
            {label: QbzSession.tr("All",QbzSession.trRev),action:"filter:all"},
            {label: QbzSession.tr("Visible",QbzSession.trRev),action:"filter:visible"},
            {label: QbzSession.tr("Hidden",QbzSession.trRev),action:"filter:hidden"},
            {label: QbzSession.tr("Name",QbzSession.trRev),action:"sort:name"},
            {label: QbzSession.tr("Tracks",QbzSession.trRev),action:"sort:tracks"},
            {label: QbzSession.tr("Recent",QbzSession.trRev),action:"sort:recent"},
            {label: QbzSession.tr("Play Count",QbzSession.trRev),action:"sort:playcount"},
            {label: QbzSession.tr("Custom",QbzSession.trRev),action:"sort:custom"}
        ]
        onPicked: function(a) {
            if(a==="import") QbzPlaylistImport.open()
            else if(a==="new-folder") QbzFolderEdit.openNewFolder()
            else if(a==="folders") QbzPlaylistManager.toggleFolderMode()
            else if(a.indexOf("view:")===0) QbzPlaylistManager.setViewMode(a.slice(5))
            else if(a.indexOf("filter:")===0) QbzPlaylistManager.setFilter(a.slice(7))
            else if(a.indexOf("sort:")===0) QbzPlaylistManager.setSort(a.slice(5))
        }
    }

    // ================================ body ================================
    Item {
        id: bodyHost
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: headerBar.bottom
        anchors.bottom: parent.bottom

        ListView {
            id: pmList
            anchors.fill: parent
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            spacing: 0
            // ~2 grid rows of pre-instantiation past the viewport.
            cacheBuffer: root.kioskHost ? 128 : 400
            reuseItems: true

            // An INT model in all three modes: the delegates read
            // `root.playlists[index]` / `root.tree[index]`, so a republish that
            // keeps the row count re-evaluates their bindings instead of
            // rebuilding the list, and the grid's row delegate can slice.
            model: root.rowCount

            header: PmPageHead {
                kioskHost: root.kioskHost
                viewport: pmList
                width: pmList.width
                loading: QbzPlaylistManager.pmLoading
                folders: root.folders
                viewMode: root.viewMode
                folderMode: root.folderMode
                foldersCollapsed: root.foldersCollapsed
                currentFolderId: root.currentFolderId
                currentFolderName: root.currentFolderName
                canReorder: root.canReorder
                folderCount: root.folderCount
                playlistCount: root.playlistCount
                rowsInBody: root.rowsInBody
            }
            footer: Item {
                width: 1
                height: root.footerH
            }

            // Assigning `delegate` from a ternary re-creates every delegate on a
            // mode switch — which is exactly right here, and cheaper than a
            // per-delegate Loader.
            delegate: root.viewMode === "grid" ? gridRowDelegate
                : root.viewMode === "tree" ? treeRowDelegate
                : listRowDelegate
        }

        // The control's own rule is `visible: maxScroll > 0` (its auto-hide, =
        // the Slint ListScrollbar's `auto-hide: true`). NEVER assign `visible`
        // at this call site — that replaces the binding and a short page gets a
        // full-height thumb on gutter hover.
        // Back/forward scroll memory (controls/ScrollMemory.qml): reports
        // this container's offset while it is the live page, and restores it
        // when a back/forward step arms this route.
        ScrollMemory { target: pmList; scope: "playlistmanager" }
        QbzScrollBar {
            target: pmList
            anchors.right: parent.right
            anchors.rightMargin: 4
            anchors.top: parent.top
            anchors.bottom: parent.bottom
        }
    }

    // ============================= delegates ==============================

    // GRID — one delegate is one ROW of cards. This is what makes a card grid
    // windowed without a GridView (which could not carry a header this tall and
    // would force one uniform cell height across the 220/250 split).
    Component {
        id: gridRowDelegate

        Item {
            id: cell
            required property int index

            // FULL-WIDTH DELEGATE, INSET ON THE CHILD — the shape the list and
            // tree delegates below already use, and the reason this one is
            // written the same way now:
            //
            // this delegate used to carry `x: 32` ITSELF. A ListView owns its
            // delegates' position, so an `x` set on the delegate root is not
            // reliably the one that survives layout — and it did not here: the
            // card grid sat ~24 px left of the section headings above it, which
            // `PmPageHead` draws at 32 through its own Column. Headings at one
            // inset and cards at another, on the same page.
            //
            // The inset now lives where the view has no opinion about it.
            width: pmList.width
            height: root.cardH + 16

            // A BINDING, not a function call: it must re-evaluate when the
            // window resizes (gridCols) or the document republishes.
            readonly property var slice: root.playlists.slice(
                cell.index * root.gridCols, (cell.index + 1) * root.gridCols)

            Row {
                x: 32
                width: root.contentW
                spacing: 16

                Repeater {
                    model: cell.slice

                    delegate: PmGridCard {
                        kioskHost: root.kioskHost
                        required property var modelData
                        required property int index

                        item: modelData
                        // ABSOLUTE index in the visible order — the reorder
                        // arrows need the position in the whole set, not the
                        // column within this row.
                        itemIndex: cell.index * root.gridCols + index
                        canReorder: root.canReorder
                    }
                }
            }
        }
    }

    // LIST — 64 px row + the 4 px inter-row gap.
    Component {
        id: listRowDelegate

        Item {
            id: lrow
            required property int index

            readonly property var entry: root.playlists[lrow.index] || ({})

            width: pmList.width
            height: root.kioskHost && root.canReorder ? 92 : 68

            PmListRow {
                kioskHost: root.kioskHost
                x: 32
                width: root.contentW
                item: lrow.entry
                itemIndex: lrow.index
                canReorder: root.canReorder
                foldersCount: root.folders.length
                // The ONE menu lives at the view level (D17) — the row asks,
                // the view opens.
                onFolderMenuRequested: function (anchorItem) {
                    folderMenu.openFor(String(lrow.entry.id || ""),
                                       String(lrow.entry.folderId || ""),
                                       anchorItem)
                }
            }
        }
    }

    // TREE — 44 px folder rows and 40 px playlist rows, + the 2 px gap. A
    // Loader picks the arm so a folder row never instantiates a collage and a
    // playlist row never instantiates a folder tile.
    Component {
        id: treeRowDelegate

        Item {
            id: trow
            required property int index

            readonly property var entry: root.tree[trow.index] || ({})
            readonly property bool isFolder: trow.entry.kind === "folder"

            width: pmList.width
            height: (root.kioskHost ? 64 : (trow.isFolder ? 44 : 40)) + 2

            Loader {
                x: 32
                width: root.contentW
                height: root.kioskHost ? 64 : (trow.isFolder ? 44 : 40)
                sourceComponent: trow.isFolder ? treeFolderArm : treePlaylistArm
            }

            Component {
                id: treeFolderArm

                PmTreeFolderRow {
                    kioskHost: root.kioskHost
                    folder: trow.entry.folder || ({})
                    expanded: trow.entry.expanded === true
                }
            }
            Component {
                id: treePlaylistArm

                PmTreePlaylistRow {
                    kioskHost: root.kioskHost
                    item: trow.entry.playlist || ({})
                    indent: trow.entry.indent === true
                }
            }
        }
    }

    // ======================== the ONE folder menu =========================
    // Declared last (declaration order is z-order) and mounted ONCE: with
    // `reuseItems: true` a per-row menu would give every recycled delegate its
    // own Popup.
    PmFolderMenu {
        id: folderMenu
        kioskHost: root.kioskHost
        folders: root.folders
    }
}
