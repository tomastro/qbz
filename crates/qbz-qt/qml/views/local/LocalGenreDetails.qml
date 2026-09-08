// Expanded-album list for the Genres browser. Only visible albums request
// tracks; every album starts expanded, matching the classic column browser
// while keeping the delegate count proportional to the viewport.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root

    property var view: null
    property var albums: []
    // album id -> {tracks, discs, versions, versionIndex}. Only the selected
    // version carries heavy rows; the picker metadata is compact.
    property var detailCache: ({})
    // album id -> mediaFilterJson used to build the cached document. Keeping
    // this separate lets a filter change retain the mounted track geometry
    // while the replacement picker document is derived from Rust's raw cache.
    property var detailFilters: ({})
    property var pending: ({})
    property var cacheOrder: []
    // A cold viewport can contain eight compact album headers. Starting one
    // cross-source detail query for every header at once contends on the same
    // SQLite caches, then most results move off-screen as the first expanded
    // album acquires its real height. Keep a small foreground window instead:
    // the first result reshapes the ListView before we decide what is useful
    // to fetch next.
    readonly property int maxConcurrentDetailRequests: 2
    function stableFilterJson(value) {
        var source = value || ({})
        var keys = Object.keys(source).sort()
        var normalized = {}
        for (var i = 0; i < keys.length; i++)
            if (source[keys[i]] === true) normalized[keys[i]] = true
        return JSON.stringify(normalized)
    }
    readonly property string mediaFilterJson:
        stableFilterJson(view ? (view.genresFilter || {}) : {})
    // Absence means expanded: albums are open by default, while a sparse map
    // remembers only the rows the user explicitly folded.
    property var collapsed: ({})
    // "album id\ndisc number" -> true. Discs, like albums, start open and
    // only explicit folds occupy this sparse map.
    property var collapsedDiscs: ({})
    readonly property bool initialDetailLoading:
        Object.keys(detailCache).length === 0
        && Object.keys(pending).length > 0

    QbzTheme { id: theme }

    // Model/filter changes can invalidate this Loader before a deferred QML
    // closure runs. Object-owned timers are cancelled with the view, and the
    // short report debounce also coalesces the layout churn from a large box
    // set into one visible-window update.
    Timer {
        id: ensureVisibleTimer
        interval: 16
        repeat: false
        onTriggered: root.ensureVisible()
    }
    Timer {
        id: reportTimer
        interval: 16
        repeat: false
        onTriggered: root.report()
    }

    function scheduleEnsureVisible() {
        ensureVisibleTimer.restart()
    }
    function scheduleReport() {
        reportTimer.restart()
    }

    function ensure(albumId) {
        if (!albumId) return false
        if (detailCache[albumId] !== undefined
                && detailFilters[albumId] === mediaFilterJson) return false
        // A quick source A -> A+B -> A sequence may leave an older worker in
        // flight. Dedupe only the exact funnel; Rust generation-checks and
        // suppresses any older result before it reaches this signal.
        if (pending[albumId] === mediaFilterJson) return false
        var next = Object.assign({}, pending)
        next[albumId] = mediaFilterJson
        pending = next
        QbzLocal.genreAlbumTracks(albumId, mediaFilterJson)
        return true
    }

    function currentPendingCount() {
        var count = 0
        var keys = Object.keys(pending)
        for (var i = 0; i < keys.length; i++)
            if (pending[keys[i]] === mediaFilterJson) count++
        return count
    }

    function ensureVisible() {
        if (albums.length === 0 || !list.visible) return
        var first = list.indexAt(2, list.contentY + 1)
        var last = list.indexAt(2, list.contentY + list.height - 1)
        if (first < 0) first = 0
        if (last < 0) last = Math.min(albums.length - 1, first + 4)
        var available = maxConcurrentDetailRequests - currentPendingCount()
        if (available <= 0) return
        for (var i = first; i <= last && i < albums.length; i++) {
            if (ensure(albums[i].id) && --available === 0) break
        }
    }

    onMediaFilterJsonChanged: {
        // Do not collapse a loaded 247-track box merely because another
        // source was enabled. Its visible delegate asks for the new funnel
        // below; the old rows remain stable until the picker/selection update
        // arrives, avoiding a full destroy/recreate cycle and scroll jump.
        // Old-generation workers are suppressed in Rust and publish no signal;
        // do not let their QML bookkeeping occupy the new funnel's two slots.
        pending = ({})
        scheduleEnsureVisible()
    }

    function toggle(albumId) {
        var next = Object.assign({}, collapsed)
        if (next[albumId]) delete next[albumId]
        else next[albumId] = true
        collapsed = next
    }

    function discKey(albumId, disc) { return albumId + "\n" + disc }
    function discCollapsed(albumId, disc) {
        return collapsedDiscs[discKey(albumId, disc)] === true
    }
    function toggleDisc(albumId, disc) {
        var key = discKey(albumId, disc)
        var next = Object.assign({}, collapsedDiscs)
        if (next[key]) delete next[key]
        else next[key] = true
        collapsedDiscs = next
    }

    Connections {
        target: QbzLocal
        function onLocalGenreAlbumReady(albumId, json, filterJson) {
            // A queued cache hit can cross a source-chip change even though
            // the worker paths are generation-checked. Never label that old
            // physical-version document as belonging to the current funnel.
            if (filterJson !== root.mediaFilterJson) return
            var doc = { "artKey": "", "tracks": [], "discs": [],
                        "versions": [], "versionIndex": 0 }
            try {
                var parsed = JSON.parse(json || "{}")
                // Tolerate an in-flight publish from an older process during
                // development; release builds always send the object shape.
                doc = Array.isArray(parsed)
                    ? { "artKey": "", "tracks": parsed, "discs": [],
                        "versions": [], "versionIndex": 0 }
                    : {
                        "artKey": parsed.artKey || "",
                        "tracks": parsed.tracks || [],
                        "discs": parsed.discs || [],
                        "versions": parsed.versions || [],
                        "versionIndex": Number(parsed.versionIndex || 0)
                    }
            }
            catch (e) { console.warn("[qbz-qt] genres: bad album rows — " + e) }
            var cache = Object.assign({}, root.detailCache)
            var filters = Object.assign({}, root.detailFilters)
            var order = root.cacheOrder.slice()
            var existing = order.indexOf(albumId)
            if (existing >= 0) order.splice(existing, 1)
            order.push(albumId)
            while (order.length > 32) {
                var evicted = order.shift()
                delete cache[evicted]
                delete filters[evicted]
            }
            // If the selected physical version itself did not change, retain
            // the exact track/disc arrays. The picker can gain a second source
            // without resetting hundreds of nested delegates that are already
            // correct. `key` fingerprints the filtered physical row set, so a
            // format/quality change with the same directory cannot reuse it.
            var previous = cache[albumId]
            if (previous) {
                var previousVersions = previous.versions || []
                var previousIndex = Number(previous.versionIndex || 0)
                var previousVersion = previousVersions[previousIndex] || ({})
                var nextVersion = doc.versions[doc.versionIndex] || ({})
                if ((previousVersion.key || "") !== ""
                        && previousVersion.key === (nextVersion.key || "")) {
                    doc.artKey = previous.artKey || doc.artKey
                    doc.tracks = previous.tracks || doc.tracks
                    doc.discs = previous.discs || doc.discs
                }
            }
            cache[albumId] = doc
            filters[albumId] = root.mediaFilterJson
            root.cacheOrder = order
            root.detailCache = cache
            root.detailFilters = filters
            var live = Object.assign({}, root.pending)
            if (live[albumId] === root.mediaFilterJson) delete live[albumId]
            root.pending = live
            // The selected version can name a different artwork key. Report
            // after the cache publish so the artwork window requests it even
            // when the outer album list itself did not change.
            root.scheduleReport()
            root.scheduleEnsureVisible()
        }
    }

    function report() {
        if (!view || albums.length === 0 || !list.visible) return
        var first = list.indexAt(2, list.contentY + 1)
        var last = list.indexAt(2, list.contentY + list.height - 1)
        if (first < 0) first = 0
        if (last < 0) last = Math.min(albums.length - 1, first + 4)
        // Only build the visible slice. Mapping all 1,800 albums on every
        // scroll tick would undo the virtualization this view relies on.
        var artworkRows = []
        for (var i = first; i <= last && i < albums.length; i++) {
            var album = albums[i]
            var detail = detailCache[album.id]
            artworkRows.push({ "artKey": detail && detail.artKey
                ? detail.artKey : (album.artKey || "") })
            var discs = detail ? (detail.discs || []) : []
            for (var d = 0; d < discs.length; d++)
                if (discs[d].artKey)
                    artworkRows.push({ "artKey": discs[d].artKey })
        }
        if (artworkRows.length > 0)
            view.queueWindowReport(artworkRows, 0, artworkRows.length - 1,
                                   "genres-details")
    }

    ListView {
        id: list
        anchors.fill: parent
        anchors.rightMargin: 14
        clip: true
        reuseItems: true
        // One expanded album may be thousands of pixels tall. A 600px outer
        // cache eagerly queried whole albums above and below the viewport;
        // the row-level window below supplies the useful look-ahead instead.
        cacheBuffer: 240
        boundsBehavior: Flickable.StopAtBounds
        model: root.albums
        onContentYChanged: {
            root.scheduleReport()
            root.scheduleEnsureVisible()
        }
        onHeightChanged: {
            root.scheduleReport()
            root.scheduleEnsureVisible()
        }
        onModelChanged: {
            root.scheduleReport()
            root.scheduleEnsureVisible()
        }

        delegate: Item {
            id: albumBlock
            required property var modelData
            required property int index
            readonly property var detail: root.detailCache[modelData.id]
                || ({ "artKey": "", "tracks": [], "discs": [],
                      "versions": [], "versionIndex": 0 })
            readonly property var tracks: detail.tracks || []
            readonly property var discs: detail.discs || []
            readonly property var versions: detail.versions || []
            readonly property int versionIndex: Number(detail.versionIndex || 0)
            readonly property bool loaded: root.detailCache[modelData.id] !== undefined
            readonly property bool expanded: root.collapsed[modelData.id] !== true
            readonly property int discHeaders: loaded ? discs.length : 0
            function startsDisc(rowIndex) {
                if (albumBlock.discHeaders === 0) return false
                if (rowIndex === 0) return true
                return Number(tracks[rowIndex - 1].disc || 1)
                    !== Number(tracks[rowIndex].disc || 1)
            }
            function discInfo(number) {
                for (var i = 0; i < discs.length; i++)
                    if (Number(discs[i].disc || 0) === number) return discs[i]
                return null
            }
            function discIsCollapsed(number) {
                return root.discCollapsed(modelData.id, number)
            }
            function discArt(info) {
                if (!info) return ""
                return (root.view && info.artKey
                        && root.view.artMap[info.artKey])
                    || info.cover || ""
            }
            readonly property bool discArtDistinct: {
                if (discs.length < 2) return false
                var seen = {}
                for (var i = 0; i < discs.length; i++) {
                    var path = discArt(discs[i])
                    if (path === "" || seen[path]) return false
                    seen[path] = true
                }
                return true
            }
            // The server-backed track count is useful metadata, not geometry.
            // Reserving `trackCount * rowHeight` before the detail arrives made
            // a cold 13-track album look like 650 px of empty content, and a
            // large box set could consume several whole screens with only a
            // spinner at its top. Keep cold rows compact; once the visible
            // album resolves, its real track rows become its real height.
            readonly property int visibleTracks: {
                if (!loaded) return 0
                var count = 0
                for (var i = 0; i < tracks.length; i++) {
                    if (!discIsCollapsed(Number(tracks[i].disc || 1))) count++
                }
                return count
            }
            readonly property int headerH: 72
            readonly property int discHeaderH: 46
            readonly property int loadingBodyH: 50
            readonly property real bodyHeight: loaded
                ? visibleTracks * 50 + discHeaders * discHeaderH : 0
            // Keep the album itself as one variable-height outer delegate, but
            // give its tracks a real viewport. The former nested Repeater
            // constructed one wrapper and two Loaders for all 247 tracks in a
            // box set before the first frame could paint. Moving this small
            // ListView through the visible slice preserves the exact outer
            // geometry while Qt only instantiates the rows on screen.
            readonly property real trackViewportStart: Math.max(
                0, list.contentY - y - headerH - 300)
            readonly property real trackViewportEnd: Math.min(
                bodyHeight, list.contentY + list.height - y - headerH + 300)
            readonly property real trackViewportHeight: Math.max(
                0, trackViewportEnd - trackViewportStart)
            function ensureCurrent() {
                // Delegate creation is only a visibility hint. Let the view's
                // bounded scheduler choose foreground albums after ListView
                // has settled their current geometry.
                root.scheduleEnsureVisible()
            }
            width: list.width
            height: headerH + (expanded
                ? (loaded
                    ? bodyHeight + 12
                    : loadingBodyH)
                : 0)

            // The Rust bridge always publishes cache hits on the next Qt
            // event-loop turn, so requesting synchronously is safe and leaves
            // no delegate-owned callback behind when ListView recycles it.
            Component.onCompleted: ensureCurrent()
            onModelDataChanged: ensureCurrent()

            LocalAlbumRow {
                x: 4
                width: parent.width - 8
                view: root.view
                item: albumBlock.modelData
                artSource: root.view
                    ? ((albumBlock.detail.artKey
                        && root.view.artMap[albumBlock.detail.artKey])
                       || albumBlock.modelData.artPath
                       || root.view.artMap[albumBlock.modelData.artKey] || "") : ""
                isFavorite: root.view
                    ? root.view.albumFavorite(albumBlock.modelData)
                    : albumBlock.modelData.isFavorite === true
                showSource: true
                expandable: true
                expanded: albumBlock.expanded
                detailsMode: true
                versions: albumBlock.versions.length > 0
                    ? albumBlock.versions
                    : [{
                        "version": "",
                        "trackCount": albumBlock.modelData.trackCount || 0,
                        "quality": albumBlock.modelData.qualityDetail || "",
                        "source": albumBlock.modelData.sourceRaw
                            || albumBlock.modelData.source || "local"
                    }]
                versionIndex: albumBlock.versionIndex
                onOpened: root.view.openAlbum(albumBlock.modelData.id)
                onPlayRequested: QbzLocal.genreAlbumAction(
                    albumBlock.modelData.id, "play", "")
                onShuffleRequested: QbzLocal.genreAlbumAction(
                    albumBlock.modelData.id, "shuffle", "")
                onEnqueueRequested: function(mode) {
                    QbzLocal.genreAlbumAction(albumBlock.modelData.id, mode, "")
                }
                onFavoriteRequested: if (root.view) {
                    root.view.toggleAlbumFavorite(albumBlock.modelData, artSource)
                }
                onToggleExpanded: root.toggle(albumBlock.modelData.id)
                onVersionPicked: function(index) {
                    QbzLocal.genreAlbumSelectVersion(albumBlock.modelData.id, index)
                }
            }

            QbzSpinner {
                visible: albumBlock.expanded && !albumBlock.loaded
                anchors.horizontalCenter: parent.horizontalCenter
                y: albumBlock.headerH
                    + (albumBlock.loadingBodyH - height) / 2
                size: 18
            }

            ListView {
                id: trackList
                visible: albumBlock.expanded && albumBlock.loaded
                    && albumBlock.trackViewportHeight > 0
                x: 4
                y: albumBlock.headerH + albumBlock.trackViewportStart
                width: parent.width - 8
                height: albumBlock.trackViewportHeight
                model: albumBlock.tracks
                contentY: albumBlock.trackViewportStart
                interactive: false
                clip: true
                reuseItems: true
                cacheBuffer: 300
                boundsBehavior: Flickable.StopAtBounds
                delegate: Item {
                    id: trackBlock
                    required property var modelData
                    required property int index
                    width: parent ? parent.width : 0
                    readonly property bool showDisc: albumBlock.startsDisc(index)
                    readonly property int discNumber: Number(modelData.disc || 1)
                    readonly property bool discFolded:
                        albumBlock.discIsCollapsed(discNumber)
                    height: (showDisc ? albumBlock.discHeaderH : 0)
                        + (discFolded ? 0 : 50)

                    // A divider exists only on disc boundaries and only
                    // while this virtualized delegate is near the viewport.
                    Loader {
                            width: parent.width
                            height: trackBlock.showDisc ? albumBlock.discHeaderH : 0
                            active: trackBlock.showDisc
                            sourceComponent: Item {
                            id: discHeader
                            width: parent.width
                            height: albumBlock.discHeaderH
                            readonly property var info:
                                albumBlock.discInfo(trackBlock.discNumber)

                            QbzIconButton {
                                x: 8
                                anchors.verticalCenter: parent.verticalCenter
                                btnSize: 26
                                iconSize: 12
                                name: trackBlock.discFolded ? "plus" : "minus"
                                onClicked: root.toggleDisc(
                                    albumBlock.modelData.id, trackBlock.discNumber)
                            }
                            RoundedImage {
                                id: discThumb
                                x: 44
                                width: 30
                                height: 30
                                radius: theme.radiusSm
                                anchors.verticalCenter: parent.verticalCenter
                                visible: albumBlock.discArtDistinct && source !== ""
                                source: albumBlock.discArt(parent.info)
                            }
                            Text {
                                x: discThumb.visible ? 84 : 44
                                width: Math.max(0, discActions.x - x - 12)
                                anchors.verticalCenter: parent.verticalCenter
                                text: {
                                    var base = QbzSession.tr("Disc", QbzSession.trRev)
                                        + " " + trackBlock.discNumber
                                    return parent.info && parent.info.title
                                        ? base + " — " + parent.info.title : base
                                }
                                color: theme.textSecondary
                                font.pixelSize: theme.fontLegal
                                font.weight: theme.weightSemibold
                                elide: Text.ElideRight
                            }

                            // Smaller than AlbumView's 32/44px header actions:
                            // these float right, before the trailing table
                            // columns, and do not become a centred hero rail.
                            Row {
                                id: discActions
                                x: parent.width - 176
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 6
                                QbzIconButton {
                                    btnSize: 26
                                    iconSize: 13
                                    name: "play-fill"
                                    onClicked: QbzLocal.genreAlbumDiscAction(
                                        albumBlock.modelData.id,
                                        trackBlock.discNumber,
                                        "play")
                                }
                                QbzIconButton {
                                    btnSize: 26
                                    iconSize: 13
                                    name: "shuffle"
                                    onClicked: QbzLocal.genreAlbumDiscAction(
                                        albumBlock.modelData.id,
                                        trackBlock.discNumber,
                                        "shuffle")
                                }
                            }
                            QbzIconButton {
                                id: discMenuButton
                                x: parent.width - 70
                                anchors.verticalCenter: parent.verticalCenter
                                btnSize: 32
                                name: "ellipsis"
                                iconSize: 16
                                onClicked: {
                                    discMenuLoader.active = true
                                    discMenuLoader.item.openBelowRight(discMenuButton)
                                }
                            }
                            // A popup with five delegated rows is useful only
                            // after the click. Keep it out of every box-set
                            // divider's steady-state scene tree.
                            Loader {
                                id: discMenuLoader
                                active: false
                                sourceComponent: CardMenu {
                                    menuWidth: 200
                                    entries: [
                                        { "label": QbzSession.tr("Play", QbzSession.trRev), "icon": "play-fill", "action": "play" },
                                        { "label": QbzSession.tr("Shuffle", QbzSession.trRev), "icon": "shuffle", "action": "shuffle" },
                                        { "label": QbzSession.tr("Play next", QbzSession.trRev), "icon": "list-start", "action": "next" },
                                        { "label": QbzSession.tr("Play later", QbzSession.trRev), "icon": "list-plus", "action": "later" },
                                        { "label": QbzSession.tr("Add to queue", QbzSession.trRev), "icon": "list-end", "action": "queue" }
                                    ]
                                    onPicked: function (action) {
                                        QbzLocal.genreAlbumDiscAction(
                                            albumBlock.modelData.id,
                                            trackBlock.discNumber,
                                            action)
                                    }
                                }
                            }
                            Rectangle {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.bottom: parent.bottom
                                height: 1
                                color: theme.borderSubtle
                            }
                            }
                    }

                    Loader {
                            y: trackBlock.showDisc ? albumBlock.discHeaderH : 0
                            width: parent.width
                            height: trackBlock.discFolded ? 0 : 50
                            active: !trackBlock.discFolded
                            sourceComponent: LocalTrackRow {
                                view: root.view
                                item: trackBlock.modelData
                                number: trackBlock.modelData.number > 0
                                    ? trackBlock.modelData.number : trackBlock.index + 1
                                showAlbum: false
                                showArtwork: false
                                zebra: true
                                onPlayRequested: QbzLocal.genreAlbumAction(
                                    albumBlock.modelData.id, "play",
                                    trackBlock.modelData.id)
                                onEnqueueRequested: function(mode) {
                                    QbzLocal.enqueue("track", trackBlock.modelData.id, mode)
                                }
                            }
                    }
                }
            }
        }
    }

    // The per-album spinner can sit below a restored scroll offset while its
    // first document is cold. This viewport-level affordance stays visible
    // until any detail document lands, then never flashes for ordinary
    // look-ahead requests during scrolling.
    Rectangle {
        visible: root.initialDetailLoading
        z: 3
        anchors.centerIn: parent
        width: 44
        height: 44
        radius: 8
        color: theme.ambientOn ? theme.surfaceElevatedA50 : theme.surfaceElevated
        border.width: 1
        border.color: theme.ambientOn ? theme.frostBorder : theme.borderSubtle
        QbzSpinner {
            anchors.centerIn: parent
            size: 20
        }
    }

    ScrollMemory { target: list; scope: "local:genres" }
    QbzScrollBar {
        target: list
        anchors.right: parent.right
        anchors.rightMargin: 4
        anchors.top: parent.top
        anchors.bottom: parent.bottom
    }

    Component.onCompleted: scheduleReport()
    Component.onDestruction: if (view) view.releaseWindow("genres-details")
}
