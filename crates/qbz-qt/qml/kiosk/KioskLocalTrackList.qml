// KioskLocalTrackList — the bounded Local Library track surface for kiosk.
//
// The desktop Tracks tab is the view with the documented 16K-row freeze, and
// the answer there is the paged native model `QbzLocalTracks` (250-row keyset
// pages behind an eight-page LRU) with the accumulated-JSON reader kept as the
// automatic fallback when a catalog session fails. This is the kiosk consumer
// of the SAME two readers — it never builds its own document and it never
// falls back to the legacy JSON while the native model is active, which is
// precisely the defect the release audit found (`localTracksJson` stays at its
// last value once the native path takes over, so a kiosk reading it renders an
// empty or stale list).
//
// ONE ListView, constant row pitch plus an optional group band, `reuseItems`
// on, `cacheBuffer` of at most one row per side. Mounted delegates are proportional to the
// viewport, never to the 20 000-row model.
//
// TOUCH: 64px rows — the contract's primary target — set on the delegate, so
// the shared KioskTrackRow's desktop-era 62 is not edited for the kiosk.
//
// ARTWORK: gated on `QbzLocal.localTrackArtwork`, exactly like Full UI, whose
// default is OFF for the freeze reason. With it off this surface reports NO
// artwork window at all rather than decoding covers nothing renders.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    id: root

    /// The KioskLocalLibrary root (artwork registry + artPathOf).
    property var view: null
    property string surface: "tracks"
    property string scrollScope: ""

    property bool nativeActive: false
    property var nativeModel: null
    /// Legacy accumulated-page rows.
    property var rows: []
    /// Legacy paging is the fallback reader's own contract; the native model
    /// pages itself through `pageMiss`.
    property bool legacyPaging: true

    QbzTheme { id: theme }

    readonly property real rowH: 64
    readonly property real headerH: 28
    readonly property int trackCount: root.nativeActive && root.nativeModel
        ? (root.nativeModel.totalCount || 0) : root.rows.length

    readonly property bool wantArtwork: QbzLocal.localTrackArtwork

    // ---------------------------------------------------------------------
    // Focus (QbzKioskNav) — the leading `tabs` entries are the tab strip.
    // ---------------------------------------------------------------------
    readonly property int focusedItem: QbzKioskNav.index - QbzKioskNav.tabs
    readonly property bool itemFocused: QbzKioskNav.navActive
        && QbzKioskNav.zone === "content"
        && root.focusedItem >= 0
        && root.focusedItem < root.trackCount

    function scrollFocusIntoView() {
        if (!root.itemFocused)
            return
        list.positionViewAtIndex(root.focusedItem, ListView.Contain)
    }

    Connections {
        target: QbzKioskNav
        function onIndexChanged() { Qt.callLater(root.scrollFocusIntoView) }
        function onActivateSeqChanged() {
            if (root.itemFocused)
                root.playAt(root.focusedItem)
        }
    }

    // ---------------------------------------------------------------------
    // Playback
    // ---------------------------------------------------------------------
    /// Index-based on the native path so the source-native TrackRef stays in
    /// Rust; the legacy path still sends the visible id order, which is the
    /// contract `play_tracks_visible` documents.
    function playAt(index) {
        if (root.nativeActive) {
            QbzLocal.tracksNativePlay(index)
            return
        }
        var row = root.rows[index]
        if (!row)
            return
        var ids = []
        for (var i = 0; i < root.rows.length; i++)
            ids.push(root.rows[i].id)
        QbzLocal.playTracksVisible(JSON.stringify(ids), row.id)
    }

    /// The KioskTrackRow object for one row of either reader.
    function trackOf(item) {
        if (!item)
            return ({})
        return {
            "id": item.id,
            "title": item.title || "",
            "artist": item.artist || "",
            "duration": item.duration || "",
            "artwork": root.wantArtwork
                ? (item.artPath || (root.view ? root.view.artPathOf(item.artKey) : ""))
                : ""
        }
    }

    // ---------------------------------------------------------------------
    // Legacy paging — the fallback reader appends 500-row pages on scroll.
    // ---------------------------------------------------------------------
    function requestNextPage() {
        if (root.nativeActive || !root.legacyPaging
            || !QbzLocal.localTracksHasMore
            || QbzLocal.localTracksLoadingMore
            || QbzLocal.localTracksLoading)
            return
        var scrollY = Math.max(0, list.contentY - list.originY)
        if (!list.atYEnd && scrollY + list.height < list.contentHeight - 600)
            return
        QbzLocal.tracksLoadMore()
    }

    // ---------------------------------------------------------------------
    // Artwork window — the mounted band, and only when covers are drawn.
    // ---------------------------------------------------------------------
    function report() {
        if (!root.view)
            return
        if (!list.visible || !root.wantArtwork || root.trackCount === 0) {
            root.view.releaseWindow(root.surface)
            return
        }
        var first = list.indexAt(4, list.contentY + 1)
        var last = list.indexAt(4, list.contentY + Math.max(1, list.height) - 1)
        if (first < 0)
            first = Math.max(0, Math.floor((list.contentY - list.originY) / root.rowH))
        if (last < 0)
            last = Math.min(root.trackCount - 1, first + Math.ceil(list.height / root.rowH))
        first = Math.max(0, first - 1)
        last = Math.min(root.trackCount - 1, last + 1)

        var resident = []
        var i
        if (root.nativeActive && root.nativeModel) {
            for (i = first; i <= last; i++) {
                var entry = root.nativeModel.rowAt(i)
                if (!entry || entry.loading || !entry.row || !entry.row.artKey)
                    continue
                resident.push(entry.row)
            }
        } else {
            for (i = first; i <= last; i++) {
                var legacy = root.rows[i]
                if (legacy && legacy.artKey)
                    resident.push(legacy)
            }
        }
        if (resident.length === 0)
            root.view.releaseWindow(root.surface)
        else
            root.view.queueWindowReport(resident, 0, resident.length - 1, root.surface)
    }

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

    onWantArtworkChanged: root.reportSoon()
    onRowsChanged: root.reportSoon()
    onNativeActiveChanged: root.reportSoon()
    onVisibleChanged: {
        if (root.visible)
            root.reportSoon()
        else if (root.view)
            root.view.releaseWindow(root.surface)
    }
    Component.onCompleted: root.reportSoon()
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

    ListView {
        id: list
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        clip: true
        topMargin: 8
        bottomMargin: 8
        spacing: 0
        // At most one row per side, including at the 192px content floor.
        cacheBuffer: Math.min(root.rowH, Math.max(0, list.height / 2))
        boundsBehavior: Flickable.StopAtBounds
        reuseItems: true
        model: root.visible ? (root.nativeActive && root.nativeModel ? root.nativeModel : root.rows) : []

        onContentYChanged: {
            root.report()
            root.requestNextPage()
        }
        onAtYEndChanged: if (atYEnd) root.requestNextPage()
        onMovementEnded: root.requestNextPage()
        onModelChanged: root.report()
        onHeightChanged: root.report()
        onVisibleChanged: if (visible) root.reportSoon()

        delegate: Item {
            id: rowSlot
            required property var modelData
            required property int index

            /// The native reader wraps the row; the legacy reader IS the row.
            readonly property var entry: root.nativeActive
                ? rowSlot.modelData : ({ "row": rowSlot.modelData, "loading": false })
            readonly property bool rowLoading: rowSlot.entry
                && rowSlot.entry.loading === true
            readonly property bool grouped: root.nativeActive
                && rowSlot.modelData && rowSlot.modelData.groupStart === true
                && !rowSlot.rowLoading

            width: list.width
            height: root.rowH + (rowSlot.grouped ? root.headerH : 0)

            Text {
                x: 8
                width: parent.width - 16
                height: root.headerH
                visible: rowSlot.grouped
                verticalAlignment: Text.AlignVCenter
                text: rowSlot.grouped ? rowSlot.modelData.groupLabel : ""
                color: theme.textSecondary
                font.pixelSize: theme.fontLegal
                font.weight: theme.weightSemibold
                elide: Text.ElideRight
            }

            // Page not resident yet — a static tile, no image, no row.
            Rectangle {
                y: rowSlot.grouped ? root.headerH : 0
                width: parent.width
                height: root.rowH - 6
                visible: rowSlot.rowLoading
                radius: theme.radiusSm
                color: theme.surfaceElevated
                opacity: 0.55
            }

            KioskTrackRow {
                y: rowSlot.grouped ? root.headerH : 0
                width: parent.width
                // 64px is the contract's primary touch target; the shared
                // primitive's desktop-era 62 is overridden here, not edited.
                height: root.rowH
                visible: !rowSlot.rowLoading
                enabled: !rowSlot.rowLoading
                track: root.trackOf(rowSlot.entry ? rowSlot.entry.row : null)
                navFocused: root.itemFocused && root.focusedItem === rowSlot.index
                onClicked: root.playAt(rowSlot.index)
            }
        }
    }

    ScrollMemory { target: list; scope: root.scrollScope }
}
