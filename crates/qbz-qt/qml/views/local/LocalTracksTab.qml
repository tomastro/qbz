// Tracks tab body (LocalLibraryView.slint:1317). The compatibility reader is
// server-paginated JSON; Phase E binds the same delegates to a native
// QAbstractListModel with exact rowCount, 250-row keyset pages and an eight-page
// LRU. Missing data is a placeholder and only emits an async page request.
//
// Grouping (off / album / artist / name) inserts 32px header rows, so the
// model is an entry array — { t: 0, label } | { t: 1, row, n } — exactly as
// the collection does it, keeping ONE windowing ListView with variable row
// heights instead of a Repeater per group.

import QtQuick
import QtQml.Models
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root

    /// The tab's own left/right breathing room. Named because the scrollbar
    /// has to CANCEL it (see the QbzScrollBar at the bottom of the list) —
    /// with the number written twice, moving one would silently misalign it.
    readonly property int sideInset: 32

    property var view: null

    QbzTheme { id: theme }

    readonly property int rowH: 50
    readonly property int headerH: 32
    readonly property bool nativeModelActive: QbzLocal.localTracksNativeActive
    // Synchronous QML-side latch. QbzLocal queues its loadingMore mutation
    // onto the Qt event loop, so a kinetic-scroll burst can emit more than one
    // contentY change before that property turns true. All of those changes
    // describe the same page miss and must collapse into one bridge call.
    property bool pageRequestPending: false
    // Programmatic viewport restoration emits the same Flickable signals as
    // user input. Keep those signals from recursively requesting another page.
    property bool restoringPageAnchor: false
    // Captured at REQUEST time, while the old model is unquestionably still
    // mounted. Bridge-property notifications may otherwise clear the loading
    // latch before `tracksVisible` re-evaluates and make rebuild miss the only
    // trustworthy viewport.
    property var pendingPageAnchor: null
    // Invalidates a deferred viewport restore when another document/model
    // replacement wins before Qt's next event-loop turn.
    property int entriesEpoch: 0

    // Keep one QAbstractItemModel object mounted for the entire compatibility
    // session. Binding a fresh JS array to ListView.model for every accumulated
    // 500-row document invokes setModel() and resets the view before any anchor
    // repair can run. A ListModel append is a real rowsInserted notification:
    // Qt keeps contentY and the visible delegate untouched.
    ListModel {
        id: legacyEntriesModel
    }

    function sameEntryIdentity(a, b) {
        if (!a || !b || a.t !== b.t)
            return false
        if (a.t === 0)
            return String(a.label) === String(b.label)
        return a.row && b.row && String(a.row.id) === String(b.row.id)
    }

    function canAppendLegacyEntries(nextEntries) {
        if (legacyEntriesModel.count !== root.entries.length
            || nextEntries.length < root.entries.length)
            return false
        for (var i = 0; i < root.entries.length; i++) {
            if (!root.sameEntryIdentity(root.entries[i], nextEntries[i]))
                return false
        }
        return true
    }

    function publishLegacyEntries(nextEntries, anchor, epoch) {
        var previousCount = root.entries.length
        if (root.canAppendLegacyEntries(nextEntries)) {
            // Every paged query, grouped or not, has an immutable prefix.
            // Append only the new tail, preserving the viewport by construction.
            // Hold the request guard through the rowsInserted polish so a
            // kinetic-scroll signal cannot request a second page recursively.
            if (root.pageRequestPending)
                root.restoringPageAnchor = true
            for (var i = previousCount; i < nextEntries.length; i++)
                legacyEntriesModel.append({"modelData": nextEntries[i]})
            root.entries = nextEntries
            if (root.pageRequestPending)
                pageAnchorSettle.restart()
            return
        }

        // A fresh search/sort/group query can replace the whole document.
        // Rebuild rows inside the SAME ListModel object and then restore the
        // semantic anchor. This avoids QQuickItemView::setModel() even when a
        // full reconciliation is necessary.
        legacyEntriesModel.clear()
        for (var j = 0; j < nextEntries.length; j++)
            legacyEntriesModel.append({"modelData": nextEntries[j]})
        root.entries = nextEntries
        root.restorePageAnchor(anchor, nextEntries, epoch)
    }

    function requestNextPage(force) {
        if (root.nativeModelActive
            || root.pageRequestPending
            || root.restoringPageAnchor
            || !QbzLocal.localTracksHasMore
            || QbzLocal.localTracksLoadingMore
            || QbzLocal.localTracksLoading)
            return

        // ListView coordinates are relative to originY, which is allowed to
        // become non-zero when a variable-height model is replaced. The old
        // `contentY + height` check silently stopped matching the visual end
        // after repeated 500-row JSON republishes (the owner reached exactly
        // 1,500 rows). QbzScrollBar already follows this same documented
        // coordinate contract.
        var scrollY = Math.max(0, list.contentY - list.originY)
        var nearEnd = list.atYEnd
            || scrollY + list.height >= list.contentHeight - 600
        if (!force && !nearEnd)
            return

        root.pendingPageAnchor = root.capturePageAnchor()
        root.pageRequestPending = true
        pageRequestRelease.stop()
        QbzLocal.tracksLoadMore()
    }

    function capturePageAnchor() {
        if (root.nativeModelActive || root.entries.length === 0 || !list.visible)
            return null

        // Preserve identity, not merely pixels. Group modes globally reorder
        // the accumulated rows when a page arrives, so the same contentY can
        // point at a different track after the model swap.
        var index = list.indexAt(4, list.contentY + 1)
        if (index < 0)
            index = Math.min(root.entries.length - 1,
                             Math.max(0, list.count - 1))
        var i = index
        while (i < root.entries.length && root.entries[i].t !== 1)
            i++
        if (i >= root.entries.length) {
            i = index - 1
            while (i >= 0 && root.entries[i].t !== 1)
                i--
        }
        if (i < 0 || !root.entries[i].row)
            return null

        var cell = list.itemAtIndex(i)
        return {
            "id": String(root.entries[i].row.id),
            // Position inside the viewport, including a partially clipped
            // first row or a group header immediately above it. A ListView
            // delegate's y is in contentItem coordinates, so subtracting the
            // viewport's contentY is the visual coordinate. mapToItem(list)
            // preserves the content coordinate here and would add thousands
            // of rows back into the restore calculation.
            "screenY": cell ? cell.y - list.contentY : 0
        }
    }

    function restorePageAnchor(anchor, nextEntries, epoch) {
        if (!anchor)
            return
        var target = -1
        for (var i = 0; i < nextEntries.length; i++) {
            var entry = nextEntries[i]
            if (entry.t === 1 && entry.row
                && String(entry.row.id) === anchor.id) {
                target = i
                break
            }
        }
        if (target < 0)
            return

        // A non-page document replacement can still clear/refill the stable
        // model. Let those notifications settle, then retain the exact
        // semantic track and partial-row offset on the next event-loop turn.
        Qt.callLater(function () {
            if (epoch !== root.entriesEpoch || root.nativeModelActive)
                return
            root.restoringPageAnchor = true
            list.forceLayout()
            list.positionViewAtIndex(target, ListView.Beginning)
            list.forceLayout()
            var cell = list.itemAtIndex(target)
            var currentScreenY = cell ? cell.y - list.contentY : 0
            var wanted = list.contentY + currentScreenY - anchor.screenY
            var minY = list.originY
            var maxY = minY + Math.max(0, list.contentHeight - list.height)
            list.contentY = Math.max(minY, Math.min(wanted, maxY))
            // Keep the guard through Qt's polish pass. contentY/model signals
            // can arrive after this callback returns; releasing synchronously
            // is what allowed two extra 500-row requests and the ~1,000-row
            // forward jump seen in the owner smoke.
            pageAnchorSettle.restart()
        })
    }

    // --------------------------- entry model ------------------------------
    property var entries: []
    property var alphaJumps: []
    function alphaKey(value) {
        var text = String(value || "").trim()
        if (text.length === 0) return "#"
        var first = text.charAt(0).toUpperCase()
        return first >= "A" && first <= "Z" ? first : "#"
    }
    /// entry index -> index into `view.tracksVisible`, -1 for a group header.
    /// Group headers inflate the entry indices, so reporting them straight
    /// into the row array asked for the covers of tracks BELOW the ones on
    /// screen, by the number of headers scrolled past.
    property var rowIndex: []
    function rebuild() {
        var anchor = root.pendingPageAnchor
        if (!anchor && root.pageRequestPending)
            anchor = root.capturePageAnchor()
        root.entriesEpoch += 1
        var epoch = root.entriesEpoch
        if (root.nativeModelActive) {
            legacyEntriesModel.clear()
            entries = []
            rowIndex = []
            try {
                alphaJumps = JSON.parse(QbzLocal.localTracksNativeJumpsJson) || []
            } catch (e) {
                alphaJumps = []
            }
            reportSoon()
            return
        }
        var rows = view ? view.tracksVisible : []
        var mode = view ? view.tracksGroup : "off"
        var out = []
        var jumps = []
        var jumpSeen = ({})
        var idx = []
        var prev = null
        for (var i = 0; i < rows.length; i++) {
            var t = rows[i]
            var key = mode === "album" ? (t.album || "")
                    : mode === "artist" ? (t.artist || "")
                    : mode === "name" ? (t.title || "").slice(0, 1).toUpperCase()
                    : ""
            if (mode !== "off" && key !== "" && key !== prev) {
                var letter = root.alphaKey(key)
                if (jumpSeen[letter] !== true) {
                    jumpSeen[letter] = true
                    jumps.push({ "letter": letter, "index": out.length })
                }
                out.push({ "t": 0, "label": key })
                idx.push(-1)
                prev = key
            }
            out.push({ "t": 1, "row": t, "n": i + 1 })
            idx.push(i)
        }
        alphaJumps = jumps
        rowIndex = idx
        root.publishLegacyEntries(out, anchor, epoch)
        report()
    }
    Component.onCompleted: { rebuild(); reportSoon() }
    Component.onDestruction: if (view) view.releaseWindow("tracks")
    Connections {
        target: root.view
        function onTracksVisibleChanged() { if (!root.nativeModelActive) root.rebuild() }
        function onTracksGroupChanged() { root.rebuild() }
        // The row artwork column is OFF by default; turning it on has to ask
        // for the covers the rows are already bound to.
        function onTrackArtworkChanged() { root.reportSoon() }
        function onArtworkRefresh() { root.reportSoon() }
    }
    Connections {
        target: QbzLocal
        function onLocalTracksNativeActiveChanged() { root.rebuild() }
        function onLocalTracksNativeJumpsJsonChanged() { root.rebuild() }
        function onLocalTracksLoadingMoreChanged() {
            if (!QbzLocal.localTracksLoadingMore)
                pageRequestRelease.restart()
        }
        function onLocalTracksLoadingChanged() {
            // Search/sort/reset supersedes any compatibility-page request.
            if (QbzLocal.localTracksLoading) {
                pageAnchorSettle.stop()
                pageRequestRelease.stop()
                root.restoringPageAnchor = false
                root.pendingPageAnchor = null
                root.pageRequestPending = false
            }
        }
    }
    Timer {
        id: pageAnchorSettle
        interval: 50
        repeat: false
        onTriggered: {
            root.restoringPageAnchor = false
            root.pendingPageAnchor = null
            root.pageRequestPending = false
            root.reportSoon()
        }
    }
    Timer {
        id: pageRequestRelease
        interval: 250
        repeat: false
        onTriggered: {
            if (root.restoringPageAnchor) {
                restart()
                return
            }
            // Error/stale-result escape hatch: if no model publication ever
            // consumed the request-time anchor, the list must remain usable.
            root.pendingPageAnchor = null
            root.pageRequestPending = false
        }
    }
    Connections {
        target: root.view ? root.view.nativeTracksModel : null
        function onDataChanged() { root.reportSoon() }
        function onModelReset() { root.reportSoon() }
    }

    // --------------------------- row play ---------------------------------
    // PARITY-DEBT #14. Rust used to queue `state.tracks_raw` and find the
    // clicked row in it while QML could present another order. The visible id
    // array still goes down explicitly, and the raw rows play the part of the
    // authoritative cache
    // (local_playback::order_by_visible, the local twin of
    // library_qt::order_by_visible).
    //
    // `view.tracksVisible` is THE array `entries` is built from — never read
    // `list.model` back off the view (see views/LibraryView.qml).
    function visibleTrackIds() {
        var rows = root.view ? root.view.tracksVisible : []
        var out = []
        for (var i = 0; i < rows.length; i++) out.push(rows[i].id)
        return out
    }
    function playRow(index, id) {
        if (root.nativeModelActive) QbzLocal.tracksNativePlay(index)
        else QbzLocal.playTracksVisible(JSON.stringify(visibleTrackIds()), id)
    }

    // First page = the shape of the 50px track rows (the Slint mounts a bare
    // 36px LoadingSpinner). ONE instance for the whole viewport.
    QbzSkeleton {
        variant: "rowList"
        anchors.fill: parent
        anchors.leftMargin: root.sideInset
        anchors.rightMargin: root.sideInset
        anchors.topMargin: 12
        visible: QbzLocal.localTracksLoading
        rowH: root.rowH
        rowGap: 0
        rowArt: root.view.trackArtwork
        rowArtSize: 36
        phase: root.view.skelPhase
    }
    LocalNote {
        visible: !QbzLocal.localTracksLoading
            && (root.nativeModelActive ? QbzLocal.localTracksNativeTotal === 0
                            : root.view.tracks.length === 0)
            && root.view.tracksSearch === ""
        text: QbzSession.tr("No tracks in your local library yet.", QbzSession.trRev)
    }
    LocalNote {
        visible: !QbzLocal.localTracksLoading
            && (root.nativeModelActive ? QbzLocal.localTracksNativeTotal === 0
                            : (root.view.tracks.length === 0
                               || root.view.tracksVisible.length === 0))
            && root.view.tracksSearch !== ""
        text: QbzSession.tr("No tracks match your search.", QbzSession.trRev)
    }

    Column {
        anchors.fill: parent
        anchors.leftMargin: root.sideInset
        anchors.rightMargin: root.sideInset
        anchors.topMargin: 12
        spacing: 8
        visible: !QbzLocal.localTracksLoading
            && (root.nativeModelActive ? QbzLocal.localTracksNativeTotal > 0
                            : root.view.tracksVisible.length > 0)

        QbzMultiSelectBar {
            visible: root.view.tracksMultiSelect
            width: parent.width
            selectedCount: root.view.tracksSelectedCount
            actions: [
                { "id": "select-all", "label": QbzSession.tr("Select all", QbzSession.trRev), "icon": "square-check-big", "danger": false, "needsSelection": false },
                { "id": "queue", "label": QbzSession.tr("Add to queue", QbzSession.trRev), "icon": "list-end", "danger": false, "needsSelection": true },
                { "id": "play-later", "label": QbzSession.tr("Play later", QbzSession.trRev), "icon": "list-plus", "danger": false, "needsSelection": true },
                { "id": "play-next", "label": QbzSession.tr("Play next", QbzSession.trRev), "icon": "list-start", "danger": false, "needsSelection": true },
                // NO "Remove from favorites": this tab is the whole local
                // library, not a favourites surface — removing from a
                // collection the user is not looking at is not an action this
                // context can offer. (It was doubly wrong: local hearts are
                // unwired, so `local_bulk.rs`'s arm is a log-only no-op.)
                { "id": "add-to-playlist", "label": QbzSession.tr("Add to playlist", QbzSession.trRev), "icon": "list-music", "danger": false, "needsSelection": true },
                { "id": "add-to-mixtape", "label": QbzSession.tr("Add to Mixtape/Collection", QbzSession.trRev), "icon": "cassette-tape", "danger": false, "needsSelection": true },
                { "id": "clear", "label": QbzSession.tr("Clear", QbzSession.trRev), "icon": "x", "danger": false, "needsSelection": true },
            ]
            onAction: function (id) { root.view.tracksBulkAction(id) }
        }

        Item {
            width: parent.width
            height: parent.height - (root.view.tracksMultiSelect ? 52 : 0)

            ListView {
                id: list
                anchors.fill: parent
                // The tab's 32px content inset already is the rail lane. Keep
                // rows aligned with every other Local Library surface, then
                // place A-Z and the scrollbar *inside that existing gutter*
                // below instead of shrinking the rows by another 42px.
                anchors.rightMargin: 0
                clip: true
                cacheBuffer: 50 * 12
                boundsBehavior: Flickable.StopAtBounds
                model: root.nativeModelActive
                    ? root.view.nativeTracksModel : legacyEntriesModel

                onContentYChanged: {
                    root.report()
                    // Infinite scroll: 600px of runway (Slint :1442).
                    root.requestNextPage(false)
                }
                // `contentYChanged` is normally sufficient, but a scrollbar
                // seek and a short, bounded flick may settle directly on the
                // end without another coordinate signal after layout. These
                // two public Flickable signals make that terminal state an
                // explicit page request as well.
                onAtYEndChanged: if (atYEnd) root.requestNextPage(false)
                onMovementEnded: root.requestNextPage(false)
                onModelChanged: root.report()
                onHeightChanged: root.report()
                onVisibleChanged: {
                    if (visible) root.reportSoon()
                    else if (root.view) root.view.releaseWindow("tracks")
                }

                delegate: Loader {
                    required property var modelData
                    width: list.width
                    height: root.nativeModelActive
                        ? root.rowH + (modelData.groupStart ? root.headerH : 0)
                        : (modelData.t === 0 ? root.headerH : root.rowH)
                    sourceComponent: root.nativeModelActive ? nativeRowComp
                        : (modelData.t === 0 ? headerComp : rowComp)

                    Component {
                        id: headerComp
                        Item {
                            Text {
                                x: 2
                                anchors.verticalCenter: parent.verticalCenter
                                width: parent.width - 4
                                text: modelData.label
                                color: theme.textSecondary
                                font.pixelSize: theme.fontLegal
                                font.weight: theme.weightSemibold
                                elide: Text.ElideRight
                            }
                        }
                    }
                    Component {
                        id: rowComp
                        LocalTrackRow {
                            width: list.width
                            view: root.view
                            item: modelData.row
                            // The row artwork column was rendering a
                            // permanently blank 36px cell: local track rows
                            // ship an EMPTY artPath (local_rows.rs:284) and
                            // nothing ever fed LocalTrackRow.artSource. It is
                            // the same id-keyed artMap every other local
                            // surface reads.
                            artSource: root.view.artMap[modelData.row.artKey] || ""
                            number: modelData.n
                            // Album column hides when the list is already
                            // grouped by album (Slint :1486).
                            showAlbum: root.view.tracksGroup !== "album"
                            showArtwork: root.view.trackArtwork
                            selectMode: root.view.tracksMultiSelect
                            checked: root.view.tracksSelected[modelData.row.id] === true
                            onPlayRequested: root.playRow(modelData.n - 1, modelData.row.id)
                            onEnqueueRequested: function (m) {
                                QbzLocal.enqueue("track", modelData.row.id, m)
                            }
                            onToggleSelect: function (mods) { root.view.toggleTrackSelected(modelData.row.id, mods) }
                        }
                    }
                    Component {
                        id: nativeRowComp
                        Item {
                            Text {
                                x: 2
                                y: 0
                                width: parent.width - 4
                                height: root.headerH
                                visible: modelData.groupStart && !modelData.loading
                                verticalAlignment: Text.AlignVCenter
                                text: modelData.groupLabel
                                color: theme.textSecondary
                                font.pixelSize: theme.fontLegal
                                font.weight: theme.weightSemibold
                                elide: Text.ElideRight
                            }
                            LocalTrackRow {
                                y: modelData.groupStart ? root.headerH : 0
                                width: parent.width
                                height: root.rowH
                                visible: !modelData.loading
                                enabled: !modelData.loading
                                view: root.view
                                item: modelData.row
                                artSource: modelData.row.artPath
                                    || root.view.artMap[modelData.row.artKey] || ""
                                number: modelData.n
                                showAlbum: root.view.tracksGroup !== "album"
                                showArtwork: root.view.trackArtwork
                                selectMode: root.view.tracksMultiSelect
                                checked: modelData.selected
                                nativeActions: true
                                nativeIndex: modelData.n - 1
                                onPlayRequested: QbzLocal.tracksNativePlay(modelData.n - 1)
                                onEnqueueRequested: function (m) {
                                    QbzLocal.tracksNativeEnqueue(modelData.n - 1, m)
                                }
                                onToggleSelect: function (mods) {
                                    QbzLocal.tracksNativeToggleSelect(
                                        modelData.n - 1,
                                        (mods & Qt.ShiftModifier) !== 0)
                                }
                            }
                            QbzSkeleton {
                                y: modelData.groupStart ? root.headerH : 0
                                width: parent.width
                                height: root.rowH
                                visible: modelData.loading
                                variant: "rowList"
                                rowH: root.rowH
                                rowGap: 0
                                rowArt: root.view.trackArtwork
                                rowArtSize: 36
                                phase: root.view.skelPhase
                            }
                        }
                    }
                }

                // Infinite scroll remains the fast path. The explicit shared
                // affordance is the recovery path: a platform/input sequence
                // that omits the expected end signal can never strand a large
                // catalog at a page boundary again.
                footer: Item {
                    width: list.width
                    height: loadMore.visible ? loadMore.height : 0
                    QbzLoadMore {
                        id: loadMore
                        width: parent.width
                        visible: !root.nativeModelActive
                            && QbzLocal.localTracksHasMore
                        busy: QbzLocal.localTracksLoadingMore
                        skeleton: "rows"
                        rowH: root.rowH
                        rowGap: 0
                        rowArtSize: 36
                        rowCount: 3
                        onClicked: root.requestNextPage(true)
                    }
                }
            }
            QbzAlphaStrip {
                visible: root.view.tracksGroup !== "off"
                anchors.right: parent.right
                // This parent ends 32px before the view edge. A negative
                // margin consumes that outer gutter: the 18px alphabet ends
                // 12px before the window edge, leaving a distinct 8px lane
                // for the auto-hiding scrollbar at 4px.
                anchors.rightMargin: -20
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                jumps: root.alphaJumps
                completeAlphabet: true
                onJump: function (ordinal, index) {
                    list.positionViewAtIndex(index, ListView.Beginning)
                }
            }
            // Back/forward scroll memory (controls/ScrollMemory.qml).
            ScrollMemory { target: list; scope: "local:tracks" }
            QbzScrollBar {
                anchors.right: parent.right
                // The bar belongs on the VIEW edge, not on the content edge.
                // Its parent sits inside the tab's 32px inset, so a plain 4px
                // margin put the bar 36px from the window — a visibly wide
                // empty channel with nothing in it (owner, 2026-08-16). The
                // negative margin cancels the inset and leaves the same 4px
                // gap the other lists have; the ROWS keep their inset.
                anchors.rightMargin: 4 - root.sideInset
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                target: list
                visible: list.contentHeight > list.height
            }
        }
    }

    // TRIGGERS: `contentY` + the model swap alone never fired for a list that
    // was still hidden behind `localTracksLoading` when its first page landed
    // — the covers only started resolving on the first scroll. Mount, the
    // rebuild, becoming visible and a resize report too.
    function report() {
        if (!view || !list) return
        // The row artwork column is gated on the appearance setting (default
        // OFF, for the 16K-track freeze). With it off nothing on this list
        // renders a cover, so nothing is worth decoding.
        var modelCount = root.nativeModelActive
            ? QbzLocal.localTracksNativeTotal : root.entries.length
        if (!list.visible || !view.trackArtwork || modelCount === 0) {
            view.releaseWindow("tracks")
            return
        }
        var first = list.indexAt(4, list.contentY + 1)
        var last = list.indexAt(4, list.contentY + Math.max(1, list.height) - 1)
        if (first < 0) first = 0
        if (last < 0) last = Math.min(modelCount - 1, first + 12)
        if (root.nativeModelActive) {
            var resident = []
            var nativeFirst = Math.max(0, first - 4)
            var nativeLast = Math.min(modelCount - 1, last + 4)
            for (var j = nativeFirst; j <= nativeLast; j++) {
                var data = root.view.nativeTracksModel.rowAt(j)
                if (!data.loading && data.row && data.row.artKey) resident.push(data.row)
            }
            if (resident.length === 0) {
                view.releaseWindow("tracks")
                return
            }
            view.queueWindowReport(resident, 0, resident.length - 1, "tracks")
            return
        }
        // Entry band -> row band (group headers carry no cover).
        var lo = -1
        var hi = -1
        for (var i = first; i <= last; i++) {
            var n = root.rowIndex[i]
            if (n === undefined || n < 0) continue
            if (lo < 0) lo = n
            hi = n
        }
        if (lo < 0) { lo = 0; hi = Math.min(view.tracksVisible.length - 1, 12) }
        view.queueWindowReport(view.tracksVisible, Math.max(0, lo - 4), hi + 4, "tracks")
    }

    /// Report now, then again once the ListView has laid out (`indexAt`
    /// answers -1 until then — the state a just-shown list is in).
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
