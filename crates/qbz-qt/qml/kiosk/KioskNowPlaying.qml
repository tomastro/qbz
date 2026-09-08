// KioskNowPlaying — the kiosk Now Playing centerpiece (QML port of
// crates/qbz-ui/ui/shell/KioskNowPlaying.slint, 474 lines).
//
// A full-screen 58/42 split with a 1px divider: the current track on the LEFT
// (a Cover<->Lyrics toggle over the big art, then title / artist link / seek /
// the 7-control transport with the quality stamp floating at its right end)
// and the queue on the RIGHT (Up Next / History tabs over a windowed row
// list). It reads the same bridges the desktop surfaces read — QbzPlayer,
// QbzQueue, QbzLyrics — nothing is mirrored.
//
// THE 58 IS A REMAINDER, NOT A CONSTANT (KioskNowPlaying.slint:370): the right
// panel is 42% of (width - 1) and the left takes what is left, so the two can
// never disagree by a rounding pixel and leave a seam over the divider.
//
// THREE THINGS THE SHELL DELEGATES TO THIS VIEW:
//   1. The PLAYER-ZONE RING is drawn HERE, around the in-page transport
//      (:333-338). KioskShell hides both the small transport bar and its own
//      player ring on this route (KioskShell.qml:452,497), so without this the
//      player zone would be invisible while focused.
//   2. The queue-panel refresh (:170) — the paginated slice is only rebuilt on
//      demand.
//   3. The lyrics-follow gate (:173,202,212). In Slint that flag is
//      `ShellState.kiosk-lyrics-follow`, read by the Rust sync engine; in Qt
//      the engine IS QML (shell/LyricsSyncEngine.qml) and its `gateOpen`
//      property is the same boolean, so the flag is the binding below rather
//      than a bridge member (contract §8.5 / trap 2). Because `gateOpen`
//      includes `showLyrics`, mounting in Cover mode resets it structurally —
//      the explicit assignment in Component.onCompleted is the belt to that
//      brace, for a Loader that keeps this item alive across remounts.
//
// NAV: the model is the RIGHT panel only — 2 queue tabs followed by the active
// tab's rows as a 1-column list (:135-143). The left panel (the Cover/Lyrics
// pair, the artist link, the seek rail) is TOUCH-ONLY and carries no ring
// (:132-134); the transport is the separate "player" zone.
//
// Slint's `viewport-y` is negative as you scroll down; Qt's `contentY` is its
// positive twin — every occurrence is already translated, and both scroll
// writes run through Qt.callLater so they land on a settled layout.
//
// The seek bar is INLINED rather than extracted to a shared SeekBar.qml: the
// port has no such component (the desktop bars inline their own) and adding
// one is a new file, which is a build.rs change this block does not own. The
// geometry below is SeekBar.slint's `show-times: true` arm, number for number.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../shell"
import "../theme"

Rectangle {
    id: root

    // KioskNowPlaying.slint:120 sets no background — the shell's content panel
    // paints the surface behind the left half, the right panel paints its own.
    color: "transparent"

    QbzTheme { id: theme }

    // ---- View-local state ------------------------------------------------
    // The left panel's Cover(false) / Lyrics(true) mode (:127).
    property bool showLyrics: false
    // Up Next (0) / History (1). Slint reads QueueState.tab, an app-global;
    // the Qt queue bridge has no tab field and the desktop panel keeps its own
    // (shell/QueuePanel.qml:32), so this view does the same. Leaving the view
    // therefore resets it to Up Next, which is the desktop panel's behaviour.
    property int queueTab: 0
    function selectQueueTab(tab) {
        if (tab === queueTab || tab < 0 || tab > 1) return
        navigation.recordTab(String(tab))
        queueTab = tab
    }
    KioskNavigation {
        id: navigation
        route: "nowplaying"
        snapshot: ({ activeTab: String(root.queueTab), showLyrics: root.showLyrics, leftScroll: leftPanel.contentY, page: root.doc.page || 0 })
        onRestore: function(saved) {
            root.queueTab = Number(saved.activeTab) === 1 ? 1 : 0
            root.showLyrics = !!saved.showLyrics
            Qt.callLater(function() { leftPanel.contentY = saved.leftScroll || 0 })
            if (saved.page !== undefined && saved.page !== root.doc.page) QbzQueue.queueSetPage(saved.page)
        }
    }

    // ---- Queue document --------------------------------------------------
    readonly property var doc: root.parseQueue()
    function parseQueue() {
        try {
            return JSON.parse(QbzQueue.queueJson)
        } catch (e) {
            return ({})
        }
    }
    // `upcoming` is ALREADY the paginated page slice (queue_qt.rs:471), which
    // is why mounting the whole list here is cheap.
    readonly property var upcoming: root.doc.upcoming || []
    readonly property var historyRows: root.doc.history || []
    readonly property var activeRows: root.queueTab === 0 ? root.upcoming : root.historyRows

    // ---- Lyrics document -------------------------------------------------
    readonly property var lyricsDoc: root.parseLyrics()
    function parseLyrics() {
        try {
            return JSON.parse(QbzLyrics.docJson)
        } catch (e) {
            return ({})
        }
    }
    readonly property var lyricsLines: root.lyricsDoc.lines ? root.lyricsDoc.lines : []
    readonly property bool lyricsSynced: root.lyricsDoc.synced === true
    readonly property int lyricsStatus: root.lyricsDoc.status !== undefined
        ? root.lyricsDoc.status : 0
    // The mount gate (:248). Nothing else may mount the karaoke view: the
    // reference records a Slint relayout panic behind that rule, and while the
    // Qt engine has no such footgun the GATE IS THE CONTRACT — same condition,
    // same three fallbacks.
    readonly property bool lyricsReady: root.lyricsStatus === 2 && root.lyricsLines.length > 0

    // The three fallback strings (:266-276), in the source's evaluation order.
    // Note what is deliberately NOT here, and must not be added: status 4
    // (error) shows the plain "No lyrics available" rather than the backend
    // error string, status 2 with zero lines shows "Fetching lyrics...", and
    // there is no LRCLIB contribution link and no spinner. The desktop
    // shell/LyricsPanel.qml does all four differently on purpose.
    readonly property string lyricsFallback: root.lyricsStatus === 5
        ? QbzSession.tr("Lyrics unavailable offline", QbzSession.trRev)
        : (root.lyricsStatus === 3 || root.lyricsStatus === 4)
            ? QbzSession.tr("No lyrics available", QbzSession.trRev)
            : QbzSession.tr("Fetching lyrics...", QbzSession.trRev)

    // ---- Nav geometry (:135-143) ----------------------------------------
    // 2 leading tab entries + the ACTIVE tab's rows, one column. The clamp of
    // `index` into the new count lives in the bridge (kiosk_nav_qt).
    function publishNav() {
        QbzKioskNav.publishNav(2, 1,
            2 + (root.queueTab === 0 ? root.upcoming.length : root.historyRows.length),
            false)
    }
    // :148 — one probe over BOTH lengths, so a republish of either list
    // refreshes the count even while the other tab is showing.
    readonly property int navLenProbe: root.upcoming.length + root.historyRows.length
    onNavLenProbeChanged: root.publishNav()
    onQueueTabChanged: {
        root.publishNav()
        // Slint mounts a FRESH Flickable per tab (:404,:438), so each switch
        // starts at the top. One instance per tab lives for the whole view
        // here, so the reset is explicit.
        upFlick.contentY = 0
        histFlick.contentY = 0
    }

    readonly property int focusedItem: QbzKioskNav.index - QbzKioskNav.tabs
    readonly property bool itemFocused: QbzKioskNav.navActive
        && QbzKioskNav.zone === "content"
        && root.focusedItem >= 0
        && root.focusedItem < root.activeRows.length

    // Scroll the focused row into view (:416-425 / :448-457). 60px pitch =
    // the 58px row + the 2px column spacing; ±8px of air at either edge.
    function scrollFocusIntoView() {
        if (root.itemFocused)
            (root.queueTab === 0 ? upFlick : histFlick).positionViewAtIndex(root.focusedItem, ListView.Contain)
    }

    Connections {
        target: QbzKioskNav

        function onIndexChanged() {
            Qt.callLater(root.scrollFocusIntoView)
        }
        function onZoneChanged() {
            if (QbzKioskNav.navActive && QbzKioskNav.zone === "player")
                leftPanel.contentY = Math.max(0, leftPanel.contentHeight - leftPanel.height)
        }

        // Enter pulse (:154-167): on a tab it switches tabs, on a row it plays
        // that row. The `- 2` is the source's literal, matching `tabs = 2`.
        function onActivateSeqChanged() {
            if (!QbzKioskNav.navActive || QbzKioskNav.zone !== "content")
                return
            if (QbzKioskNav.index < QbzKioskNav.tabs) {
                root.selectQueueTab(QbzKioskNav.index)
                return
            }
            var i = QbzKioskNav.index - 2
            if (root.queueTab === 0 && i < root.upcoming.length)
                QbzQueue.queuePlayUpcoming(i)
            else if (root.queueTab === 1 && i < root.historyRows.length)
                QbzQueue.queuePlayHistory(i)
        }
    }

    // ---- Mount side effects (:169-175) -----------------------------------
    Component.onCompleted: {
        QbzQueue.queuePanelOpened()
        root.publishNav()
    }

    // ---- Lyrics sync ------------------------------------------------------
    // THIS BINDING IS `kiosk-lyrics-follow`. Slint writes the flag true when
    // the Lyrics tab is picked (:212) and false on the Cover tab (:202) and on
    // mount (:173); the same three transitions fall out of `showLyrics` here.
    LyricsSyncEngine {
        id: kioskSync
        gateOpen: root.showLyrics && root.lyricsReady && root.lyricsSynced
    }

    // Every doc commit re-lands the ladder on the right line even while paused
    // (shell/LyricsPanel.qml:60-66).
    Connections {
        target: QbzLyrics
        function onDocJsonChanged() {
            kioskSync.reset()
            kioskSync.kick()
        }
    }

    Component {
        id: lyricsViewComponent

        LyricsLinesView {
            lines: root.lyricsLines
            synced: root.lyricsSynced
            // Owner feedback 2026-09-07: the kiosk Now Playing lyrics are centered.
            centered: true
            // Slint's row reads LyricsState.show-translation itself
            // (LyricsLinesView.slint:271); the Qt view takes it from the host.
            showTranslation: QbzLyrics.showTranslation
            sync: kioskSync
            sidePadding: 8
            lineGap: 13
            sizeInactive: 19
            sizeActive: 23
            dimmingMode: QbzLyrics.dimmingMode
            // Hardcoded true at :256 — the kiosk surface always follows,
            // whatever the display pref says.
            autoFollow: true
            uppercase: QbzLyrics.uppercase
            // The Qt view takes the INDEX and maps it to a family itself
            // (LyricsLinesView.qml:66-69), which is the same map as :259-263.
            fontIndex: QbzLyrics.fontIndex
            // Hardcoded accent at :264 — not the custom lyrics swatch.
            activeColor: theme.accent
            // Slint computes `reduce-motion || lite-fill` inside the row
            // (LyricsLinesView.slint:232); kiosk forces reduce-motion.
            liteFill: QbzLyrics.liteFill || QbzShell.reduceMotion
        }
    }

    // ---- Track Info + "Add to…" hosts ------------------------------------
    // TransportControls owns neither surface (it emits trackInfoRequested /
    // addRequested and the HOST mounts them, shell/TransportControls.qml:29,31),
    // so the two hosts are replicated from shell/PlayerBar.qml. Without them
    // the (i) and "+" buttons would be inert affordances.
    function openTrackInfo() {
        if (!QbzPlayer.npHasTrack)
            return
        var id = QbzPlayer.npTrackId
        if (!/^[0-9]+$/.test(id))
            return
        trackInfo.openFor(id)
    }

    TrackInfoModal { id: trackInfo }

    // Favourite state of the now-playing track: the queue document's `current`
    // row, with a one-slot override so a heart flipped on ANY other surface
    // reaches this transport (PlayerBar.qml:128-160).
    property string favOverrideId: ""
    property bool favOverrideValue: false
    readonly property bool npFavorite: {
        if (root.favOverrideId !== "" && root.favOverrideId === QbzPlayer.npTrackId)
            return root.favOverrideValue
        return !!(root.doc.current && root.doc.current.isFavorite)
    }
    Connections {
        target: QbzLibrary
        function onLibraryFavoriteChanged(key, value) {
            var id = QbzPlayer.npTrackId
            if (id !== "" && key === "track:" + id) {
                root.favOverrideId = id
                root.favOverrideValue = value
            }
        }
    }

    // The provenance word the MyQBZ payload needs ("qobuz" | "" = unknown).
    // The id guard keeps a stale queue document from lending the previous
    // track's provenance to this one (PlayerBar.qml:162-194).
    readonly property string npSource: {
        var cur = root.doc.current
        if (cur && cur.id === QbzPlayer.npTrackId)
            return cur.isLocal === true ? "" : "qobuz"
        return ""
    }

    CardMenu {
        id: addMenu
        menuWidth: 232
        entries: {
            var m = [
                {
                    "label": root.npFavorite
                        ? QbzSession.tr("Remove from library", QbzSession.trRev)
                        : QbzSession.tr("Add to library", QbzSession.trRev),
                    "icon": root.npFavorite ? "heart-filled" : "heart",
                    "action": "favorite"
                },
                { "label": QbzSession.tr("Add to queue", QbzSession.trRev), "icon": "list-end", "action": "queue" },
                { "label": QbzSession.tr("Play later", QbzSession.trRev), "icon": "list-plus", "action": "later" },
                { "label": QbzSession.tr("Play next", QbzSession.trRev), "icon": "list-start", "action": "next" },
            ]
            // "Add to playlist" sits SECOND (TransportControls.slint:143), and
            // rides the npSource gate: unknown provenance -> the entry is
            // ABSENT, never rendered-and-inert.
            if (root.npSource === "qobuz") {
                m.splice(1, 0, {
                    "label": QbzSession.tr("Add to playlist", QbzSession.trRev),
                    "icon": "list-music",
                    "action": "playlist"
                })
            }
            if (root.npSource !== "") {
                m.push({
                    "label": QbzSession.tr("Add to mixtape", QbzSession.trRev),
                    "icon": "cassette-tape",
                    "action": "mixtape"
                })
            }
            if (QbzPlayer.npAlbumId !== "") {
                m.push({
                    "label": QbzSession.tr("Add album to collection", QbzSession.trRev),
                    "icon": "disc-3",
                    "action": "album-favorite"
                })
            }
            return m
        }
        onPicked: function (a) {
            var id = QbzPlayer.npTrackId
            if (a === "favorite") {
                if (id !== "") QbzQueue.queueToggleFavorite("track", id)
            } else if (a === "queue" || a === "later" || a === "next") {
                if (id !== "") QbzPlayer.enqueueTrack(id, a)
            } else if (a === "playlist") {
                if (id !== "") QbzPlaylistPicker.openForTrack(id)
            } else if (a === "album-favorite") {
                if (QbzPlayer.npAlbumId !== "")
                    QbzLibrary.libraryToggleFavorite("album", QbzPlayer.npAlbumId)
            } else if (a === "mixtape") {
                // `npArtworkPath` is a file:// CACHE path, so it is NOT the
                // artworkUrl — the store would keep a dead local path.
                if (id !== "" && root.npSource !== "")
                    QbzMyQbzAdd.open(JSON.stringify([{
                        "itemType": "track", "source": root.npSource,
                        "sourceItemId": id, "title": QbzPlayer.npTitle,
                        // artworkUrl STAYS EMPTY here, deliberately: the player bridge exposes only np_artwork_path.
                        // A file:// cache path must NOT be stored — the collection's
                        // artwork_url is a snapshot other machines read — so this needs a
                        // remote-url field on the document first. The five sister sites
                        // that HAD one were stamped 2026-08-22.
                        "subtitle": QbzPlayer.npArtist, "artworkUrl": "",
                        "year": null, "trackCount": null
                    }]))
            }
        }
    }

    function fmt(secs) {
        var m = Math.floor(secs / 60)
        var s = Math.floor(secs % 60)
        return m + ":" + (s < 10 ? "0" : "") + s
    }
    function clamp01(v) {
        return Math.min(Math.max(v, 0), 1)
    }

    // =====================================================================
    // RIGHT — the queue (:369-472). Declared first so the divider and the
    // left panel can anchor off it: the right width is the fixed term and
    // the left is the remainder (:370).
    // =====================================================================
    Rectangle {
        id: rightPanel
        anchors.right: root.right
        anchors.top: root.top
        anchors.bottom: root.bottom
        width: Math.round((root.width - 1) * 0.42)
        color: theme.surfaceCard

        // Tabs row (:375-395): 30px tall, 20px apart, left-aligned.
        Row {
            id: queueTabsRow
            x: 18
            y: 18
            height: 44
            spacing: 20

            QueueTab {
                height: queueTabsRow.height
                label: QbzSession.tr("Up Next", QbzSession.trRev)
                active: root.queueTab === 0
                navFocused: QbzKioskNav.navActive
                    && QbzKioskNav.zone === "content"
                    && QbzKioskNav.index === 0
                onPicked: root.selectQueueTab(0)
            }
            QueueTab {
                height: queueTabsRow.height
                label: QbzSession.tr("History", QbzSession.trRev)
                active: root.queueTab === 1
                navFocused: QbzKioskNav.navActive
                    && QbzKioskNav.zone === "content"
                    && QbzKioskNav.index === 1
                onPicked: root.selectQueueTab(1)
            }
        }

        // Body (:402-470): a Flickable whose content height IS the column's,
        // so short lists top-anchor. (The reference's note: a ListView at
        // height 100% bottom-anchors a short list.)
        Item {
            id: queueBody
            x: 18
            y: queueTabsRow.y + queueTabsRow.height + 12
            width: Math.max(0, rightPanel.width - 36)
            height: Math.max(0, rightPanel.height - 18 - queueBody.y - (pager.visible ? 52 : 0))

            ListView {
                id: upFlick
                anchors.fill: queueBody
                visible: root.queueTab === 0
                model: visible ? root.upcoming : []
                clip: true; cacheBuffer: 0; reuseItems: true; spacing: 2
                boundsBehavior: Flickable.StopAtBounds
                delegate: QueueRowLite {
                    id: queueRow
                    required property var modelData
                    required property int index
                    width: upFlick.width
                    item: modelData
                    KioskCoverSource { id: cover; remote: queueRow.modelData.artUrl || ""; edge: 44 }
                    artPath: cover.source
                    navFocused: root.itemFocused && root.focusedItem === index
                    onPlay: QbzQueue.queuePlayUpcoming(index)
                }
            }

            ListView {
                id: histFlick
                anchors.fill: queueBody
                visible: root.queueTab === 1
                model: visible ? root.historyRows : []
                clip: true; cacheBuffer: 0; reuseItems: true; spacing: 2
                boundsBehavior: Flickable.StopAtBounds
                delegate: QueueRowLite {
                    // id must differ from the upcoming delegate's `queueRow`:
                    // two delegates sharing an id makes qmllint fail to resolve
                    // the inline QueueRowLite type in the SECOND one (the
                    // 2026-09-07 pre-release qmllint gate failure; qmlcachegen
                    // and the runtime tolerate the collision, qmllint does not).
                    id: queueRowHist
                    required property var modelData
                    required property int index
                    width: histFlick.width
                    item: modelData
                    KioskCoverSource { id: cover; remote: queueRowHist.modelData.artUrl || ""; edge: 44 }
                    artPath: cover.source
                    navFocused: root.itemFocused && root.focusedItem === index
                    onPlay: QbzQueue.queuePlayHistory(index)
                }
            }
        }
    }

    ScrollMemory { target: upFlick; scope: "nowplaying:0" }
    ScrollMemory { target: histFlick; scope: "nowplaying:1" }
    KioskSkeleton { parent: queueBody; anchors.fill: parent; kind: "list"; empty: root.activeRows.length === 0 }
    Row {
        id: pager
        parent: rightPanel
        anchors.bottom: parent.bottom; anchors.bottomMargin: 8; anchors.horizontalCenter: parent.horizontalCenter
        visible: root.queueTab === 0 && (root.doc.pageCount || 0) > 1
        height: 44; spacing: 12
        Repeater {
            model: ["Previous", "Next"]
            delegate: Rectangle {
                required property string modelData
                required property int index
                width: Math.max(64, (rightPanel.width - 48) / 2); height: 44; radius: theme.radiusSm
                enabled: index === 0 ? (root.doc.page || 0) > 0 : (root.doc.page || 0) + 1 < root.doc.pageCount
                opacity: enabled ? 1 : 0.4; color: theme.surfaceElevated
                Text { anchors.centerIn: parent; text: QbzSession.tr(modelData, QbzSession.trRev); color: theme.textPrimary; font.pixelSize: 14 }
                MouseArea { anchors.fill: parent; onClicked: QbzQueue.queueSetPage((root.doc.page || 0) + (index === 0 ? -1 : 1)) }
            }
        }
    }

    // The 1px rule between the panels (:362-365).
    Rectangle {
        id: divider
        anchors.right: rightPanel.left
        anchors.top: root.top
        anchors.bottom: root.bottom
        width: 1
        color: theme.borderSubtle
    }

    // =====================================================================
    // LEFT — the current track (:181-360). 18px padding, 12px spacing.
    // =====================================================================
    Flickable {
        id: leftPanel
        contentWidth: width
        contentHeight: Math.max(height, bottomBlock.height + toggleRow.height + 112)
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        anchors.left: root.left
        anchors.right: divider.left
        anchors.top: root.top
        anchors.bottom: root.bottom

        // Cover / Lyrics toggle (:192-215), pinned to the TOP so it lines up
        // with the Up Next / History tabs across the divider. TOUCH-ONLY:
        // neither tab sets navFocused (:82-84).
        Row {
            id: toggleRow
            x: 18
            y: 18
            spacing: 18

            QueueTab {
                label: QbzSession.tr("Cover", QbzSession.trRev)
                active: !root.showLyrics
                onPicked: root.showLyrics = false
            }
            QueueTab {
                label: QbzSession.tr("Lyrics", QbzSession.trRev)
                active: root.showLyrics
                onPicked: root.showLyrics = true
            }
        }

        // The flexible art / lyrics area (:221-278) — it takes ALL the slack
        // between the pinned toggle above and the pinned transport below, and
        // CLIPS, so on a short window (1080x600) the cover shrinks instead of
        // pushing the controls off-screen.
        Item {
            id: artArea
            x: 18
            y: toggleRow.y + toggleRow.height + 12
            width: Math.max(0, leftPanel.width - 36)
            height: Math.max(0, bottomBlock.y - 12 - artArea.y)
            clip: true

            // CONTAIN, not crop (:233): the cover is as large as fits and
            // scales with the window without reading parent geometry.
            Loader {
                anchors.fill: artArea
                active: !root.showLyrics
                sourceComponent: KioskArtwork {
                    source: QbzPlayer.npArtworkPath
                    radius: 0
                    fit: "contain"
                }
            }

            Item {
                id: lyricsArm
                anchors.fill: artArea
                visible: root.showLyrics

                Loader {
                    anchors.fill: lyricsArm
                    active: root.showLyrics && root.lyricsReady
                    sourceComponent: lyricsViewComponent
                }

                Text {
                    anchors.fill: lyricsArm
                    visible: !root.lyricsReady
                    text: root.lyricsFallback
                    color: theme.textMuted
                    font.pixelSize: 14
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
            }
        }

        // Pinned metadata + transport (:284-358): natural height at the
        // bottom, so ALL the slack goes to the art area above.
        Column {
            id: bottomBlock
            x: 18
            y: leftPanel.contentHeight - 18 - bottomBlock.height
            width: Math.max(0, leftPanel.width - 36)
            spacing: 8

            Text {
                width: bottomBlock.width
                text: QbzPlayer.npTitle
                color: theme.textPrimary
                font.pixelSize: 20
                font.weight: theme.weightBold
                horizontalAlignment: Text.AlignHCenter
                elide: Text.ElideRight
                wrapMode: Text.NoWrap
                maximumLineCount: 1
            }

            // The WHOLE 18px row is the artist link (:297-313). Touch-only.
            Item {
                id: artistRow
                width: bottomBlock.width
                height: 44

                Text {
                    id: artistLabel
                    anchors.centerIn: artistRow
                    text: QbzPlayer.npArtist
                    color: theme.textSecondary
                    font.pixelSize: 14
                    verticalAlignment: Text.AlignVCenter
                }

                MouseArea {
                    anchors.fill: artistRow
                    cursorShape: Qt.PointingHandCursor
                    onClicked: QbzArtist.openArtist(QbzPlayer.npArtistId)
                }
            }

            // ---- Seek bar (SeekBar.slint, show-times arm) ----------------
            Item {
                id: seekRow
                width: bottomBlock.width
                height: 20

                Text {
                    id: elapsedLabel
                    anchors.left: seekRow.left
                    anchors.verticalCenter: seekRow.verticalCenter
                    width: 44
                    horizontalAlignment: Text.AlignRight
                    text: root.fmt(QbzPlayer.npElapsedSecs)
                    color: theme.textMuted
                    font.pixelSize: 11
                }

                Text {
                    id: remainingLabel
                    anchors.right: seekRow.right
                    anchors.verticalCenter: seekRow.verticalCenter
                    width: 44
                    horizontalAlignment: Text.AlignLeft
                    text: "-" + root.fmt(Math.max(0, QbzPlayer.npDurationSecs - QbzPlayer.npElapsedSecs))
                    color: theme.textMuted
                    font.pixelSize: 11
                }

                Rectangle {
                    id: seekTrack
                    anchors.left: elapsedLabel.right
                    anchors.leftMargin: 12
                    anchors.right: remainingLabel.left
                    anchors.rightMargin: 12
                    anchors.verticalCenter: seekRow.verticalCenter
                    height: 3
                    radius: 2
                    color: theme.surfaceElevated

                    // Buffered / cache line — text-muted @0.35, NOT
                    // border-muted (an alpha-over-surface token, which inverts
                    // against the elevated rail on light themes).
                    Rectangle {
                        width: seekTrack.width * root.clamp01(QbzPlayer.npCacheProgress)
                        height: seekTrack.height
                        radius: 2
                        color: theme.textMuted
                        opacity: 0.35
                    }
                    Rectangle {
                        width: seekTrack.width * root.clamp01(QbzPlayer.npProgress)
                        height: seekTrack.height
                        radius: 2
                        color: theme.accent
                    }
                    // Hover thumb.
                    Rectangle {
                        width: 12
                        height: 12
                        radius: 6
                        color: theme.textPrimary
                        anchors.verticalCenter: seekTrack.verticalCenter
                        x: seekTrack.width * root.clamp01(QbzPlayer.npProgress) - width / 2
                        opacity: seekArea.containsMouse ? 1.0 : 0.0
                        Behavior on opacity {
                            NumberAnimation { duration: 100 }
                        }
                    }

                    // Tall, easy-to-grab hit area over the thin rail. The seek
                    // target is LOCKED to what has downloaded while streaming,
                    // and the cursor turns not-allowed past it (:93-99).
                    MouseArea {
                        id: seekArea
                        anchors.left: seekTrack.left
                        anchors.right: seekTrack.right
                        anchors.verticalCenter: seekTrack.verticalCenter
                        height: 44
                        hoverEnabled: true
                        cursorShape: root.clamp01(seekArea.mouseX / seekArea.width) > QbzPlayer.npSeekableMax
                            ? Qt.ForbiddenCursor
                            : Qt.PointingHandCursor
                        onPressed: QbzPlayer.seek(Math.min(
                            root.clamp01(seekArea.mouseX / seekArea.width),
                            QbzPlayer.npSeekableMax))
                        onPositionChanged: {
                            if (seekArea.pressed)
                                QbzPlayer.seek(Math.min(
                                    root.clamp01(seekArea.mouseX / seekArea.width),
                                    QbzPlayer.npSeekableMax))
                        }
                    }

                    // Hover time bubble with a downward caret (:103-140).
                    Item {
                        id: seekTip
                        readonly property int atSecs: Math.round(
                            root.clamp01(seekArea.mouseX / seekTrack.width) * QbzPlayer.npDurationSecs)
                        width: tipLabel.implicitWidth + 18
                        height: 24
                        x: Math.min(Math.max(seekArea.mouseX - seekTip.width / 2, 0),
                                    Math.max(0, seekTrack.width - seekTip.width))
                        y: -seekTip.height - 11
                        opacity: seekArea.containsMouse ? 1.0 : 0.0

                        // Shadow approximation — one offset rect, the port's
                        // stand-in for Slint's drop-shadow-blur (the same
                        // choice controls/QbzTooltip.qml documents).
                        Rectangle {
                            x: 0
                            y: 2
                            width: seekTip.width
                            height: seekTip.height
                            radius: 4
                            color: "#66000000"
                        }
                        Rectangle {
                            id: tipBubble
                            anchors.fill: seekTip
                            radius: 4
                            color: theme.surfaceElevated

                            Text {
                                id: tipLabel
                                anchors.centerIn: tipBubble
                                text: root.fmt(seekTip.atSecs)
                                color: theme.textPrimary
                                font.pixelSize: 11
                                font.weight: theme.weightMedium
                            }
                        }

                        // Canvas is this port's polygon primitive
                        // (Canvas.Immediate — the other render strategies
                        // segfault against list recycling).
                        Canvas {
                            id: tipCaret
                            width: 10
                            height: 6
                            x: Math.round((seekTip.width - tipCaret.width) / 2)
                            y: seekTip.height - 1
                            renderTarget: Canvas.Image
                            renderStrategy: Canvas.Immediate
                            // A Canvas does not repaint on a colour binding.
                            property color fill: theme.surfaceElevated
                            onFillChanged: tipCaret.requestPaint()
                            onPaint: {
                                var ctx = tipCaret.getContext("2d")
                                if (!ctx)
                                    return
                                ctx.reset()
                                ctx.clearRect(0, 0, tipCaret.width, tipCaret.height)
                                ctx.fillStyle = tipCaret.fill
                                ctx.beginPath()
                                ctx.moveTo(0, 0)
                                ctx.lineTo(tipCaret.width / 2, tipCaret.height)
                                ctx.lineTo(tipCaret.width, 0)
                                ctx.closePath()
                                ctx.fill()
                            }
                        }
                    }
                }
            }

            // ---- Transport + quality stamp (:331-357) -------------------
            // The stamp floats at the right of the SAME row instead of owning
            // a line, which is what keeps this block short on a 600px window.
            Rectangle {
                id: transportRow
                width: bottomBlock.width
                height: transport.height + 24
                radius: theme.radiusSm
                // THE IN-PAGE PLAYER-ZONE RING (:333-338). 3px + an 0.08 tint,
                // lighter than the 0.12 card ring. KioskShell hides both the
                // small bar and its own player ring on this route, so this is
                // the only thing that shows the player zone has the focus.
                border.width: QbzKioskNav.navActive && QbzKioskNav.zone === "player" ? 3 : 0
                border.color: theme.accent
                color: QbzKioskNav.navActive && QbzKioskNav.zone === "player"
                    ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.08)
                    : "transparent"

                // ONE row, the desktop New bar's arrangement (shell/
                // TransportControls.qml): the three transport buttons in the
                // middle, the modifiers flanking them — info and shuffle to
                // the left, repeat, add and favorite to the right. Every
                // button is CENTRED on the row's axis: the previous Flow
                // top-aligned a mix of 44px and 64px buttons, which is the
                // misalignment reported on 2026-09-07. The cluster sits on
                // the panel's centre and gives way to the quality stamp only
                // when the panel is too narrow for both.
                Row {
                    id: transport
                    x: Math.max(4, Math.min((transportRow.width - width) / 2,
                                            transportRow.width - width - qualityStamp.width - 12))
                    y: 4
                    height: 64
                    spacing: 2
                    QbzIconButton { name: "info"; btnSize: 44; iconSize: 20; anchors.verticalCenter: parent.verticalCenter; btnEnabled: QbzPlayer.npHasTrack; onClicked: root.openTrackInfo() }
                    QbzIconButton { name: "shuffle"; btnSize: 44; iconSize: 20; anchors.verticalCenter: parent.verticalCenter; active: QbzPlayer.npShuffle; btnEnabled: QbzPlayer.npHasTrack; onClicked: QbzPlayer.toggleShuffle() }
                    QbzIconButton { name: "skip-back"; btnSize: 64; iconSize: 24; anchors.verticalCenter: parent.verticalCenter; btnEnabled: QbzPlayer.npHasTrack; onClicked: QbzPlayer.previous() }
                    QbzIconButton { name: QbzPlayer.npPlaying ? "pause" : "play-fill"; btnSize: 64; iconSize: 28; anchors.verticalCenter: parent.verticalCenter; activeBackground: true; active: true; btnEnabled: QbzPlayer.npHasTrack || QbzQueue.hasPlayTarget; onClicked: QbzPlayer.togglePlay() }
                    QbzIconButton { name: "skip-forward"; btnSize: 64; iconSize: 24; anchors.verticalCenter: parent.verticalCenter; btnEnabled: QbzPlayer.npHasTrack; onClicked: QbzPlayer.next() }
                    QbzIconButton { name: QbzPlayer.npRepeatMode === 2 ? "repeat-1" : "repeat"; btnSize: 44; iconSize: 20; anchors.verticalCenter: parent.verticalCenter; active: QbzPlayer.npRepeatMode > 0; btnEnabled: QbzPlayer.npHasTrack; onClicked: QbzPlayer.cycleRepeat() }
                    QbzIconButton { id: addButton; name: "plus"; btnSize: 44; iconSize: 20; anchors.verticalCenter: parent.verticalCenter; btnEnabled: QbzPlayer.npHasTrack; onClicked: addMenu.openBelowRight(addButton) }
                    QbzIconButton { name: root.npFavorite ? "heart-filled" : "heart"; btnSize: 44; iconSize: 20; anchors.verticalCenter: parent.verticalCenter; active: root.npFavorite; btnEnabled: QbzPlayer.npHasTrack && root.npSource === "qobuz"; onClicked: QbzQueue.queueToggleFavorite("track", QbzPlayer.npTrackId) }
                }

                AudioStamp {
                    id: qualityStamp
                    anchors.right: transportRow.right
                    anchors.rightMargin: 2
                    anchors.bottom: transportRow.bottom
                }
            }
        }
    }

    // =====================================================================
    // File-local components
    // =====================================================================

    // The tab used by BOTH panels (:78-115): the left Cover/Lyrics pair and
    // the right Up Next/History pair. Label over a 2px underline, the whole
    // thing clickable, an optional 2px focus ring (the "tab" ring family).
    // Note the delta vs KioskSearch's SearchTab: 14px/bold + a 4px gap here,
    // 15px/semibold + no gap there.
    component QueueTab: Rectangle {
        id: tab

        property string label: ""
        property bool active: false
        // Only the RIGHT panel's pair joins the nav model; the left toggle
        // leaves this false (:82-84).
        property bool navFocused: false
        signal picked()

        implicitWidth: Math.max(64, tabColumn.width + 16)
        implicitHeight: 44
        color: "transparent"
        radius: theme.radiusSm
        border.width: tab.navFocused ? 2 : 0
        border.color: theme.accent

        // alignment: start — the label sits at the TOP of the row's height,
        // so the underlines of both panels line up across the divider.
        Column {
            id: tabColumn
            x: (tab.width - width) / 2
            y: (tab.height - height) / 2
            width: tabLabel.implicitWidth
            spacing: 4

            Text {
                id: tabLabel
                text: tab.label
                color: tab.active ? theme.textPrimary : theme.textMuted
                font.pixelSize: 14
                font.weight: theme.weightBold
            }

            Rectangle {
                width: tabColumn.width
                height: 2
                radius: 2
                color: tab.active ? theme.accent : "transparent"
            }
        }

        MouseArea {
            anchors.fill: tab
            cursorShape: Qt.PointingHandCursor
            onClicked: tab.picked()
        }
    }

    // The queue row (:13-76). NOT KioskTrackRow — 58px instead of 62, a 44px
    // cover instead of 46, and its own type scale.
    component QueueRowLite: Rectangle {
        id: qrow

        property var item: ({})
        // The RESOLVED local cover path; "" leaves the bare elevated square,
        // which is what the reference's `artwork.width > 0` gate does (:36).
        property string artPath: ""
        property bool navFocused: false
        signal play()

        readonly property string rowTitle: qrow.item && qrow.item.title ? qrow.item.title : ""
        readonly property string rowArtist: qrow.item && qrow.item.artist ? qrow.item.artist : ""
        readonly property string rowDuration: qrow.item && qrow.item.duration ? qrow.item.duration : ""

        height: 64
        radius: theme.radiusSm
        // Row ring family: 2px + an 0.18 tint.
        color: qrow.navFocused
            ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.18)
            : rowArea.containsMouse
                ? theme.alphaTier(6)
                : "transparent"
        border.width: qrow.navFocused ? 2 : 0
        border.color: theme.accent

        Row {
            id: rowCells
            anchors.left: qrow.left
            anchors.leftMargin: 8
            anchors.right: qrow.right
            anchors.rightMargin: 12
            anchors.top: qrow.top
            anchors.bottom: qrow.bottom
            spacing: 12

            Item {
                id: artCell
                width: 44
                height: rowCells.height

                Rectangle {
                    id: artTile
                    width: 44
                    height: 44
                    y: (artCell.height - artTile.height) / 2
                    radius: 5
                    color: theme.surfaceElevated
                    clip: true

                    KioskArtwork {
                        anchors.fill: artTile
                        visible: qrow.artPath !== ""
                        source: qrow.artPath
                        radius: 5
                        fit: "crop"
                    }
                }
            }

            Item {
                id: rowTextCol
                width: Math.max(0, rowCells.width - artCell.width - rowDurationLabel.width
                                   - 2 * rowCells.spacing)
                height: rowCells.height

                Column {
                    anchors.verticalCenter: rowTextCol.verticalCenter
                    width: rowTextCol.width
                    spacing: 2

                    Text {
                        width: rowTextCol.width
                        text: qrow.rowTitle
                        // The reference's accent rule (:50). In the reference
                        // `playing` is true ONLY for the now-playing row, and
                        // this view renders only upcoming and history — so the
                        // rule never fires there. The Qt QueueRow (queue_qt.rs
                        // :35-66) carries NO `playing` field at all, so the
                        // read is undefined and the strict === keeps the arm
                        // dead: every row is text-primary, exactly as the
                        // reference renders. Do NOT "fix" it into an
                        // id === npTrackId comparison: that would light rows
                        // the reference leaves plain.
                        color: qrow.item && qrow.item.playing === true
                            ? theme.accent : theme.textPrimary
                        font.pixelSize: 14
                        font.weight: theme.weightMedium
                        elide: Text.ElideRight
                        wrapMode: Text.NoWrap
                        maximumLineCount: 1
                    }

                    Text {
                        width: rowTextCol.width
                        text: qrow.rowArtist
                        color: theme.textMuted
                        font.pixelSize: 13
                        elide: Text.ElideRight
                        wrapMode: Text.NoWrap
                        maximumLineCount: 1
                    }
                }
            }

            Text {
                id: rowDurationLabel
                height: rowCells.height
                text: qrow.rowDuration
                color: theme.textMuted
                font.pixelSize: 13
                verticalAlignment: Text.AlignVCenter
            }
        }

        MouseArea {
            id: rowArea
            anchors.fill: qrow
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: qrow.play()
        }
    }
}
