// Artists tab body (LocalLibraryView.slint:2213) — two-column
// master/detail: the merged, A-Z grouped artist rail on the left (280px,
// with its own AlphaStrip), that artist's albums on the right.
//
// The Slint rail is a plain Flickable with a nested for-per-section; here
// the sections are flattened into one entry array so the rail is a WINDOWING
// ListView — a 4k-artist library mounts a viewport, not 4k rows.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root

    property var view: null

    QbzTheme { id: theme }

    readonly property int headerH: 22
    readonly property int rowH: 62
    readonly property bool nativeActive: QbzLocal.localArtistsNativeActive
    readonly property var nativeModel: QbzLocalArtists
    readonly property var nativeAlbumModel: QbzLocalArtistAlbums
    readonly property int artistTotal: nativeActive
        ? QbzLocal.localArtistsNativeTotal : (view ? view.artists.length : 0)

    // --------------------------- entry model ------------------------------
    property var entries: []
    property var alphaJumps: []
    /// The rail's rows in DISPLAY order — the A-Z grouping reorders them, so
    /// this is NOT `view.artistsVisible` and the window report must index
    /// into it instead. (Reporting entry indices against `artistsVisible`
    /// asked for the wrong avatars by the number of letter headers scrolled
    /// past — up to 27 rows off at the bottom of the rail.)
    property var flat: []
    /// entry index -> index into `flat`, -1 for a letter header.
    property var flatIndex: []
    function parseNativeJumps() {
        try { return JSON.parse(QbzLocal.localArtistsNativeJumpsJson || "[]") }
        catch (e) { return [] }
    }
    function rebuild() {
        if (nativeActive) {
            entries = []
            alphaJumps = parseNativeJumps()
            flat = []
            flatIndex = []
            report()
            return
        }
        var groups = view ? view.artistsGrouped : []
        var out = []
        var jumps = []
        var flatOut = []
        var idx = []
        for (var i = 0; i < groups.length; i++) {
            var g = groups[i]
            if (!g.items || g.items.length === 0) continue
            jumps.push({ "letter": g.letter, "index": out.length })
            out.push({ "t": 0, "label": g.letter })
            idx.push(-1)
            for (var j = 0; j < g.items.length; j++) {
                out.push({ "t": 1, "item": g.items[j] })
                idx.push(flatOut.length)
                flatOut.push(g.items[j])
            }
        }
        entries = out
        alphaJumps = jumps
        flat = flatOut
        flatIndex = idx
        report()
    }
    Component.onCompleted: {
        rebuild()
        reportSoon()
        nativeQueryCoalescer.restart()
        detailQueryCoalescer.restart()
    }
    Component.onDestruction: if (view) view.releaseWindow("artists-rail")
    Connections {
        target: root.view
        function onArtistsGroupedChanged() { root.rebuild() }
        function onArtistsSearchChanged() { nativeQueryCoalescer.restart() }
        function onArtistsSortChanged() { nativeQueryCoalescer.restart() }
        function onArtistsFilterChanged() { nativeQueryCoalescer.restart() }
        function onSelectedArtistChanged() { detailQueryCoalescer.restart() }
        function onArtworkRefresh() { root.reportSoon() }
    }
    Connections {
        target: QbzLocal
        function onLocalArtistsNativeActiveChanged() {
            root.rebuild()
            if (root.nativeActive) detailQueryCoalescer.restart()
        }
        function onLocalArtistsNativeJumpsJsonChanged() {
            if (root.nativeActive) root.alphaJumps = root.parseNativeJumps()
        }
        function onLocalArtistsLoadingChanged() {
            if (!QbzLocal.localArtistsLoading) {
                root.reportSoon()
                detailQueryCoalescer.restart()
            }
        }
    }
    Timer {
        id: nativeQueryCoalescer
        interval: 0
        onTriggered: if (root.view)
            QbzLocal.artistsNativeReset(
                root.view.artistsSearch,
                root.view.artistsSort,
                JSON.stringify(root.view.artistsFilter || ({})))
    }
    Timer {
        id: detailQueryCoalescer
        interval: 0
        onTriggered: root.resetNativeDetail()
    }
    function resetNativeDetail() {
        if (!nativeActive || !view || !artistCollection || artistCollection.cols <= 0) return
        QbzLocal.artistsNativeSelect(view.selectedArtist, artistCollection.cols)
    }

    // Loading = the shape of the master/detail pair that is coming: 60px
    // rail rows with a ROUND 48px avatar on the left, the album grid on the
    // right. Two composites, two animators (see QbzSkeleton COST).
    Row {
        visible: QbzLocal.localArtistsLoading
        anchors.fill: parent
        anchors.topMargin: 12
        spacing: 0
        // The rail's own left inset (24), so the placeholder rows land where
        // the real ones do.
        Item { width: 24; height: 1 }
        QbzSkeleton {
            variant: "rowList"
            width: 256 - 22
            height: parent.height
            rowH: 60
            rowGap: 2
            rowArtSize: 48
            rowArtRound: true
            phase: root.view.skelPhase
        }
        Item { width: 22 + 1; height: 1 }
        QbzSkeleton {
            variant: "cardGrid"
            width: parent.width - 281
            height: parent.height
            cellW: 220
            cellH: 266
            phase: root.view.skelPhase
        }
    }
    // Empty library — icon + line (Slint :2222).
    Column {
        visible: !QbzLocal.localArtistsLoading && root.artistTotal === 0
            && (!root.view || root.view.artistsSearch === "")
        anchors.centerIn: parent
        spacing: 10
        QbzIcon {
            anchors.horizontalCenter: parent.horizontalCenter
            name: "user"
            width: 44
            height: 44
            tintName: "muted"
        }
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: QbzSession.tr("No artists in your local library yet.", QbzSession.trRev)
            color: theme.textMuted
            font.pixelSize: theme.fontBody
        }
    }

    Row {
        anchors.fill: parent
        visible: !QbzLocal.localArtistsLoading
            && (root.artistTotal > 0 || (root.view && root.view.artistsSearch !== ""))
        spacing: 0

        // ---------------------- master rail ------------------------------
        Item {
            width: 280
            height: parent.height
            clip: true

            Column {
                anchors.fill: parent
                anchors.leftMargin: 24
                anchors.topMargin: 12
                spacing: 8

                Text {
                    text: (root.nativeActive ? QbzLocal.localArtistsNativeTotal
                                             : root.view.artistsVisibleCount) + " "
                        + QbzSession.tr("artists", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: theme.fontLegal
                    font.weight: theme.weightSemibold
                }
                Text {
                    visible: (root.nativeActive ? QbzLocal.localArtistsNativeTotal
                                                : root.view.artistsVisibleCount) === 0
                    text: QbzSession.tr("No artists match your search.", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: theme.fontLegal
                }
                Item {
                    visible: (root.nativeActive ? QbzLocal.localArtistsNativeTotal
                                                : root.view.artistsVisibleCount) > 0
                    width: parent.width
                    height: parent.height - 30

                    // Back/forward scroll memory (controls/ScrollMemory.qml).
                    ScrollMemory { target: rail; scope: "local:artists" }
                    ListView {
                        id: rail
                        anchors.fill: parent
                        anchors.rightMargin: 22
                        clip: true
                        cacheBuffer: 62 * 8
                        boundsBehavior: Flickable.StopAtBounds
                        model: root.nativeActive ? root.nativeModel : root.entries
                        onContentYChanged: root.report()
                        onModelChanged: root.report()
                        onHeightChanged: root.report()
                        onVisibleChanged: {
                            if (visible) root.reportSoon()
                            else if (root.view) root.view.releaseWindow("artists-rail")
                        }

                        delegate: Loader {
                            required property var modelData
                            width: rail.width
                            height: modelData.t === 0 ? root.headerH : root.rowH
                            sourceComponent: modelData.loading ? loadingComp
                                : modelData.t === 0 ? letterComp : artistComp
                            onModelDataChanged: root.reportSoon()

                            Component {
                                id: loadingComp
                                QbzSkeleton {
                                    variant: "rowList"
                                    rowH: 60
                                    rowGap: 2
                                    rowArtSize: 48
                                    rowArtRound: true
                                    phase: root.view ? root.view.skelPhase : false
                                }
                            }

                            Component {
                                id: letterComp
                                Item {
                                    Text {
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: modelData.label
                                        color: theme.textSecondary
                                        font.pixelSize: theme.fontLegal
                                        font.weight: theme.weightSemibold
                                    }
                                }
                            }
                            Component {
                                id: artistComp
                                LocalArtistRow {
                                    width: rail.width
                                    view: root.view
                                    item: modelData.item
                                    artSource: modelData.item.artPath
                                        || root.view.artMap[modelData.item.artKey] || ""
                                    selected: root.view.selectedArtist === modelData.item.name
                                    onPicked: function (name) { root.view.selectArtist(name) }
                                }
                            }
                        }
                    }
                    QbzAlphaStrip {
                        visible: root.artistTotal > 0
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.bottom: parent.bottom
                        jumps: root.alphaJumps
                        completeAlphabet: true
                        onJump: function (ordinal, index) {
                            rail.positionViewAtIndex(index, ListView.Beginning)
                        }
                    }
                }
            }
        }

        Rectangle {
            width: 1
            height: parent.height
            color: theme.borderSubtle
        }

        // ---------------------- detail pane ------------------------------
        Item {
            width: parent.width - 281
            height: parent.height
            clip: true

            Column {
                visible: root.view.selectedArtist === ""
                anchors.centerIn: parent
                spacing: 10
                QbzIcon {
                    anchors.horizontalCenter: parent.horizontalCenter
                    name: "user"
                    width: 44
                    height: 44
                    tintName: "muted"
                }
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: QbzSession.tr("Select an artist to view their albums", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: theme.fontBody
                }
            }

            Column {
                anchors.fill: parent
                anchors.leftMargin: 28
                anchors.rightMargin: 8
                anchors.topMargin: 14
                spacing: 12
                visible: root.view.selectedArtist !== ""

                Column {
                    width: parent.width
                    spacing: 2
                    Text {
                        width: parent.width
                        text: root.view.selectedArtist
                        color: theme.textPrimary
                        font.pixelSize: theme.fontSection
                        font.weight: theme.weightBold
                        elide: Text.ElideRight
                    }
                    Text {
                        text: (root.nativeActive
                               ? QbzLocal.localArtistAlbumsNativeTotal
                               : root.view.artistAlbums.length) + " "
                            + QbzSession.tr("albums", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: theme.fontLegal
                    }
                }
                Text {
                    visible: !QbzLocal.localArtistAlbumsLoading
                        && (root.nativeActive
                            ? QbzLocal.localArtistAlbumsNativeTotal === 0
                            : root.view.artistAlbums.length === 0)
                    width: parent.width
                    topPadding: 24
                    text: QbzSession.tr("No albums found", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: theme.fontBody
                    horizontalAlignment: Text.AlignHCenter
                }
                QbzSkeleton {
                    visible: root.nativeActive && QbzLocal.localArtistAlbumsLoading
                    width: parent.width
                    height: parent.height - 60
                    variant: "cardGrid"
                    cellW: 220
                    cellH: 266
                    phase: root.view.skelPhase
                }
                LocalAlbumCollection {
                    id: artistCollection
                    visible: !QbzLocal.localArtistAlbumsLoading
                        && (root.nativeActive
                            ? QbzLocal.localArtistAlbumsNativeTotal > 0
                            : root.view.artistAlbums.length > 0)
                    width: parent.width
                    height: parent.height - 60
                    view: root.view
                    surface: "artist-albums"
                    rows: root.nativeActive ? [] : root.view.artistAlbums
                    grouped: false
                    viewMode: "grid"
                    showSource: true
                    nativeActive: root.nativeActive
                    nativeModel: root.nativeAlbumModel
                    nativeJumpsJson: "[]"
                    onColsChanged: detailQueryCoalescer.restart()
                    onOpenRequested: function (id) { root.view.openAlbum(id) }
                    onPlayRequested: function (id) {
                        QbzLocal.playAlbumFiltered(id, JSON.stringify(root.view.artistsFilter || {}), false)
                    }
                    onEnqueueRequested: function (id, m) {
                        QbzLocal.enqueueAlbumFiltered(id, JSON.stringify(root.view.artistsFilter || {}), m)
                    }
                }
            }
        }
    }

    // Artist avatars ride the same id-keyed artwork window as every other
    // cover; the rail reports its mounted band.
    //
    // TRIGGERS: the rail used to report on `contentY` and the model swap
    // only, and both of those fire while the pane is still behind
    // `localArtistsLoading` — the report was made against a ListView that had
    // not laid out, and nothing re-fired once the rows appeared. It now also
    // reports on mount, on the rebuild, when the pane becomes visible and
    // when the viewport resizes.
    function report() {
        if (!view || !rail) return
        var count = root.nativeActive ? root.nativeModel.totalCount : root.entries.length
        if (!rail.visible || count === 0) {
            view.releaseWindow("artists-rail")
            return
        }
        var first = rail.indexAt(4, rail.contentY + 1)
        var last = rail.indexAt(4, rail.contentY + Math.max(1, rail.height) - 1)
        if (first < 0) first = 0
        if (last < 0) last = Math.min(count - 1, first + 12)
        if (root.nativeActive) {
            var resident = []
            for (var entryIndex = first; entryIndex <= last; entryIndex++) {
                var entry = root.nativeModel.rowAt(entryIndex)
                if (!entry || entry.loading || entry.t !== 1 || !entry.item) continue
                resident.push(entry.item)
            }
            if (resident.length > 0)
                view.queueWindowReport(resident, 0, resident.length - 1, "artists-rail")
            else
                view.releaseWindow("artists-rail")
            return
        }
        // Entry band -> rail-row band (letter headers do not carry a cover).
        var lo = -1
        var hi = -1
        for (var i = first; i <= last; i++) {
            var n = root.flatIndex[i]
            if (n === undefined || n < 0) continue
            if (lo < 0) lo = n
            hi = n
        }
        if (lo < 0) { lo = 0; hi = Math.min(root.flat.length - 1, 12) }
        view.queueWindowReport(root.flat, Math.max(0, lo - 4), hi + 4, "artists-rail")
    }

    /// Report now, then again once the ListView has laid out — `indexAt`
    /// answers -1 before that, which is exactly the state of a pane that has
    /// just been shown.
    Timer {
        id: reportSettle
        interval: 50
        repeat: false
        onTriggered: root.report()
    }
    function reportSoon() {
        report()
        reportSettle.restart()
    }
}
