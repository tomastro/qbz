// PurchaseTracksCollection — a content-sized, viewport-windowed purchase
// track tape. PurchasesView owns the one Flickable; this item preserves the
// complete scroll footprint while mounting only a bounded runway of rows.

import QtQuick
import com.blitzfc.qbz

Item {
    id: root

    property var tracks: []
    property Flickable flick: null
    signal playRequested(string trackId)

    readonly property int rowHeight: 56
    readonly property int rowGap: 2
    readonly property int pitch: rowHeight + rowGap
    readonly property bool windowed: root.flick !== null

    width: parent ? parent.width : 0
    height: root.tracks.length > 0
        ? root.tracks.length * root.rowHeight
            + (root.tracks.length - 1) * root.rowGap
        : 0

    property int bandFirst: 0
    property int bandLast: -1

    function localViewportTop() {
        var p = root.mapToItem(root.flick.contentItem, 0, 0)
        return root.flick.contentY - p.y
    }

    function sampleBand() {
        if (!root.windowed || !root.visible || root.flick.height <= 0)
            return
        var top = root.localViewportTop()
        var h = root.flick.height
        var runwayTop = top - 2 * h
        var runwayBottom = top + 3 * h
        if (root.tracks.length === 0 || runwayBottom <= 0
                || runwayTop >= root.height) {
            root.bandFirst = 0
            root.bandLast = -1
            return
        }
        root.bandFirst = Math.max(0, Math.floor(runwayTop / root.pitch))
        root.bandLast = Math.min(root.tracks.length - 1,
            Math.max(root.bandFirst, Math.ceil(runwayBottom / root.pitch) - 1))
        root.reportArtWindow()
    }

    function ensureBandCoverage() {
        if (!root.windowed || !root.visible || root.flick.height <= 0)
            return
        var top = root.localViewportTop()
        var h = Math.max(1, root.flick.height)
        if (root.tracks.length === 0 || top + 3 * h <= 0
                || top - 2 * h >= root.height)
            return
        var first = Math.max(0, Math.floor(top / root.pitch))
        var last = Math.max(0, Math.ceil((top + h) / root.pitch))
        var runway = Math.max(1, Math.ceil(h / root.pitch))
        if (first < root.bandFirst || last > root.bandLast
                || (first > runway && first - root.bandFirst < runway)
                || (last < root.tracks.length - runway
                    && root.bandLast - last < runway))
            root.sampleBand()
    }

    Connections {
        target: root.flick
        ignoreUnknownSignals: true
        function onContentYChanged() { root.ensureBandCoverage() }
        function onHeightChanged() { root.sampleBand() }
    }
    Component.onCompleted: root.sampleBand()
    onTracksChanged: root.sampleBand()
    onVisibleChanged: root.sampleBand()
    onYChanged: root.sampleBand()

    // Resolve only artwork inside the mounted runway and accept results one
    // key at a time; no full purchase document republish is needed.
    property var artMap: ({})
    readonly property var artAsked: ({ "seen": ({}) })

    function artOf(track) {
        if (!track)
            return ""
        var url = track.artworkUrl || ""
        return (url !== "" && root.artMap[url])
            ? root.artMap[url] : (track.artPath || "")
    }

    function reportArtWindow() {
        if (!root.windowed || root.tracks.length === 0
                || root.bandLast < root.bandFirst)
            return
        var lo = Math.max(0, root.bandFirst)
        var hi = Math.min(root.tracks.length - 1, root.bandLast)
        var pending = []
        var seen = root.artAsked.seen
        for (var i = lo; i <= hi; i++) {
            var track = root.tracks[i] || ({})
            var url = track.artworkUrl || ""
            if (url === "" || (track.artPath || "") !== ""
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

    readonly property int mountedFrom: root.windowed
        ? Math.min(root.tracks.length, root.bandFirst) : 0
    readonly property int mountedTo: root.windowed
        ? Math.min(root.tracks.length, Math.max(root.mountedFrom, root.bandLast + 1))
        : root.tracks.length

    Repeater {
        model: root.visible ? Math.max(0, root.mountedTo - root.mountedFrom) : 0
        delegate: PurchaseTrackRow {
            required property int index
            readonly property int globalIndex: root.mountedFrom + index
            readonly property var rowData: root.tracks[globalIndex] || ({})
            x: 0
            y: globalIndex * root.pitch
            width: root.width
            track: rowData
            artSource: root.artOf(rowData)
            rowIndex: globalIndex
            onPlayRequested: root.playRequested(rowData.id || "")
        }
    }
}
