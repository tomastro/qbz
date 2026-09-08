// Local Library > Artists, kiosk body — reported DEAD in the release audit.
//
// Two causes, both fixed here:
//   1. the old kiosk read `QbzLocal.localArtistsJson`, which the LEGACY reader
//      publishes and which stops being republished the moment the paged
//      `QbzLocalArtists` surface goes active (the production default). A live
//      library rendered an empty avatar wall;
//   2. it mounted a `Repeater` over EVERY artist with a per-slot Loader —
//      20 487 items and 10 075 Loaders at the 10 000-artist fixture, 1.87 s to
//      idle (K0 runtime baseline). Nothing about that is virtualised.
//
// Now: ONE windowing ListView over whichever reader is authoritative, with a
// 72px touch row per artist. A LIST rather than a grid because the native
// model's rows ARE one artist each (plus A-Z letter bands) — chunking a paged
// model into grid rows would mean materialising it, which is the whole thing
// the model exists to avoid.
//
// SELECTION IS A DRILL-DOWN, not a split. The desktop pane is a 280px rail
// beside an album grid; at 800×480 that leaves 520px for the grid, i.e. two
// cards. Tapping an artist replaces the list with that artist's albums and a
// back chip. Selecting an artist records NO history entry — same as Full UI,
// where `selectedArtist` is opaque view state restored with the entry, never a
// destination of its own.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    id: root

    property var view: null

    QbzTheme { id: theme }

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    readonly property string nativeError: root.nativeActive ? QbzLocal.localArtistsNativeError : ""
    readonly property bool nativeActive: QbzLocal.localArtistsNativeActive
    readonly property var nativeModel: QbzLocalArtists
    readonly property var nativeAlbumModel: QbzLocalArtistAlbums
    readonly property int artistTotal: root.nativeActive
        ? (QbzLocal.localArtistsNativeTotal || 0)
        : (root.view ? root.view.artists.length : 0)

    readonly property string selectedArtist: root.view ? root.view.selectedArtist : ""
    readonly property bool drilled: root.selectedArtist !== ""

    readonly property real rowH: 72
    readonly property real headerH: 26

    // Grid sizing for the round-card rail (owner feedback 2026-09-07:
    // homologate the single-column rail to Library > Artists' round-card grid).
    // Mirrors KioskLibrary's feedGrid roundCells math: ~5 columns at 800px,
    // ~9 at 1280px.
    readonly property real gridPad: root.view ? root.view.pad : 16
    readonly property real gridGap: 14
    readonly property real gridMinCell: 132
    readonly property real gridLabelH: 46
    readonly property real gridAvailRow: Math.max(0, width - 2 * gridPad + gridGap)
    readonly property int gridColumns: width > 0
        ? Math.max(2, Math.floor(gridAvailRow / (gridMinCell + gridGap)))
        : 0
    readonly property real gridRightPad: Math.max(0, gridPad - gridGap)
    readonly property real gridAvail: Math.max(0, width - gridPad - gridRightPad)
    readonly property real gridCellW: gridColumns > 0 ? Math.max(1, Math.floor(gridAvail / gridColumns)) : 1
    readonly property real gridCardW: Math.max(0, gridCellW - gridGap)
    readonly property real gridCellH: Math.max(1, gridCardW + gridLabelH + gridGap)

    // ---------------------------------------------------------------------
    // Readers
    // ---------------------------------------------------------------------
    // Legacy entries mirror the native row shape ({t, item}) so ONE delegate
    // serves both and the two paths cannot drift apart visually.
    readonly property var legacyEntries: {
        if (root.nativeActive || !root.view)
            return []
        var rows = root.view.artists
        var out = []
        for (var i = 0; i < rows.length; i++)
            out.push({ "t": 1, "loading": false, "item": rows[i] })
        return out
    }
    readonly property int entryCount: root.nativeActive
        ? (root.nativeModel.totalCount || 0) : root.legacyEntries.length

    /// The selected artist's albums on the LEGACY path. The match itself lives
    /// in Rust (`artistAlbumIds` -> local_artist_match), because it is the same
    /// normalised-name rule the Artists merge uses and there must be one of it.
    readonly property var legacyArtistAlbums: {
        if (root.nativeActive || !root.drilled || !root.view)
            return []
        var ids = ({})
        try {
            var parsed = JSON.parse(QbzLocal.artistAlbumIds(root.selectedArtist))
            for (var j = 0; j < parsed.length; j++)
                ids[parsed[j]] = true
        } catch (e) {
            return []
        }
        var rows = root.view.albums
        var out = []
        for (var i = 0; i < rows.length; i++)
            if (ids[rows[i].id])
                out.push(rows[i])
        return out
    }
    readonly property int artistAlbumTotal: root.nativeActive
        ? (QbzLocal.localArtistAlbumsNativeTotal || 0) : root.legacyArtistAlbums.length

    // ---------------------------------------------------------------------
    // Native queries
    // ---------------------------------------------------------------------
    // `artists_native_reset` REFUSES a non-default sort or funnel: a filtered
    // descriptor makes Rust drop the native reader (`deactivate_for_legacy`)
    // WITHOUT publishing the legacy document, which would leave this rail
    // empty. So the rail keeps the neutral descriptor and lists every artist;
    // the host's funnel (KioskLocalLibrary.qml) applies to the LEGACY reader
    // (rail and drill-down) and to the Albums, Genres and Tracks tabs. The
    // native drill-down (`artistsNativeSelect`) carries no funnel either —
    // a Rust seam this kiosk-only round does not add.
    Timer {
        id: railQuery
        interval: 0
        repeat: false
        onTriggered: QbzLocal.artistsNativeReset("", "name-asc", "{}")
    }
    Timer {
        id: detailQuery
        interval: 0
        repeat: false
        onTriggered: {
            if (root.selectedArtist === "")
                return
            // NO `nativeActive` gate and no geometry gate. The native flag
            // can be false at the instant this fires (a fresh mount issues
            // the rail reset in the same tick; a restored drill-down mounts
            // before activation) and the query was silently dropped — the
            // "0 albums" drill-down of 2026-09-07. Rust refuses the call
            // itself when the native reader is not in play (`select_artist`
            // checks `requested()` and the album mode), so an unconditional
            // call costs nothing there, and the legacy drill-down keeps
            // reading `legacyArtistAlbums` regardless. Rust also clamps
            // columns to >= 1; `onColumnsChanged` re-issues once settled.
            QbzLocal.artistsNativeSelect(root.selectedArtist, Math.max(1, albumGrid.columns))
        }
    }
    Component.onCompleted: {
        railQuery.restart()
        if (root.drilled)
            detailQuery.restart()
        root.publishNav()
    }
    // THE 2026-09-07 "0 albums" DEFECT. This used to be a root
    // `onSelectedArtistChanged` handler guarded by `if (root.drilled)`. Inside
    // that handler the DERIVED readonly `drilled` is still the previous value
    // (the alias has changed, its dependents have not been re-evaluated yet —
    // verified with a bare `qml` scene: handler sees drilled=false while the
    // alias already reads "A-ha"), so the detail query was never issued and
    // every kiosk drill-down showed "0 albums" with nothing in the log. The
    // desktop tab reacts through a Connections on the HOST's signal
    // (views/local/LocalArtistsTab.qml:89), where every derived value is
    // fresh; this is that pattern, and the guards below read the SOURCE
    // rather than a derived property.
    Connections {
        target: root.view
        function onSelectedArtistChanged() {
            if (root.view.selectedArtist !== "")
                detailQuery.restart()
            root.publishNav()
            Qt.callLater(root.reportSoon)
        }
    }
    Connections {
        target: albumGrid
        function onColumnsChanged() {
            if (root.drilled)
                detailQuery.restart()
            root.publishNav()
        }
    }

    // The focus geometry follows the drill-down: the list is one column over
    // the model's rows, the detail is the album grid's own pitch.
    function publishNav() {
        if (!root.view)
            return
        if (root.drilled)
            root.view.publishNav(Math.max(1, albumGrid.columns), root.artistAlbumTotal)
        else
            root.view.publishNav(Math.max(1, root.gridColumns), root.entryCount)
    }
    onEntryCountChanged: root.publishNav()
    onArtistAlbumTotalChanged: root.publishNav()
    Connections {
        target: QbzLocal
        function onLocalArtistsNativeActiveChanged() {
            root.reportSoon()
            if (root.drilled)
                detailQuery.restart()
        }
        function onLocalArtistsLoadingChanged() {
            if (!QbzLocal.localArtistsLoading) {
                root.reportSoon()
                if (root.drilled)
                    detailQuery.restart()
            }
        }
        // Resolved covers have to reach the paged rows, which carry their own
        // `artPath`; the id-keyed map alone only serves the legacy reader.
        function onLocalArtworkReady(key, path) {
            root.nativeModel.setArtwork(key, path)
            root.nativeAlbumModel.setArtwork(key, path)
        }
    }
    // Page requests are only emitted while this signal has a receiver, so both
    // subscriptions belong to the mounted tab.
    Connections {
        target: root.nativeModel
        function onDataChanged() { root.reportSoon() }
        function onModelReset() { root.reportSoon() }
        function onPageMiss(page, generation) {
            QbzLocal.artistsNativePageMiss(page, generation)
        }
    }
    Connections {
        target: root.nativeAlbumModel
        function onPageMiss(page, generation) {
            QbzLocal.artistAlbumsNativePageMiss(page, generation)
        }
    }

    // ---------------------------------------------------------------------
    // Focus. The index space is the model's ROWS, letter bands included: a
    // paged model cannot answer "the Nth artist" without materialising the
    // pages in between. A band simply does nothing on Enter.
    // ---------------------------------------------------------------------
    readonly property int focusedItem: QbzKioskNav.index - QbzKioskNav.tabs
    readonly property bool itemFocused: QbzKioskNav.navActive
        && QbzKioskNav.zone === "content"
        && !root.drilled
        && root.focusedItem >= 0
        && root.focusedItem < root.entryCount

    function entryAt(index) {
        if (root.nativeActive)
            return root.nativeModel.rowAt(index)
        return root.legacyEntries[index] || null
    }

    Connections {
        target: QbzKioskNav
        function onIndexChanged() {
            if (root.itemFocused)
                Qt.callLater(function () {
                    rail.positionViewAtIndex(root.focusedItem, GridView.Contain)
                })
        }
        function onActivateSeqChanged() {
            if (!root.itemFocused)
                return
            var entry = root.entryAt(root.focusedItem)
            if (entry && !entry.loading && entry.t === 1 && entry.item)
                root.select(entry.item.name)
        }
    }

    function select(name) {
        if (root.view)
            root.view.selectArtist(name || "")
    }

    // ---------------------------------------------------------------------
    // Artwork window — the rail's mounted band. The drill-down grid reports
    // its own window through its own surface slot.
    // ---------------------------------------------------------------------
    function report() {
        if (!root.view)
            return
        if (!rail.visible || root.entryCount === 0) {
            root.view.releaseWindow("artists")
            return
        }
        // Grid window: one row of overscan on each side, in ENTRY units
        // (each cell is one entry — bands included).
        var cols = Math.max(1, root.gridColumns)
        var ch = Math.max(1, root.gridCellH)
        var firstRow = Math.max(0, Math.floor((rail.contentY - rail.originY) / ch) - 1)
        var lastRow = Math.floor((rail.contentY - rail.originY + Math.max(1, rail.height)) / ch) + 1
        var first = Math.max(0, firstRow * cols)
        var last = Math.min(root.entryCount - 1, (lastRow + 1) * cols - 1)

        var resident = []
        for (var i = first; i <= last; i++) {
            var entry = root.entryAt(i)
            if (!entry || entry.loading || entry.t !== 1 || !entry.item)
                continue
            resident.push(entry.item)
        }
        if (resident.length === 0)
            root.view.releaseWindow("artists")
        else
            root.view.queueWindowReport(resident, 0, resident.length - 1, "artists")
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
    Component.onDestruction: if (root.view) root.view.releaseWindow("artists")
    Connections {
        target: root.view
        function onArtworkRefresh() { root.reportSoon() }
    }

    // =====================================================================
    // The artist list
    // =====================================================================
    GridView {
        id: rail
        anchors.fill: parent
        visible: !root.drilled && !QbzLocal.localArtistsLoading
            && root.nativeError === ""
            && root.artistTotal > 0
        clip: true
        leftMargin: root.gridPad
        rightMargin: root.gridRightPad
        topMargin: root.gridPad
        bottomMargin: root.gridPad
        cellWidth: root.gridCellW
        cellHeight: root.gridCellH
        cacheBuffer: Math.max(0, Math.ceil(root.gridCellH))
        boundsBehavior: Flickable.StopAtBounds
        reuseItems: true
        model: rail.visible ? (root.nativeActive ? root.nativeModel : root.legacyEntries) : []

        onContentYChanged: root.report()
        onModelChanged: root.report()
        onHeightChanged: root.report()
        onVisibleChanged: {
            if (visible)
                root.reportSoon()
            else if (root.view)
                root.view.releaseWindow("artists")
        }

        // Uniform cell. Bands (t:0) become a centered letter tile — Library >
        // Artists has no bands, but keeping them as inline letter tiles marks
        // the A-Z groups without breaking the grid's uniform geometry.
        delegate: Item {
            id: entrySlot
            required property var modelData
            required property int index

            readonly property bool entryLoading: entrySlot.modelData
                && entrySlot.modelData.loading === true
            readonly property bool band: !entrySlot.entryLoading
                && entrySlot.modelData && entrySlot.modelData.t === 0
            readonly property var item: entrySlot.modelData
                ? entrySlot.modelData.item : null

            width: root.gridCellW
            height: root.gridCellH

            // A-Z band letter, centered in the cell's art box.
            Text {
                width: root.gridCardW
                height: root.gridCardW
                x: (root.gridCellW - width) / 2
                visible: entrySlot.band
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                text: entrySlot.band ? entrySlot.modelData.label : ""
                color: theme.textSecondary
                font.pixelSize: 34
                font.weight: theme.weightBold
            }

            // Page not resident: a static circular tile, no avatar request.
            Rectangle {
                width: root.gridCardW
                height: root.gridCardW
                x: (root.gridCellW - width) / 2
                visible: entrySlot.entryLoading
                radius: width / 2
                color: theme.surfaceElevated
                opacity: 0.55
            }

            // The artist — a round card (title + album count), the whole tile
            // is the tap target; tapping drills into that artist's albums.
            KioskCard {
                visible: !entrySlot.band && !entrySlot.entryLoading
                round: true
                artSize: root.gridCardW
                navFocused: root.itemFocused && root.focusedItem === entrySlot.index
                album: ({
                    "id": entrySlot.item ? (entrySlot.item.name || "") : "",
                    "title": entrySlot.item ? (entrySlot.item.name || "") : "",
                    "artist": entrySlot.item
                        ? ((entrySlot.item.albumCount || 0) + " " + root.t("albums")) : "",
                    "artwork": entrySlot.item
                        ? (entrySlot.item.artPath
                           || (root.view ? root.view.artPathOf(entrySlot.item.artKey) : "")) : ""
                })
                onClicked: function (name) { if (name !== "") root.select(name) }
            }
        }
    }

    ScrollMemory { target: rail; scope: "local:artists"; relativeToOrigin: true }

    // =====================================================================
    // The drill-down: one artist's albums
    // =====================================================================
    Item {
        anchors.fill: parent
        visible: root.drilled

        Rectangle {
            id: detailBar
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            height: 64
            color: "transparent"

            Row {
                anchors.left: parent.left
                anchors.leftMargin: root.view ? root.view.pad : 16
                anchors.right: parent.right
                anchors.rightMargin: root.view ? root.view.pad : 16
                anchors.verticalCenter: parent.verticalCenter
                spacing: 12

                // 64px primary touch target — leaving the drill-down is the
                // one action this bar has to make impossible to miss.
                Rectangle {
                    id: backChip
                    width: 64
                    height: 48
                    radius: theme.radiusSm
                    color: theme.surfaceElevated
                    border.width: 1
                    border.color: theme.borderSubtle

                    QbzIcon {
                        anchors.centerIn: parent
                        name: "chevron-left"
                        width: 22
                        height: 22
                        tintName: "textPrimary"
                    }
                    MouseArea {
                        anchors.fill: parent
                        // A generous margin turns the 64x48 chip into a
                        // >64px effective target without moving the visual.
                        anchors.margins: -8
                        onClicked: root.select("")
                    }
                }

                Column {
                    width: parent.width - 64 - 12
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 2

                    Text {
                        width: parent.width
                        text: root.selectedArtist
                        color: theme.textPrimary
                        font.pixelSize: 18
                        font.weight: theme.weightBold
                        elide: Text.ElideRight
                        maximumLineCount: 1
                    }
                    Text {
                        width: parent.width
                        text: root.artistAlbumTotal + " " + root.t("albums")
                        color: theme.textMuted
                        font.pixelSize: 13
                        elide: Text.ElideRight
                    }
                }
            }
        }

        KioskLocalAlbumGrid {
            id: albumGrid
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: detailBar.bottom
            anchors.bottom: parent.bottom
            visible: root.drilled && !QbzLocal.localArtistAlbumsLoading
                && root.artistAlbumTotal > 0
            view: root.view
            surface: "artist-albums"
            scrollScope: "local:artists:albums"
            pad: root.view ? root.view.pad : 16
            nativeActive: root.nativeActive
            nativeModel: root.nativeAlbumModel
            rows: root.nativeActive ? [] : root.legacyArtistAlbums
            onOpen: function (id) {
                if (root.view)
                    root.view.openAlbum(id)
            }
        }

        KioskSkeleton {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: detailBar.bottom
            anchors.bottom: parent.bottom
            kind: "grid"
            columns: Math.max(2, albumGrid.columns)
            pad: albumGrid.pad
            gap: albumGrid.gap
            loading: root.drilled && QbzLocal.localArtistAlbumsLoading
            empty: root.drilled && !QbzLocal.localArtistAlbumsLoading
                && root.artistAlbumTotal === 0
            emptyText: root.t("No albums found")
        }
    }

    // Rail-level loading / error / empty. Static, mounts no artwork.
    KioskSkeleton {
        anchors.fill: parent
        kind: "list"
        rowHeight: root.rowH
        rowArtSize: 52
        pad: root.view ? root.view.pad : 16
        loading: !root.drilled && QbzLocal.localArtistsLoading
        error: root.drilled ? "" : root.nativeError
        empty: !root.drilled && !QbzLocal.localArtistsLoading
            && root.nativeError === ""
            && root.artistTotal === 0
        emptyText: root.t("No artists in your local library yet.")
    }
}
