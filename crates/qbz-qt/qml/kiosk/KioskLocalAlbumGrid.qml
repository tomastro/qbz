// KioskLocalAlbumGrid — the bounded album surface every Local Library kiosk
// tab renders its cards through (Albums, Folders, Genres results, the Artists
// drill-down).
//
// WHY THIS EXISTS INSTEAD OF KioskAlbumGrid. The kiosk primitive takes a JS
// ARRAY of albums. The Local Library's authoritative surface is the paged
// native model `QbzLocalAlbums` (and `QbzLocalArtistAlbums` for the artist
// drill-down), whose rows are already CHUNKED into `columns` cards by Rust —
// exactly the shape the desktop `LocalAlbumCollection` consumes. Feeding that
// model to a GridView is not possible without flattening it back into an
// array, which is the O(n) materialisation the native model exists to avoid.
// So this is the kiosk twin of LocalAlbumCollection: ONE windowing ListView
// whose delegate is one visual ROW of cards.
//
// It serves both readers, because both can be live in one session:
//   - NATIVE (`nativeActive`): model = the paged QAbstractListModel. Missing
//     pages arrive as `{loading:true}` rows and are drawn as static skeleton
//     tiles; the owner tab answers `pageMiss`.
//   - LEGACY (native disabled, or a surface that has no native model at all —
//     Folders and the Genres results): the rows array is chunked here, once
//     per (rows, columns) change, never per frame.
//
// BOUNDED BY CONSTRUCTION: the mounted delegate count is the viewport plus
// `cacheBuffer` (two card rows), never the model size. The cell count inside
// a row is the CONSTANT `columns`, so ListView delegate recycling actually
// recycles the cards instead of rebuilding them (the LocalAlbumCollection
// finding, reproduced here).
//
// ARTWORK: only the mounted band is reported, through the host's window
// registry (`view.queueWindowReport` / `view.releaseWindow`), so a tab that
// scrolls asks for the covers it is showing and lets go of the ones it is
// not. A grid that is not visible reports NOTHING.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    id: root

    /// The KioskLocalLibrary root — the artwork registry and `artPathOf` live
    /// there, because eviction is only correct against every live surface at
    /// once (the desktop `_windows` rationale).
    property var view: null
    /// Stable id for this cover surface in the host registry. Two instances
    /// can be alive at once (the Artists rail's drill-down grid and a tab
    /// grid behind it), and they must not share a slot.
    property string surface: "albums"
    /// ScrollMemory scope — "local:<tab>". Empty opts out (a grid that is not
    /// the page).
    property string scrollScope: ""

    /// Native paged reader.
    property bool nativeActive: false
    property var nativeModel: null
    /// Legacy rows (already ordered by the producer).
    property var rows: []

    /// Touch sizing. 168px is the smallest cell that keeps a card's tap area
    /// well past the 64px primary target at every supported width, and it
    /// yields 4 columns at 800px and 7 at 1280px.
    property real minCell: 168
    property real gap: 14
    property real pad: 16

    signal open(string id)

    QbzTheme { id: theme }

    // The Pi pane leaves only 192px below the tab strip. Keep a complete
    // cover + title + artist visible initially by adding columns when short.
    // The model still owns every album; only its row chunk size changes.
    readonly property real availableRowWidth: Math.max(0, root.width - 2 * root.pad + root.gap)
    readonly property real maxArtHeight: Math.max(64, root.height - root.pad - root.labelH)
    readonly property int columns: root.width > 0
        ? Math.min(Math.max(1, Math.floor(root.availableRowWidth / (64 + root.gap))),
                   Math.max(2, Math.floor(root.availableRowWidth / (root.minCell + root.gap)),
                            root.height > 0 ? Math.ceil(root.availableRowWidth / (root.maxArtHeight + root.gap)) : 1))
        : 0
    readonly property real cardW: root.columns > 0
        ? (root.width - 2 * root.pad - (root.columns - 1) * root.gap) / root.columns
        : 0
    /// Title + artist band under the square tile (KioskCard's own two lines).
    readonly property real labelH: 52
    readonly property real rowH: root.cardW + root.labelH + root.gap

    /// How many ALBUMS the surface holds — the count the host publishes into
    /// the nav geometry.
    readonly property int albumCount: root.nativeActive && root.nativeModel
        ? (root.nativeModel.albumTotal || 0) : root.rows.length
    /// How many ROWS the list mounts.
    readonly property int entryCount: root.nativeActive && root.nativeModel
        ? (root.nativeModel.totalCount || 0) : root.entries.length

    // ---------------------------------------------------------------------
    // Legacy chunking
    // ---------------------------------------------------------------------
    // One O(n) pass per (rows, columns) change, coalesced onto a zero-interval
    // Timer so a width settle followed by a document landing rebuilds once.
    property var entries: []
    function rebuild() {
        if (root.nativeActive) {
            root.entries = []
            root.report()
            return
        }
        if (root.columns <= 0) {
            // No width yet. Building against a guessed column count produces a
            // one-column grid that has to be thrown away a frame later.
            root.entries = []
            return
        }
        var out = []
        var source = root.rows
        for (var i = 0; i < source.length; i += root.columns)
            out.push({ "base": i, "items": source.slice(i, i + root.columns) })
        root.entries = out
        root.report()
    }
    Timer {
        id: rebuildCoalescer
        interval: 0
        repeat: false
        onTriggered: root.rebuild()
    }
    function scheduleRebuild() { rebuildCoalescer.restart() }
    onRowsChanged: root.scheduleRebuild()
    onColumnsChanged: root.scheduleRebuild()
    onNativeActiveChanged: { root.scheduleRebuild(); root.reportSoon() }

    /// A slot with no album. `visible: false` does not stop a binding from
    /// evaluating, so the tail cells of the last row bind against this frozen
    /// object instead of throwing a TypeError per evaluation.
    readonly property var emptySlot: ({
        "id": "", "title": "", "artist": "", "qualityTier": "",
        "artKey": "", "artPath": "", "artwork": "", "nativeIndex": -1
    })

    /// The KioskCard object for one native/legacy album row, with the artKey
    /// already resolved to a decoded local cover path.
    function cardOf(item) {
        if (!item || !item.id)
            return root.emptySlot
        return {
            "id": item.id,
            "title": item.title || "",
            "artist": item.artist || "",
            "qualityTier": item.qualityTier || "",
            "artwork": item.artPath
                || (root.view ? root.view.artPathOf(item.artKey) : "")
        }
    }

    // ---------------------------------------------------------------------
    // Focus (QbzKioskNav)
    // ---------------------------------------------------------------------
    // Same index space as the KioskAlbumGrid primitive: the leading `tabs`
    // entries are the tab strip, the rest are this surface's items.
    readonly property int focusedItem: QbzKioskNav.index - QbzKioskNav.tabs
    readonly property bool itemFocused: QbzKioskNav.navActive
        && QbzKioskNav.zone === "content"
        && root.focusedItem >= 0
        && root.focusedItem < root.albumCount

    /// Scroll the focused card's ROW in by the minimum amount, and do nothing
    /// when it is already in. Issued from a function on a settled layout
    /// (Qt.callLater), never from a binding that feeds layout — the
    /// "Recursion detected" panic class. `positionViewAtIndex` rather than
    /// hand-written contentY arithmetic, which does not survive the view's own
    /// margins and origin.
    function scrollFocusIntoView() {
        if (!root.itemFocused || root.columns <= 0)
            return
        list.positionViewAtIndex(Math.floor(root.focusedItem / root.columns),
                                 ListView.Contain)
    }

    Connections {
        target: QbzKioskNav
        function onIndexChanged() { Qt.callLater(root.scrollFocusIntoView) }
        function onActivateSeqChanged() {
            if (!root.itemFocused)
                return
            var id = root.idAtFlat(root.focusedItem)
            if (id !== "")
                root.open(id)
        }
    }

    /// The album id at a FLAT album index, without materialising the model.
    function idAtFlat(index) {
        if (root.columns <= 0)
            return ""
        if (root.nativeActive && root.nativeModel) {
            var entry = root.nativeModel.rowAt(Math.floor(index / root.columns))
            if (!entry || entry.loading || !entry.items)
                return ""
            var item = entry.items[index % root.columns]
            return item && item.id ? item.id : ""
        }
        var row = root.rows[index]
        return row && row.id ? row.id : ""
    }

    // ---------------------------------------------------------------------
    // Artwork window — the MOUNTED band only
    // ---------------------------------------------------------------------
    function report() {
        if (!root.view)
            return
        if (!list.visible || root.width <= 0 || root.entryCount === 0) {
            root.view.releaseWindow(root.surface)
            return
        }
        var first = list.indexAt(4, list.contentY + 1)
        var last = list.indexAt(4, list.contentY + Math.max(1, list.height) - 1)
        if (first < 0)
            first = Math.max(0, Math.floor((list.contentY - list.originY)
                                          / Math.max(1, root.rowH)))
        if (last < 0)
            last = Math.min(root.entryCount - 1, first + 3)
        // One row of overscan on each side; the host adds its own window-sized
        // keep margin on top of this before it evicts.
        first = Math.max(0, first - 1)
        last = Math.min(root.entryCount - 1, last + 1)

        var resident = []
        var i, j
        if (root.nativeActive && root.nativeModel) {
            for (i = first; i <= last; i++) {
                var entry = root.nativeModel.rowAt(i)
                if (!entry || entry.loading || !entry.items)
                    continue
                for (j = 0; j < entry.items.length; j++)
                    resident.push(entry.items[j])
            }
        } else {
            for (i = first; i <= last; i++) {
                var chunk = root.entries[i]
                if (!chunk || !chunk.items)
                    continue
                for (j = 0; j < chunk.items.length; j++)
                    resident.push(chunk.items[j])
            }
        }
        if (resident.length === 0)
            root.view.releaseWindow(root.surface)
        else
            root.view.queueWindowReport(resident, 0, resident.length - 1, root.surface)
    }

    /// `indexAt` answers -1 until the ListView has laid out, which is exactly
    /// the state a just-mounted or just-shown list is in: report now off the
    /// arithmetic fallback so covers start moving, then correct it.
    Timer {
        id: reportSettle
        interval: 50
        repeat: false
        onTriggered: root.report()
    }
    function reportSoon() {
        root.report()
        reportSettle.restart()
    }

    onVisibleChanged: {
        if (root.visible)
            root.reportSoon()
        else if (root.view)
            root.view.releaseWindow(root.surface)
    }
    onWidthChanged: root.reportSoon()
    onHeightChanged: root.report()
    Component.onCompleted: { root.rebuild(); root.reportSoon() }
    Component.onDestruction: if (root.view) root.view.releaseWindow(root.surface)
    Connections {
        target: root.nativeModel
        ignoreUnknownSignals: true
        function onDataChanged() { root.reportSoon() }
        function onModelReset() { root.reportSoon() }
    }
    Connections {
        target: root.view
        function onArtworkRefresh() { root.reportSoon() }
    }

    // ---------------------------------------------------------------------
    // The list
    // ---------------------------------------------------------------------
    ListView {
        id: list
        anchors.fill: parent
        clip: true
        topMargin: root.pad
        bottomMargin: root.pad
        // One row per side: at most two additional card rows.
        cacheBuffer: Math.max(1, Math.round(root.rowH))
        boundsBehavior: Flickable.StopAtBounds
        reuseItems: true
        model: root.visible ? (root.nativeActive && root.nativeModel ? root.nativeModel : root.entries) : []

        onContentYChanged: root.report()
        onModelChanged: root.report()
        onHeightChanged: root.report()
        onVisibleChanged: if (visible) root.reportSoon()

        delegate: Item {
            id: rowSlot
            required property var modelData
            required property int index

            width: list.width
            height: root.rowH

            readonly property bool rowLoading: rowSlot.modelData
                && rowSlot.modelData.loading === true
            readonly property int rowBase: rowSlot.modelData
                && typeof rowSlot.modelData.base === "number"
                    ? rowSlot.modelData.base
                    : rowSlot.index * Math.max(1, root.columns)

            // A page that has not landed yet: static tiles, no image, no text
            // measurement. Never an animation — the kiosk pulse budget is the
            // shared shell pulse and a skeleton is not allowed to spend it.
            Row {
                anchors.left: parent.left
                anchors.leftMargin: root.pad
                anchors.top: parent.top
                spacing: root.gap
                visible: rowSlot.rowLoading

                Repeater {
                    model: rowSlot.rowLoading ? root.columns : 0
                    delegate: Rectangle {
                        width: root.cardW
                        height: root.cardW
                        radius: theme.radiusSm
                        color: theme.surfaceElevated
                    }
                }
            }

            Row {
                anchors.left: parent.left
                anchors.leftMargin: root.pad
                anchors.top: parent.top
                spacing: root.gap
                visible: !rowSlot.rowLoading

                // FIXED CELL COUNT. Handing this Repeater the row's `items`
                // array would hand it a NEW array on every recycle, which
                // destroys and rebuilds the cards — the expensive half — and
                // makes the ListView's `reuseItems` buy nothing.
                Repeater {
                    model: root.columns

                    delegate: Item {
                        id: cell
                        required property int index

                        width: root.cardW
                        height: root.cardW + root.labelH

                        readonly property var item:
                            (rowSlot.modelData && rowSlot.modelData.items)
                                ? (rowSlot.modelData.items[cell.index] || null)
                                : null
                        readonly property var card: root.cardOf(cell.item)
                        readonly property int flatIndex: cell.item
                            && typeof cell.item.nativeIndex === "number"
                            && cell.item.nativeIndex >= 0
                                ? cell.item.nativeIndex
                                : rowSlot.rowBase + cell.index

                        visible: cell.item !== null && cell.card.id !== ""

                        KioskCard {
                            artSize: root.cardW
                            album: cell.card
                            navFocused: root.itemFocused
                                && root.focusedItem === cell.flatIndex
                            onClicked: function (id) {
                                if (id !== "")
                                    root.open(id)
                            }
                        }
                    }
                }
            }
        }
    }

    // Back/Forward scroll memory. The scope carries the TAB ("local:albums"),
    // so returning to a different tab cannot pour this offset into it.
    ScrollMemory { target: list; scope: root.scrollScope; relativeToOrigin: true }
}
