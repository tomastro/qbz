// KioskLibrary — the kiosk Library (favourites) view.
//
// ── WHAT THE AUDIT FOUND ──────────────────────────────────────────────────
// Three release blockers, all fixed here.
//
// 1. IT OPENED ON TRACKS. `activeTab` started at "tracks" and `KioskShell`
//    navigated Library with the tab "tracks", so entering Library from the
//    rail mounted the track list. Contract §2.2: Library opens on ALBUMS —
//    from the rail, from a fresh mount and from any programmatic navigation
//    that carries no tab. Back/Forward is the one exception, and it is the
//    point: an entry that recorded Tracks restores Tracks.
//
// 2. THREE TABS WERE UNBOUNDED. Artists, Labels and Playlists mounted a
//    `Repeater` over the whole subset. K0 measured the Artists tab at the
//    10 000-artist fixture: 130 037 visual items, 20 006 Loaders, 10 000 live
//    images and 8 357 ms to idle; Playlists and Labels, 26 037 items and 2 000
//    images each (evidence/runtime-baseline/partial-initial.jsonl). Every tab
//    is a GridView or a ListView now — the mounted delegate count is the
//    viewport plus one row of overscan, whatever the model holds.
//
// 3. COVERS WERE CAPPED, NOT WINDOWED. `albumCoverCap = 60` reproduced the
//    Slint's frozen FAV_ALBUMS_WINDOW literally: album 61 kept a bare tile
//    forever, and the non-album tabs went the other way and dispatched EVERY
//    row's key at once. Both are replaced by one rule — report the mounted
//    band, release what scrolls away.
//
// ── WHY THE GRID IS INLINE AND NOT KioskAlbumGrid ─────────────────────────
// The primitive takes `albums` as a plain array and its delegate binds
// `album: modelData`, so a resolved cover can only reach a card by REBUILDING
// that array. Handing a GridView a new model runs QQuickItemView::setModel(),
// which resets contentY to 0 and rebuilds every delegate — i.e. the grid would
// jump to the top each time a batch of covers landed, which is exactly while
// the user is looking at it. The grid below keeps the model IMMUTABLE (it
// changes only when the feed does) and lets each mounted cell bind its own
// `artwork` out of `artMap`, so an arrival re-evaluates a handful of strings
// and moves nothing. Everything else — pitch algebra, `positionViewAtIndex`
// focus, one row of overscan — is the primitive's, deliberately.
//
// ── ONE CARD, THREE SHAPES ────────────────────────────────────────────────
// KioskCard already declares `round`, which switches it to a circular tile
// with centred text: that is the artist/label shape. Playlists are the square
// shape over `covers[0]` (the first MEMBER-album cover — `artMap[artKey]`
// would be the playlist's own graphic, blank for most). So all four grids
// share one delegate and there is no per-cell Loader.

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root

    color: "transparent"

    QbzTheme { id: theme }

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    property real pad: 16

    // =====================================================================
    // Tabs
    // =====================================================================
    // ALBUMS FIRST, and it is the default. The strip order follows.
    readonly property var tabIds: ["albums", "tracks", "artists", "playlists", "labels"]

    property string activeTab: "albums"

    function tabLabel(id) {
        return id === "albums" ? root.t("Albums")
            : id === "tracks" ? root.t("Tracks")
            : id === "artists" ? root.t("Artists")
            : id === "playlists" ? root.t("Playlists")
            : root.t("Labels")
    }

    /// THE recording setter: the strip, the router handshake and the NavRail
    /// all land here, and the history entry is pushed with the OUTGOING state.
    function activateTab(tab) {
        if (!tab || tab === root.activeTab || root.tabIds.indexOf(tab) < 0)
            return
        nav.recordTab(tab)
        root.activeTab = tab
    }

    // ContentRouter's one external tab seam. `sequence` makes re-selecting the
    // same entry re-apply.
    property var tabNavigationRequest: ({})
    onTabNavigationRequestChanged: {
        if (root.tabNavigationRequest && root.tabNavigationRequest.tab)
            root.activateTab(root.tabNavigationRequest.tab)
    }

    onActiveTabChanged: {
        root.publishNav()
        root.resetWindow()
    }

    Component.onCompleted: root.publishNav()

    // =====================================================================
    // History — the shared stack, this view's opaque state on it
    // =====================================================================
    KioskNavigation {
        id: nav
        route: "library"
        snapshot: ({ "activeTab": root.activeTab })
        onRestore: function (saved) {
            if (!saved)
                return
            var tab = typeof saved.activeTab === "string" ? saved.activeTab : ""
            // A restored entry keeps the tab it recorded, Tracks included; an
            // unknown one falls back to the contract default rather than
            // leaving the view on a tab that no longer exists.
            root.activeTab = root.tabIds.indexOf(tab) >= 0 ? tab : "albums"
        }
    }

    // =====================================================================
    // The merged feed and its five subsets
    // =====================================================================
    // ONE guarded parse: a raw JSON.parse inside a binding throws on the
    // pre-publish frame and takes the whole view down.
    readonly property var feed: root.parseFeed()
    function parseFeed() {
        try {
            var parsed = JSON.parse(QbzLibrary.libraryJson)
            return (parsed instanceof Array) ? parsed : []
        } catch (e) {
            return []
        }
    }

    /// `group` empty = the kind alone decides (artists, labels).
    function subset(kind, group) {
        var out = []
        var f = root.feed
        for (var i = 0; i < f.length; i++) {
            var x = f[i]
            if (x.kind === kind && (group === "" || x.group === group))
                out.push(x)
        }
        return out
    }

    readonly property var trackRows: root.activeTab === "tracks" ? root.subset("track", "favorites") : []
    readonly property var albumRows: root.activeTab === "albums" ? root.subset("album", "favorites") : []
    readonly property var artistRows: root.activeTab === "artists" ? root.subset("artist", "") : []
    readonly property var playlistRows: root.activeTab === "playlists" ? root.subset("playlist", "favorites") : []
    readonly property var labelRows: root.activeTab === "labels" ? root.subset("label", "") : []

    /// The rows the mounted tab renders. IMMUTABLE while the feed is: a cover
    /// arrival must not replace it (see the header).
    readonly property var activeRows: root.activeTab === "albums" ? root.albumRows
        : root.activeTab === "tracks" ? root.trackRows
        : root.activeTab === "artists" ? root.artistRows
        : root.activeTab === "playlists" ? root.playlistRows
        : root.labelRows

    readonly property bool ready:
        !QbzLibrary.libraryLoading && QbzLibrary.libraryError === ""

    function emptyTextFor(tab) {
        return tab === "albums" ? root.t("No albums in your Library yet.")
            : tab === "tracks" ? root.t("No tracks in your Library yet.")
            : tab === "artists" ? root.t("No artists in your Library yet.")
            : tab === "playlists" ? root.t("No playlists in your Library yet.")
            : root.t("No labels in your Library yet.")
    }

    // =====================================================================
    // Covers — the MOUNTED band, never a fixed cap
    // =====================================================================
    // {artKey: "file://…"}. Arrivals are coalesced into one rebind per frame:
    // rebinding per arrival re-evaluates every mounted cell's `artwork` once
    // per cover, which is quadratic in the window.
    property var artMap: ({})
    property var _artInbox: ({})
    /// The last reported band, as [first, last] into `activeRows`.
    property int _windowFirst: -1
    property int _windowLast: -1

    function artPathOf(key) { return root.artMap[key] || "" }

    Timer {
        id: artFlush
        interval: 16
        repeat: false
        onTriggered: {
            // A rebind needs a NEW object reference — a same-ref assignment is
            // not a change in QML.
            root.artMap = Object.assign({}, root.artMap, root._artInbox)
            root._artInbox = ({})
        }
    }

    Connections {
        target: QbzLibrary
        function onLibraryArtworkReady(key, path) {
            root._artInbox[key] = path
            if (!artFlush.running)
                artFlush.start()
        }
    }

    /// Report the mounted band. Keys inside the band plus one band of margin
    /// survive; everything else is dropped, so a long scroll does not grow the
    /// map without bound. A key already resolved is never re-requested — a
    /// re-request costs Rust a stat per key, which a scroll would pay per pass.
    function reportWindow(first, last, pixels) {
        var rows = root.activeRows
        if (rows.length === 0 || first < 0 || last < first) {
            root._windowFirst = -1
            root._windowLast = -1
            return
        }
        first = Math.max(0, first)
        last = Math.min(rows.length - 1, last)
        root._windowFirst = first
        root._windowLast = last

        var span = last - first + 1
        var lo = Math.max(0, first - span)
        var hi = Math.min(rows.length - 1, last + span)

        var keep = ({})
        var i, key
        for (i = lo; i <= hi; i++) {
            key = rows[i] ? rows[i].artKey : ""
            if (key)
                keep[key] = true
        }
        var map = root.artMap
        var changed = false
        for (key in map) {
            if (!keep[key]) {
                delete map[key]
                changed = true
            }
        }
        for (key in root._artInbox)
            if (!keep[key])
                delete root._artInbox[key]
        if (changed)
            root.artMap = Object.assign({}, map)

        var missing = []
        var seen = ({})
        for (i = first; i <= last; i++) {
            key = rows[i] ? rows[i].artKey : ""
            if (!key || seen[key])
                continue
            seen[key] = true
            if (map[key] !== undefined || root._artInbox[key] !== undefined)
                continue
            missing.push(key)
        }
        if (missing.length > 0)
            QbzLibrary.kioskArtworkWindow(JSON.stringify(missing), Math.ceil((pixels || 64) * Screen.devicePixelRatio))
    }

    /// A tab switch invalidates the band; the incoming body reports its own.
    function resetWindow() {
        root._windowFirst = -1
        root._windowLast = -1
    }

    // =====================================================================
    // Card objects
    // =====================================================================
    // The RESOLVED path is read per cell out of `artMap`, so an arrival
    // re-evaluates the mounted cells and leaves the model alone.
    function albumCard(row) {
        if (!row)
            return ({})
        return {
            "id": row.id, "title": row.title || "", "artist": row.artist || "",
            "qualityTier": row.qualityTier || "",
            "artwork": root.artPathOf(row.artKey)
        }
    }
    /// Artists and labels both carry the display name in `title`.
    function personCard(row) {
        if (!row)
            return ({})
        return {
            "id": row.id, "title": row.title || "", "artist": "",
            "qualityTier": "", "artwork": root.artPathOf(row.artKey)
        }
    }
    /// A playlist's cover is `covers[0]` — the first MEMBER-album cover, which
    /// is what the reference's collage slot 0 holds. `artMap[artKey]` would be
    /// the playlist's OWN graphic, blank for every playlist that has none.
    function playlistCard(row) {
        if (!row)
            return ({})
        return {
            "id": row.id, "title": row.title || "", "artist": "",
            "qualityTier": "",
            "artwork": row.covers && row.covers.length > 0 ? row.covers[0] : ""
        }
    }
    function trackCard(row) {
        if (!row)
            return ({})
        return {
            "id": row.id, "title": row.title || "", "artist": row.artist || "",
            "duration": row.duration || "",
            "artwork": root.artPathOf(row.artKey)
        }
    }
    function cardFor(tab, row) {
        return tab === "albums" ? root.albumCard(row)
            : tab === "playlists" ? root.playlistCard(row)
            : root.personCard(row)
    }

    // =====================================================================
    // Activation
    // =====================================================================
    // Every one of these records its own history entry and publishes the new
    // route from Rust, so the kiosk and Full UI take the same stack.
    function openRow(tab, id) {
        if (!id)
            return
        if (tab === "albums")
            QbzAlbum.openAlbum(id)
        else if (tab === "artists")
            QbzArtist.openArtist(id)
        else if (tab === "playlists")
            QbzBridge.openPlaylist(id)
        else if (tab === "labels")
            QbzHome.openLabel(id)
    }

    /// The clicked row plays the whole VISIBLE list from there — the Qt member
    /// that reproduces the reference's play-track action.
    function playTrack(id) {
        var rows = root.trackRows
        var ids = []
        for (var i = 0; i < rows.length; i++)
            ids.push(rows[i].id)
        QbzLibrary.libraryPlayVisible(JSON.stringify(ids), id)
    }

    // =====================================================================
    // Nav geometry
    // =====================================================================
    property int _navColumns: 1
    function publishNav() {
        QbzKioskNav.publishNav(root.tabIds.length, Math.max(1, root._navColumns),
                               root.tabIds.length + root.activeRows.length, false)
    }
    onActiveRowsChanged: root.publishNav()

    // Enter on a tab entry drives the same switch a tap does; `index < tabs`
    // keeps this handler and the mounted body's disjoint.
    Connections {
        target: QbzKioskNav
        function onActivateSeqChanged() {
            if (!QbzKioskNav.navActive || QbzKioskNav.zone !== "content")
                return
            if (QbzKioskNav.index < 0 || QbzKioskNav.index >= QbzKioskNav.tabs)
                return
            var id = root.tabIds[QbzKioskNav.index]
            if (id !== undefined)
                root.activateTab(id)
        }
    }

    // =====================================================================
    // Tab strip — 64px, whole-cell hit areas
    // =====================================================================
    Rectangle {
        id: tabStrip

        anchors.left: root.left
        anchors.right: root.right
        anchors.top: root.top
        height: 64
        color: theme.surfaceMain

        Flickable {
            id: tabScroll
            anchors.fill: parent
            anchors.leftMargin: root.pad
            anchors.rightMargin: root.pad
            contentWidth: tabsRow.width
            contentHeight: height
            clip: true
            flickableDirection: Flickable.HorizontalFlick
            boundsBehavior: Flickable.StopAtBounds

            Row {
                id: tabsRow
                height: tabScroll.height
                spacing: 6

                // A FIXED five-entry list, not a data collection.
                Repeater {
                    model: root.tabIds

                    delegate: Rectangle {
                        id: tab
                        required property string modelData
                        required property int index

                        readonly property bool active: root.activeTab === tab.modelData
                        readonly property bool navFocused: QbzKioskNav.navActive
                            && QbzKioskNav.zone === "content"
                            && QbzKioskNav.index === tab.index

                        width: Math.max(88, tabText.implicitWidth + 28)
                        height: tabsRow.height
                        color: tab.active
                            ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.10)
                            : "transparent"
                        radius: theme.radiusSm
                        border.width: tab.navFocused ? 2 : 0
                        border.color: theme.accent

                        Text {
                            id: tabText
                            anchors.centerIn: parent
                            text: root.tabLabel(tab.modelData)
                            color: tab.active ? theme.textPrimary : theme.textMuted
                            font.pixelSize: 16
                            font.weight: theme.weightSemibold
                        }

                        Rectangle {
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.leftMargin: 10
                            anchors.rightMargin: 10
                            anchors.bottom: parent.bottom
                            anchors.bottomMargin: 6
                            height: 3
                            radius: 2
                            color: tab.active ? theme.accent : "transparent"
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.activateTab(tab.modelData)
                        }
                    }
                }
            }
        }

        Rectangle {
            anchors.left: tabStrip.left
            anchors.right: tabStrip.right
            y: tabStrip.height - 1
            height: 1
            color: theme.borderSubtle
        }
    }

    // =====================================================================
    // Content — ONE tab mounted at a time
    // =====================================================================
    // `Loader.active`, never `visible: false`: a hidden GridView keeps a
    // viewport and a model and goes on refilling it (the LibraryView finding
    // — half of every mount spent on the body that was not showing).
    Item {
        id: content

        anchors.left: root.left
        anchors.right: root.right
        anchors.top: tabStrip.bottom
        anchors.bottom: root.bottom
        clip: true

        // THE GRID for Albums, Artists, Playlists and Labels.
        //
        // ONE instance for all four: only one is ever mounted, and a tab
        // switch is a model change, which a GridView answers by rebuilding
        // its delegates from the top — which is what a tab switch should do.
        // Four separate instances would only duplicate the pitch algebra.
        Loader {
            anchors.fill: parent
            active: root.ready && root.activeTab !== "tracks"

            sourceComponent: Item {
                GridView {
                    id: feedGrid
                    anchors.fill: parent

                    /// Circular tile + centred text — the artist/label shape.
                    readonly property bool roundCells: root.activeTab === "artists"
                        || root.activeTab === "labels"
                    /// The smallest cell that keeps a card's tap area well past
                    /// the 64px primary target at every supported width: album
                    /// cards land on 4 columns at 800px and 7 at 1280px, the
                    /// round ones on 5 and 9.
                    readonly property real minCell: feedGrid.roundCells ? 132 : 168
                    readonly property real gap: 14

                    readonly property real labelH: 52
                    readonly property real availableRowWidth: Math.max(0, feedGrid.width - 2 * root.pad + feedGrid.gap)
                    readonly property real maxArtHeight: Math.max(64, feedGrid.height - root.pad - feedGrid.labelH)
                    readonly property int columns: feedGrid.width > 0
                        ? Math.min(Math.max(1, Math.floor(feedGrid.availableRowWidth / (64 + feedGrid.gap))),
                                   Math.max(2, Math.floor(feedGrid.availableRowWidth / (feedGrid.minCell + feedGrid.gap)),
                                            feedGrid.height > 0 ? Math.ceil(feedGrid.availableRowWidth / (feedGrid.maxArtHeight + feedGrid.gap)) : 1))
                        : 1
                    // The pitch is derived so the view's own
                    // floor(available / cellWidth) lands on `columns` exactly;
                    // the trailing cell's gap covers part of the right padding.
                    readonly property real rightPad: Math.max(0, root.pad - feedGrid.gap)
                    readonly property real avail:
                        Math.max(0, feedGrid.width - root.pad - feedGrid.rightPad)
                    readonly property real cardW: Math.max(0, feedGrid.cellWidth - feedGrid.gap)

                    model: root.activeRows
                    cellWidth: Math.max(1, Math.floor(feedGrid.avail / feedGrid.columns))
                    cellHeight: Math.max(1, feedGrid.cardW + feedGrid.labelH + feedGrid.gap)

                    leftMargin: root.pad
                    rightMargin: feedGrid.rightPad
                    topMargin: root.pad
                    bottomMargin: root.pad

                    clip: true
                    boundsBehavior: Flickable.StopAtBounds
                    // THE bound on live delegates: the viewport plus one row
                    // above and below. Nothing outside it is instantiated.
                    cacheBuffer: Math.max(0, Math.ceil(feedGrid.cellHeight))
                    reuseItems: true
                    // The ring is QbzKioskNav's; a highlighted current item
                    // would draw a second, contradictory one.
                    highlight: null
                    currentIndex: -1
                    keyNavigationEnabled: false

                    readonly property int focusedItem: QbzKioskNav.index - QbzKioskNav.tabs
                    readonly property bool itemFocused: QbzKioskNav.navActive
                        && QbzKioskNav.zone === "content"
                        && feedGrid.focusedItem >= 0
                        && feedGrid.focusedItem < root.activeRows.length

                    /// Scroll the focused card in by the MINIMUM amount, and do
                    /// nothing when it is already in. Issued from a function on
                    /// a settled layout, never from a binding that feeds layout
                    /// (the "Recursion detected" panic class).
                    function scrollFocusIntoView() {
                        if (feedGrid.itemFocused)
                            feedGrid.positionViewAtIndex(feedGrid.focusedItem, GridView.Contain)
                    }

                    // The mounted band, derived from geometry rather than by
                    // walking delegates: exact for a uniform grid and
                    // independent of when the view happened to create one.
                    function rowAt(y) {
                        return feedGrid.cellHeight > 0
                            ? Math.floor((y - feedGrid.originY) / feedGrid.cellHeight) : 0
                    }
                    function reportBand() {
                        if (!feedGrid.visible || root.activeRows.length === 0) {
                            root.resetWindow()
                            return
                        }
                        var firstRow = Math.max(0, feedGrid.rowAt(feedGrid.contentY) - 1)
                        var lastRow = Math.max(0, feedGrid.rowAt(
                            feedGrid.contentY + feedGrid.height - 1)) + 1
                        root.reportWindow(firstRow * feedGrid.columns,
                                          (lastRow + 1) * feedGrid.columns - 1, feedGrid.cardW)
                    }
                    /// Coalesced: several of these fire on one flick frame, and
                    /// the artwork pass should see one call per row crossed.
                    function reportSoon() { Qt.callLater(feedGrid.reportBand) }

                    onContentYChanged: feedGrid.reportSoon()
                    onHeightChanged: feedGrid.reportSoon()
                    onWidthChanged: feedGrid.reportSoon()
                    onModelChanged: feedGrid.reportSoon()
                    onVisibleChanged: feedGrid.reportSoon()
                    onColumnsChanged: {
                        root._navColumns = feedGrid.columns
                        root.publishNav()
                        feedGrid.reportSoon()
                    }
                    Component.onCompleted: {
                        root._navColumns = feedGrid.columns
                        root.publishNav()
                        feedGrid.reportSoon()
                    }

                    Connections {
                        target: QbzKioskNav
                        function onIndexChanged() {
                            Qt.callLater(feedGrid.scrollFocusIntoView)
                        }
                        function onActivateSeqChanged() {
                            if (feedGrid.itemFocused)
                                root.openRow(root.activeTab,
                                             root.activeRows[feedGrid.focusedItem].id)
                        }
                    }

                    delegate: Item {
                        id: cell
                        required property var modelData
                        required property int index

                        width: feedGrid.cellWidth
                        height: feedGrid.cellHeight

                        KioskCard {
                            artSize: feedGrid.cardW
                            round: feedGrid.roundCells
                            // The resolved cover is read PER CELL out of
                            // `artMap`, so an arrival re-evaluates a handful of
                            // strings instead of replacing the model.
                            album: root.cardFor(root.activeTab, cell.modelData)
                            navFocused: feedGrid.itemFocused
                                && feedGrid.focusedItem === cell.index
                            onClicked: function (id) { root.openRow(root.activeTab, id) }
                        }
                    }
                }

                // The scope carries the TAB, so returning to a different one
                // cannot pour this offset into it.
                ScrollMemory { target: feedGrid; scope: "library:" + root.activeTab }
            }
        }

        // THE TRACK LIST.
        Loader {
            anchors.fill: parent
            active: root.ready && root.activeTab === "tracks"

            sourceComponent: Item {
                ListView {
                    id: tracksList
                    anchors.fill: parent
                    anchors.leftMargin: 8
                    anchors.rightMargin: 8
                    clip: true
                    topMargin: 8
                    bottomMargin: 8
                    spacing: 0
                    // One extra screenful of runway.
                    cacheBuffer: Math.min(64, Math.max(0, height / 2))
                    boundsBehavior: Flickable.StopAtBounds
                    reuseItems: true
                    model: root.trackRows

                    readonly property int focusedItem: QbzKioskNav.index - QbzKioskNav.tabs
                    readonly property bool itemFocused: QbzKioskNav.navActive
                        && QbzKioskNav.zone === "content"
                        && tracksList.focusedItem >= 0
                        && tracksList.focusedItem < root.trackRows.length

                    function scrollFocusIntoView() {
                        if (tracksList.itemFocused)
                            tracksList.positionViewAtIndex(tracksList.focusedItem,
                                                           ListView.Contain)
                    }
                    function reportBand() {
                        if (!tracksList.visible || root.trackRows.length === 0) {
                            root.resetWindow()
                            return
                        }
                        var first = tracksList.indexAt(4, tracksList.contentY + 1)
                        var last = tracksList.indexAt(
                            4, tracksList.contentY + Math.max(1, tracksList.height) - 1)
                        if (first < 0)
                            first = Math.max(0, Math.floor(
                                (tracksList.contentY - tracksList.originY) / 64))
                        if (last < 0)
                            last = first + 12
                        root.reportWindow(Math.max(0, first - 1), last + 1, 48)
                    }
                    function reportSoon() { Qt.callLater(tracksList.reportBand) }

                    onContentYChanged: tracksList.reportSoon()
                    onHeightChanged: tracksList.reportSoon()
                    onModelChanged: tracksList.reportSoon()
                    onVisibleChanged: tracksList.reportSoon()
                    Component.onCompleted: {
                        root._navColumns = 1
                        root.publishNav()
                        tracksList.reportSoon()
                    }

                    Connections {
                        target: QbzKioskNav
                        function onIndexChanged() {
                            Qt.callLater(tracksList.scrollFocusIntoView)
                        }
                        function onActivateSeqChanged() {
                            if (tracksList.itemFocused)
                                root.playTrack(root.trackRows[tracksList.focusedItem].id)
                        }
                    }

                    delegate: KioskTrackRow {
                        id: trackDelegate
                        required property var modelData
                        required property int index

                        width: tracksList.width
                        // 64px is the contract's primary touch target; the
                        // shared primitive's desktop-era 62 is overridden here
                        // rather than edited.
                        height: 64
                        track: root.trackCard(trackDelegate.modelData)
                        navFocused: tracksList.itemFocused
                            && tracksList.focusedItem === trackDelegate.index
                        onClicked: root.playTrack(trackDelegate.modelData.id)
                    }
                }

                ScrollMemory { target: tracksList; scope: "library:tracks" }
            }
        }


        // Loading / error / empty. Static, mounts no artwork and no model, and
        // unmounts entirely once the tab is populated.
        KioskSkeleton {
            anchors.fill: parent
            kind: root.activeTab === "tracks" ? "list" : "grid"
            columns: Math.max(2, root._navColumns)
            pad: root.pad
            gap: 14
            rowHeight: 64
            rowArtSize: 46
            roundCells: root.activeTab === "artists" || root.activeTab === "labels"
            loading: QbzLibrary.libraryLoading
            error: QbzLibrary.libraryError
            empty: root.ready && root.activeRows.length === 0
            emptyText: root.emptyTextFor(root.activeTab)
        }

        // Retry. The kiosk primitive draws the message and no action, and a
        // route that can only say "it failed" is a route the user has to
        // leave. 64px, over the message.
        Rectangle {
            id: retry
            visible: QbzLibrary.libraryError !== "" && !QbzLibrary.libraryLoading
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.verticalCenter: parent.verticalCenter
            anchors.verticalCenterOffset: 56
            width: Math.max(140, retryText.implicitWidth + 44)
            height: 64
            radius: theme.radiusSm
            color: theme.surfaceElevated
            border.width: 1
            border.color: theme.borderSubtle

            Text {
                id: retryText
                anchors.centerIn: parent
                text: root.t("Retry")
                color: theme.textPrimary
                font.pixelSize: 16
                font.weight: theme.weightSemibold
            }

            MouseArea {
                anchors.fill: parent
                onClicked: QbzLibrary.reloadLibrary()
            }
        }
    }
}
