// PurchaseAlbumsCollection — one collection of purchased albums, drawn as a
// grid or as a list. 1:1 port of purchases/PurchasesView.slint's
// `AlbumsCollection` (:1155-1210).
//
// "One collection" = either the whole flat set, or ONE group's albums when the
// toolbar's grouping is on; the host mounts as many of these as there are
// groups, which is what keeps the group headers and their bodies in one place.
//
// GEOMETRY, off the .slint: grid cards 162x232 on a 16px gap (the Tauri
// `auto-fill minmax(162px, 1fr)` column math, reproduced exactly), list rows
// 60px on a 4px gap. The root is content-sized in both arms so the page's one
// Flickable owns the scroll — there is no nested scroller here.
//
// WINDOWING: the outer PurchasesView Flickable owns scrolling, so a nested
// GridView/ListView would consider its whole content visible and instantiate
// everything. Keep the full cheap footprint here, but mount only a two-viewport
// runway around the visible band. The same band drives artwork requests, so an
// account with thousands of purchases pays only for the cells near the screen.

import QtQuick
import com.blitzfc.qbz
import "../../theme"

Item {
    id: root

    /// Rows of `list_json.albums` (§G.2) — already searched / sorted / filtered
    /// by the controller.
    property var albums: []
    /// "grid" | "list".
    property string viewMode: "grid"
    /// Outer scrolling host. Null keeps the bounded/eager fallback.
    property Flickable flick: null
    signal openAlbum(string albumId)

    QbzTheme { id: theme }

    readonly property int cardWidth: 162
    readonly property int cardHeight: 232
    readonly property int cardGap: 16
    readonly property int listRowHeight: 60
    readonly property int listGap: 4
    /// The column-label strip above the rows (PurchaseListHeader), list arm only.
    readonly property int listHeaderHeight: 28

    readonly property int columns: Math.max(
        1, Math.floor((width + root.cardGap) / (root.cardWidth + root.cardGap)))
    readonly property int gridRows:
        Math.ceil(root.albums.length / Math.max(1, root.columns))

    // --- bounded visible band ---------------------------------------------
    property int bandFirst: 0
    property int bandLast: -1
    readonly property bool windowed: root.flick !== null

    function metrics() {
        var listMode = root.viewMode === "list"
        var pitch = listMode ? root.listRowHeight + root.listGap
                             : root.cardHeight + root.cardGap
        var p = root.mapToItem(root.flick.contentItem, 0, 0)
        return ({ "pitch": pitch,
                  "top": p.y + (listMode ? root.listHeaderHeight + root.listGap : 0),
                  "cols": listMode ? 1 : root.columns })
    }

    function sampleBand() {
        if (!root.windowed || !root.visible || root.flick.height <= 0)
            return
        var m = root.metrics()
        var localTop = root.flick.contentY - m.top
        var h = root.flick.height
        var totalRows = Math.ceil(root.albums.length / m.cols)
        var runwayTop = localTop - 2 * h
        var runwayBottom = localTop + 3 * h
        if (totalRows <= 0 || runwayBottom <= 0
                || runwayTop >= totalRows * m.pitch) {
            root.bandFirst = 0
            root.bandLast = -1
            return
        }
        root.bandFirst = Math.max(0, Math.floor(runwayTop / m.pitch))
        root.bandLast = Math.min(totalRows - 1,
            Math.max(root.bandFirst, Math.ceil(runwayBottom / m.pitch) - 1))
        root.reportArtWindow()
    }

    function ensureBandCoverage() {
        if (!root.windowed || !root.visible || root.flick.height <= 0)
            return
        var m = root.metrics()
        var localTop = root.flick.contentY - m.top
        var h = Math.max(1, root.flick.height)
        var totalRows = Math.ceil(root.albums.length / m.cols)
        if (totalRows <= 0 || localTop + 3 * h <= 0
                || localTop - 2 * h >= totalRows * m.pitch)
            return
        var first = Math.max(0, Math.floor(localTop / m.pitch))
        var last = Math.max(0, Math.ceil((localTop + h) / m.pitch))
        var runway = Math.max(1, Math.ceil(h / m.pitch))
        if (first < root.bandFirst || last > root.bandLast
                || (first > runway && first - root.bandFirst < runway)
                || (last < totalRows - runway && root.bandLast - last < runway))
            root.sampleBand()
    }

    Connections {
        target: root.flick
        ignoreUnknownSignals: true
        function onContentYChanged() { root.ensureBandCoverage() }
        function onHeightChanged() { root.sampleBand() }
    }
    Component.onCompleted: root.sampleBand()
    onAlbumsChanged: root.sampleBand()
    onViewModeChanged: root.sampleBand()
    onVisibleChanged: root.sampleBand()
    onWidthChanged: root.sampleBand()
    onYChanged: root.sampleBand()

    // --- viewport artwork --------------------------------------------------
    property var artMap: ({})
    readonly property var artAsked: ({ "seen": ({}) })

    function artOf(album) {
        if (!album)
            return ""
        var url = album.artworkUrl || ""
        return (url !== "" && root.artMap[url])
            ? root.artMap[url] : (album.artPath || "")
    }

    function reportArtWindow() {
        if (!root.windowed || root.albums.length === 0
                || root.bandLast < root.bandFirst)
            return
        var cols = root.viewMode === "list" ? 1 : root.columns
        var lo = Math.max(0, root.bandFirst * cols)
        var hi = Math.min(root.albums.length - 1, (root.bandLast + 1) * cols - 1)
        var pending = []
        var seen = root.artAsked.seen
        for (var i = lo; i <= hi; i++) {
            var album = root.albums[i] || ({})
            var url = album.artworkUrl || ""
            if (url === "" || (album.artPath || "") !== ""
                    || root.artMap[url] || seen[url] === true)
                continue
            seen[url] = true
            pending.push(url)
        }
        if (pending.length > 0)
            QbzShell.sidebarArtworkWindow(JSON.stringify(pending))
    }

    Connections {
        target: QbzLibrary
        function onLibraryArtworkReady(key, path) {
            if (root.artAsked.seen[key] !== true || root.artMap[key] === path)
                return
            var next = Object.assign({}, root.artMap)
            next[key] = path
            root.artMap = next
        }
    }

    width: parent ? parent.width : 0
    // Content-sized: the page Flickable reads this through its Column.
    height: root.viewMode === "list"
        ? (root.albums.length > 0
            ? root.listHeaderHeight + root.listGap
              + root.albums.length * root.listRowHeight
              + (root.albums.length - 1) * root.listGap
            : 0)
        : (root.gridRows > 0
            ? root.gridRows * root.cardHeight + (root.gridRows - 1) * root.cardGap
            : 0)

    // --- Grid -------------------------------------------------------------
    Item {
        id: grid
        visible: root.viewMode !== "list"
        anchors.fill: parent
        readonly property int mountedFrom: root.windowed
            ? Math.min(root.albums.length, root.bandFirst * root.columns) : 0
        readonly property int mountedTo: root.windowed
            ? Math.min(root.albums.length,
                       Math.max(mountedFrom, (root.bandLast + 1) * root.columns))
            : root.albums.length
        Repeater {
            // The model is gated on the arm so the hidden arm builds no
            // delegates at all (the AlbumCollection convention) — a hidden
            // Repeater still instantiates everything otherwise.
            model: root.viewMode !== "list"
                ? Math.max(0, grid.mountedTo - grid.mountedFrom) : 0
            delegate: PurchaseGridCard {
                required property int index
                readonly property int globalIndex: grid.mountedFrom + index
                readonly property var cardData: root.albums[globalIndex] || ({})
                x: (globalIndex % root.columns) * (root.cardWidth + root.cardGap)
                y: Math.floor(globalIndex / root.columns) * (root.cardHeight + root.cardGap)
                width: root.cardWidth
                height: root.cardHeight
                album: cardData
                artSource: root.artOf(cardData)
                onClicked: root.openAlbum(cardData.id || "")
            }
        }
    }

    // --- List -------------------------------------------------------------
    Item {
        id: list
        visible: root.viewMode === "list"
        anchors.fill: parent
        PurchaseListHeader {
            visible: root.viewMode === "list" && root.albums.length > 0
            width: parent.width
            height: root.listHeaderHeight
        }
        Repeater {
            id: listRepeater
            readonly property int mountedFrom: root.windowed
                ? Math.min(root.albums.length, root.bandFirst) : 0
            readonly property int mountedTo: root.windowed
                ? Math.min(root.albums.length, Math.max(mountedFrom, root.bandLast + 1))
                : root.albums.length
            model: root.viewMode === "list"
                ? Math.max(0, mountedTo - mountedFrom) : 0
            delegate: PurchaseListRow {
                required property int index
                readonly property int globalIndex: listRepeater.mountedFrom + index
                readonly property var rowData: root.albums[globalIndex] || ({})
                width: list.width
                y: root.listHeaderHeight + root.listGap
                    + globalIndex * (root.listRowHeight + root.listGap)
                album: rowData
                artSource: root.artOf(rowData)
                rowIndex: globalIndex
                onClicked: root.openAlbum(rowData.id || "")
            }
        }
    }
}
