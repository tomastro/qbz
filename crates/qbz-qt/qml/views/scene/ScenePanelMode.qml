// ArtistScene › sidepanel mode — the two-column artist→albums body, 1:1 with
// Tauri's `.artist-two-column-layout` (ArtistsByLocationView.svelte markup
// :715-817, CSS :1415-1509; the rail is VirtualizedFavoritesArtistList.svelte),
// per 01-tauri-artist-scene.md §9 and §13.
//
// EDGE TO EDGE. Tauri's layout carries `margin: 0 -8px 0 -18px`, which exactly
// cancels `.scene-view`'s 18px/8px padding, so this body starts at the pane's
// left edge and the 18px re-appears INSIDE the rail as its own padding-left.
// The host must therefore mount this component full-bleed — negating its own
// padding if it has any — or the rail lands 18px too far right and the right
// column loses its 8px tail.
//
// NOT views/library/LibraryArtistsPanel.qml, and deliberately so
// (00-CONTRACT.md §4.4). That panel is bound to QbzLibrary and a Library host,
// with a 300px rail, 56px rows, 40px portraits and 200×246 AlbumCards carrying
// follow / play / pin / context-menu affordances the scene must not have.
// Tauri's scene needs 240 / 64 / 48 and a 220px card with two actions. That is
// far more than a 10 % variation, which is the justification TRACK-RULE 5 asks
// for; the artwork-window mechanics ARE reused, verbatim, from that file.
//
// WINDOWED, because the reference is not. Tauri requests 500 albums, and the
// nearest Qt panel walks every album url the moment the document lands
// (LibraryArtistsPanel.qml:182-193) and instantiates every card through nested
// Repeaters (:383-445) — 500 live delegates and 500 artwork downloads for a
// pane showing eight. Here BOTH columns are single ListViews over a flat entry
// model (the LocalArtistsTab / LibraryArtistsPanel rail shape, extended to
// carry grid ROWS), and covers are requested only for the mounted band plus
// one screen of buffer, capped at `coverBatchCap` urls per report. Geometry and
// ordering are Tauri's; only the instantiation is bounded.
//
// PULSE LAW: the only continuous animation on this surface is the rail's name
// ticker, and it rides QbzShell.pulseMs through a Connections that is DISABLED
// while the row is not hovered — a row at rest writes nothing at all. There is
// no local Timer, no NumberAnimation and no Behavior on a data-fed property;
// the hover fades are gesture-bounded, which is the one class the law allows
// (cards/ArtistCard.qml:170).
//
// NO SPINNER for the loading state: QbzSpinner owns an infinite
// RotationAnimation at display rate and QbzSkeleton a repeating 900ms Timer,
// and 00-CONTRACT.md §4.5 closes that door for new code. The three-dot
// QbzLoadingDots is host-driven by design, so its phase is cycled here off the
// shell pulse — three writes a second, only while the pane is actually loading.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root

    // --- the frozen mount API (00-CONTRACT.md §13.1, scene lane) ---------
    /// `sceneJson.panel` — { state, artistId, error, discography, epsSingles,
    /// liveAlbums }.
    property var doc: ({})
    /// `sceneJson.artists` — the FILTERED list the host is showing. The rail
    /// is always A-Z grouped over it, whatever the view's own grouping toggle
    /// says (Tauri passes `groupArtists(filteredArtists)` unconditionally,
    /// V:718, which is why its Group select is hidden in this mode).
    property var artists: []
    /// The selected artist's Qobuz id, "" for none.
    property string selectedId: ""

    signal artistSelected(string qobuzId)
    signal albumActivated(string albumId)
    signal albumPlayed(string albumId)

    QbzTheme { id: theme }

    // ------------------------------ metrics ------------------------------
    // Every number is Tauri's; none is a theme token, because the theme's type
    // scale (fontBody 15, fontLegal 13) does not line up with this surface's
    // 14/13/12/11 and the contract makes Tauri normative for geometry.
    readonly property int railW: 240          // .artist-column width
    readonly property int railPadLeft: 18     // …its padding-left
    readonly property int rowH: 64            // ROW_HEIGHT
    readonly property int groupHeadH: 32      // HEADER_HEIGHT
    readonly property int cardW: 220          // AlbumCardLite .card width
    readonly property int coverS: 220         // …and its square cover
    // 220 cover + the card's 4px column gap + the meta row (a 14/1.3 title
    // over a 13/1.3 artist, 2px apart — 38 — against a 30px quality badge).
    readonly property int cardH: 262
    readonly property int gridGap: 22         // .artist-albums-grid gap
    readonly property int trackMin: 210       // …minmax(210px, 1fr)
    // Section header: a 16/600 title over 12px of air, a 1px rule, and 16px of
    // margin under it.
    readonly property int sectionHeadH: 51
    /// Hard bound on urls asked for in one report — see the windowing note.
    readonly property int coverBatchCap: 48

    // ========================= artwork, url-keyed =========================
    // The mechanism LibraryArtistsPanel.qml:158-193 already uses for its right
    // pane and views/ArtistView.qml for its sidebar: ask QbzShell for a list of
    // urls, take (url, path) pairs back on QbzLibrary.libraryArtworkReady.
    // Arrivals are coalesced into ONE rebind per frame — per-arrival rebinding
    // is O(n²) over a big discography.
    //
    // `_asked` is what makes this bounded rather than merely lazy:
    // sidebar_artwork_window (main.rs:968) DOWNLOADS every url it is handed
    // that is not already on disk, so re-reporting the same band on every
    // scroll pixel would re-queue the same fetches. Never pruned — a cache
    // path that has already been resolved costs one map entry, and dropping it
    // would only make the cover flicker back to the placeholder.
    property var coverMap: ({})
    property var _coverInbox: ({})
    property var _asked: ({})
    Timer {
        id: coverFlush
        interval: 16
        repeat: false
        onTriggered: {
            var m = Object.assign({}, root.coverMap, root._coverInbox)
            root._coverInbox = ({})
            root.coverMap = m
        }
    }
    Connections {
        target: QbzLibrary
        function onLibraryArtworkReady(key, path) {
            root._coverInbox[key] = path
            if (!coverFlush.running)
                coverFlush.start()
        }
    }
    function requestCovers(urls) {
        var out = []
        for (var i = 0; i < urls.length && out.length < root.coverBatchCap; i++) {
            var u = urls[i]
            if (u === "" || root._asked[u] === true || root.coverMap[u])
                continue
            root._asked[u] = true
            out.push(u)
        }
        if (out.length > 0)
            QbzShell.sidebarArtworkWindow(JSON.stringify(out))
    }

    // ------------------- field readers (the lane seam) -------------------
    // 00-CONTRACT.md §13.1 freezes the panel's SHAPE but not the album row's
    // field NAMES. The Rust lane landed them as `albumId` / `artworkUrl` /
    // `year` (4-digit string, already sliced) / `qualityTier` /
    // `qualityDetail` — src/artist_scene_qt.rs:233-279. The alternates below
    // are kept as one-token insurance across that seam: an `id`/`artUrl` row
    // from any other producer still draws, instead of a grid of blank cards
    // with no error anywhere.
    /// The artist's display name, whatever the producer called it.
    ///
    /// Same seam as the album readers above, and it has already bitten once:
    /// the swap to the standard ArtistCard (owner ruling R8) moved the artist
    /// row's `name` to the house `title`, which silently collapsed the host
    /// grid's A-Z grouping into a single "#" bucket — every step stayed a
    /// legal operation on an empty string, so nothing errored.
    function artistNameOf(a) { return String((a && (a.title || a.name)) || "") }
    function albumIdOf(a) { return String(a.albumId || a.id || "") }
    function albumArtOf(a) { return String(a.artworkUrl || a.artUrl || a.imageUrl || "") }
    function albumYearOf(a) {
        if (a.year !== undefined && a.year !== null && String(a.year) !== "")
            return String(a.year)
        // Tauri: `Number(release_date_original?.slice(0,4)) || undefined`.
        var d = String(a.releaseDate || "")
        return d.length >= 4 ? d.slice(0, 4) : ""
    }

    // ============================== the rail ==============================
    // Entry model — { t: 0, label } | { t: 1, item } — so ONE ListView windows
    // both the letter headers and the rows.
    property var railEntries: []
    /// Group keys that have members, for the host's alpha rail if it wants
    /// them. `alphaGroupKey(name)` is Tauri's: the first character uppercased,
    /// kept when /[A-Z]/, else '#' — so accented and non-Latin initials all
    /// collapse into '#' (V:271-276). That is design, not defect: the contract
    /// lists it under "deliberately NOT diverged".
    function alphaKey(name) {
        var c = String(name || "").charAt(0).toUpperCase()
        return (c >= "A" && c <= "Z") ? c : "#"
    }
    function rebuildRail() {
        var src = (root.artists || []).slice()
        // `groupArtists` sorts a COPY by localeCompare, buckets by key, then
        // orders the keys with '#' forced first (V:278-296).
        src.sort(function (a, b) {
            return root.artistNameOf(a).localeCompare(root.artistNameOf(b))
        })
        var buckets = {}
        var order = []
        for (var i = 0; i < src.length; i++) {
            var k = root.alphaKey(root.artistNameOf(src[i]))
            if (buckets[k] === undefined) {
                buckets[k] = []
                order.push(k)
            }
            buckets[k].push(src[i])
        }
        order.sort(function (a, b) {
            if (a === "#") return -1
            if (b === "#") return 1
            return a.localeCompare(b)
        })
        var out = []
        for (var g = 0; g < order.length; g++) {
            out.push({ "t": 0, "label": order[g] })
            var rows = buckets[order[g]]
            for (var r = 0; r < rows.length; r++)
                out.push({ "t": 1, "item": rows[r] })
        }
        root.railEntries = out
    }
    onArtistsChanged: { root.rebuildRail(); root.reportRailSoon() }
    Component.onCompleted: { root.rebuildRail(); root.reportRailSoon() }

    function reportRail() {
        if (!root.visible || root.railEntries.length === 0)
            return
        var first = railList.indexAt(4, railList.contentY + 1)
        var last = railList.indexAt(4, railList.contentY + Math.max(1, railList.height) - 1)
        if (first < 0)
            first = 0
        if (last < 0)
            last = Math.min(root.railEntries.length - 1, first + 12)
        first = Math.max(0, first - 4)
        last = Math.min(root.railEntries.length - 1, last + 4)
        var urls = []
        for (var i = first; i <= last; i++) {
            var e = root.railEntries[i]
            if (e.t === 1 && String(e.item.imageUrl || "") !== "")
                urls.push(String(e.item.imageUrl))
        }
        root.requestCovers(urls)
    }
    /// Report now, then again once the ListView has laid out — `indexAt`
    /// answers -1 before that, which is the state of a rail that has just been
    /// rebuilt or just been mounted (views/local/LocalArtistsTab.qml:330).
    Timer {
        id: railSettle
        interval: 50
        repeat: false
        onTriggered: root.reportRail()
    }
    function reportRailSoon() {
        root.reportRail()
        railSettle.restart()
    }

    Item {
        id: rail
        width: root.railW
        height: parent.height
        clip: true

        ListView {
            id: railList
            x: root.railPadLeft
            width: parent.width - root.railPadLeft
            height: parent.height
            // `.virtual-container { padding: 4px 12px 4px 0 }` — the 12px is
            // taken off the DELEGATE width below, not off the Flickable's
            // rightMargin, which on a vertical list only moves the horizontal
            // content bounds.
            topMargin: 4
            bottomMargin: 4
            clip: true
            cacheBuffer: root.rowH * 6
            boundsBehavior: Flickable.StopAtBounds
            model: root.railEntries

            onContentYChanged: root.reportRail()
            onModelChanged: root.reportRail()
            onHeightChanged: root.reportRail()
            onVisibleChanged: if (visible) root.reportRailSoon()

            delegate: Loader {
                required property var modelData
                width: railList.width - 12
                height: modelData.t === 0 ? root.groupHeadH : root.rowH
                sourceComponent: modelData.t === 0 ? groupHeadComp : artistRowComp

                // `.group-header` — 11/600 uppercase muted, letter-spacing
                // 0.5, padding 12px 8px 4px, and NO count (the grid's header
                // has one; this one does not).
                Component {
                    id: groupHeadComp
                    Item {
                        Text {
                            x: 8
                            y: parent.height - 4 - height
                            text: String(modelData.label).toUpperCase()
                            color: theme.textMuted
                            font.pixelSize: 11
                            font.weight: theme.weightSemibold
                            font.letterSpacing: 0.5
                        }
                    }
                }

                Component {
                    id: artistRowComp
                    Rectangle {
                        id: artistRow

                        readonly property var item: modelData.item
                        readonly property string qobuzId: String(artistRow.item.qobuzId || "")
                        readonly property bool picked: root.selectedId !== ""
                            && root.selectedId === artistRow.qobuzId

                        width: parent.width
                        height: root.rowH
                        radius: 8
                        color: artistRow.picked ? theme.surfaceElevated
                             : (rowArea.containsMouse ? theme.bgHover : "transparent")
                        // `transition: background-color 150ms ease`. Both arms
                        // are gesture-bounded — hover, and a click on this very
                        // row is the only thing that moves the selection.
                        Behavior on color { ColorAnimation { duration: 150 } }

                        // --- ticker state, all of it local to the hovered row
                        // `--ticker-offset: -(overflow + 16)px`,
                        // `--ticker-duration: (overflow + 16) / 40` seconds.
                        readonly property real overflow:
                            Math.max(0, nameText.implicitWidth - nameClip.width)
                        readonly property bool ticking: rowArea.containsMouse && artistRow.overflow > 0
                        readonly property real tickerOffset: -(artistRow.overflow + 16)
                        readonly property real tickerMs: (artistRow.overflow + 16) * 25
                        /// Loop phase 0..1, advanced on the SHELL PULSE. Must
                        /// match settings_qt::shell_pulse_ms (default 33) — a
                        /// mismatch only mis-speeds the ticker, never the
                        /// present rate (immersive/EqualizerBars.qml:55).
                        readonly property int _tickMs: 33
                        property real phase: 0
                        property real tickerX: 0

                        // Disabled while the row is not ticking, so a row at
                        // rest writes NOTHING — an invisible or frozen
                        // component that keeps assigning still dirties the
                        // whole window, which is the entire GPU bill on this
                        // shell.
                        Connections {
                            target: QbzShell
                            enabled: artistRow.ticking
                            function onPulseMsChanged() {
                                artistRow.phase = (artistRow.phase
                                    + artistRow._tickMs / artistRow.tickerMs) % 1.0
                                artistRow.tickerX = artistRow.tickerAt(artistRow.phase)
                            }
                        }
                        onTickingChanged: if (!artistRow.ticking) {
                            // One write on the way out, bounded by the gesture.
                            artistRow.phase = 0
                            artistRow.tickerX = 0
                        }
                        // `@keyframes artist-name-ticker`, linear:
                        //   0%,20% -> 0 · 70%,80% -> offset · 90%,100% -> 0.
                        function tickerAt(u) {
                            var o = artistRow.tickerOffset
                            if (u < 0.2)
                                return 0
                            if (u < 0.7)
                                return o * (u - 0.2) / 0.5
                            if (u < 0.8)
                                return o
                            if (u < 0.9)
                                return o * (1.0 - (u - 0.8) / 0.1)
                            return 0
                        }

                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: 8
                            anchors.rightMargin: 8
                            spacing: 12

                            // 48px circle, `MicVocal size={20}` placeholder.
                            Rectangle {
                                width: 48
                                height: 48
                                anchors.verticalCenter: parent.verticalCenter
                                radius: 24
                                color: theme.surfaceElevated
                                clip: true
                                QbzIcon {
                                    visible: portrait.source === ""
                                    anchors.centerIn: parent
                                    name: "mic-vocal"
                                    width: 20
                                    height: 20
                                    tintName: "muted"
                                }
                                RoundedImage {
                                    id: portrait
                                    anchors.fill: parent
                                    // `artUrl` since the card swap (owner ruling R8): the row is
                                    // published in the house card shape now, and the old
                                    // `imageUrl` no longer exists. The `|| imageUrl` tail is
                                    // the same one-token seam insurance this file documents
                                    // for the album row's field names — a row from any other
                                    // producer still draws instead of a blank rail.
                                    source: root.coverMap[String(artistRow.item.artUrl
                                                                 || artistRow.item.imageUrl || "")] || ""
                                    radius: 24
                                }
                            }

                            Item {
                                id: nameClip
                                width: parent.width - 48 - 12
                                height: parent.height
                                clip: true
                                Text {
                                    id: nameText
                                    x: artistRow.tickerX
                                    width: artistRow.ticking ? implicitWidth : nameClip.width
                                    height: parent.height
                                    text: root.artistNameOf(artistRow.item)
                                    color: theme.textPrimary
                                    font.pixelSize: 14
                                    font.weight: theme.weightMedium
                                    verticalAlignment: Text.AlignVCenter
                                    // `text-overflow: clip` while ticking.
                                    elide: artistRow.ticking ? Text.ElideNone : Text.ElideRight
                                }
                            }
                        }

                        MouseArea {
                            id: rowArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.artistSelected(artistRow.qobuzId)
                        }
                    }
                }
            }
        }
    }

    // `border-right: 1px solid var(--bg-tertiary)` — the reference's rule is
    // the ELEVATED surface, not a border token.
    Rectangle {
        x: root.railW
        width: 1
        height: parent.height
        color: theme.surfaceElevated
    }

    // ========================= the albums column =========================
    Item {
        id: pane

        // `.artist-albums-column { padding: 0 8px 0 24px }` plus the scroll
        // area's own `padding-right: 8px`.
        readonly property int padLeft: 24
        readonly property int padRight: 16
        readonly property int gridW: Math.max(root.trackMin, pane.width - pane.padLeft - pane.padRight)
        readonly property int columns: Math.max(
            1, Math.floor((pane.gridW + root.gridGap) / (root.trackMin + root.gridGap)))
        // `auto-fill minmax(210px, 1fr)`: the TRACK stretches to 1fr, while the
        // card stays 220 wide and may overflow it. 00-CONTRACT.md §6 lists that
        // mismatch under "deliberately NOT diverged" — reproduce it, do not fix
        // it silently.
        readonly property real trackW: (pane.gridW - (pane.columns - 1) * root.gridGap) / pane.columns

        // NOT `state`: every Item already owns a `state` string for QML states,
        // and redeclaring it is a hard parse error.
        readonly property string panelState: String(root.doc.state || "idle")
        readonly property bool showIdle: root.selectedId === "" || pane.panelState === "idle"
        readonly property bool showLoading: !pane.showIdle && pane.panelState === "loading"
        readonly property bool showError: !pane.showIdle && pane.panelState === "error"
        readonly property bool showEmpty: !pane.showIdle && pane.panelState !== "loading"
            && pane.panelState !== "error" && root.albumEntries.length === 0
        readonly property bool showList: !pane.showIdle && pane.panelState !== "loading"
            && pane.panelState !== "error" && root.albumEntries.length > 0

        x: root.railW + 1
        y: 0
        width: parent.width - x
        height: parent.height
        clip: true

        // --- 1 · nothing selected (`MicVocal size={48}`) ------------------
        // The four pre-list states share `.artist-albums-*`: a centred column,
        // gap 12, 14px muted.
        Column {
            visible: pane.showIdle
            anchors.centerIn: parent
            spacing: 12
            QbzIcon {
                anchors.horizontalCenter: parent.horizontalCenter
                name: "mic-vocal"
                width: 48
                height: 48
                tintName: "muted"
            }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: QbzSession.tr("Select an artist to see their albums", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: 14
            }
        }

        // --- 2 · loading --------------------------------------------------
        Column {
            visible: pane.showLoading
            anchors.centerIn: parent
            spacing: 12
            QbzLoadingDots {
                id: dots
                anchors.horizontalCenter: parent.horizontalCenter
                // Cycled off the shell pulse instead of a local Timer, and only
                // while this column is up: ~300ms per step, three property
                // writes a second, zero when idle.
                property int accMs: 0
                Connections {
                    target: QbzShell
                    enabled: pane.showLoading
                    function onPulseMsChanged() {
                        dots.accMs += 33
                        if (dots.accMs >= 300) {
                            dots.accMs = 0
                            dots.phase = (dots.phase + 1) % 3
                        }
                    }
                }
            }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: QbzSession.tr("Loading albums...", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: 14
            }
        }

        // --- 3 · error, with the raw detail under it ----------------------
        Column {
            visible: pane.showError
            anchors.centerIn: parent
            spacing: 12
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: QbzSession.tr("Failed to load albums", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: 14
            }
            // `.error-detail { font-size: 12px; opacity: 0.7 }` — the raw
            // String(err), untranslated by construction.
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                width: Math.min(implicitWidth, pane.width - 48)
                text: String(root.doc.error || "")
                color: theme.textMuted
                opacity: 0.7
                font.pixelSize: 12
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
            }
        }

        // --- 4 · loaded, nothing to show (`Disc3 size={48}`) --------------
        Column {
            visible: pane.showEmpty
            anchors.centerIn: parent
            spacing: 12
            QbzIcon {
                anchors.horizontalCenter: parent.horizontalCenter
                name: "disc-3"
                width: 48
                height: 48
                tintName: "muted"
            }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: QbzSession.tr("No albums found", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: 14
            }
        }

        // --- 5 · the three sections ---------------------------------------
        ListView {
            id: albumList
            visible: pane.showList
            x: pane.padLeft
            width: pane.gridW
            height: parent.height
            clip: true
            cacheBuffer: root.cardH
            boundsBehavior: Flickable.StopAtBounds
            model: root.albumEntries

            onContentYChanged: root.reportAlbums()
            onModelChanged: root.reportAlbums()
            onHeightChanged: root.reportAlbums()
            onVisibleChanged: if (visible) root.reportAlbumsSoon()

            delegate: Loader {
                required property var modelData
                width: albumList.width
                height: modelData.h
                sourceComponent: modelData.t === 0 ? sectionHeadComp : albumRowComp

                Component {
                    id: sectionHeadComp
                    Item {
                        // `align-items: baseline; gap: 12px`, then 12px of air,
                        // a 1px rule and 16px of margin.
                        Text {
                            id: secTitle
                            y: 0
                            text: modelData.title
                            color: theme.textPrimary
                            font.pixelSize: 16
                            font.weight: theme.weightSemibold
                        }
                        Text {
                            anchors.left: secTitle.right
                            anchors.leftMargin: 12
                            anchors.baseline: secTitle.baseline
                            text: modelData.count
                            color: theme.textMuted
                            font.pixelSize: 12
                        }
                        Rectangle {
                            y: secTitle.height + 12
                            width: parent.width
                            height: 1
                            color: theme.surfaceElevated
                        }
                    }
                }

                Component {
                    id: albumRowComp
                    Item {
                        Repeater {
                            model: modelData.albums
                            delegate: SceneAlbumCard {
                                required property var modelData
                                required property int index

                                x: index * (pane.trackW + root.gridGap)
                                albumId: root.albumIdOf(modelData)
                                title: String(modelData.title || "")
                                artist: String(modelData.artist || modelData.artistName || "")
                                genre: String(modelData.genre || "")
                                year: root.albumYearOf(modelData)
                                artSource: root.coverMap[root.albumArtOf(modelData)] || ""
                                bitDepth: Number(modelData.bitDepth || 0)
                                samplingRate: Number(modelData.samplingRate || 0)
                                qualityTier: String(modelData.qualityTier || "")
                                qualityDetail: String(modelData.qualityDetail || "")
                                onActivated: root.albumActivated(albumId)
                                onPlayed: root.albumPlayed(albumId)
                            }
                        }
                    }
                }
            }
        }

        QbzScrollBar {
            anchors.right: parent.right
            anchors.rightMargin: 4
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            target: albumList
            visible: albumList.visible && albumList.contentHeight > albumList.height
        }
    }

    // ===================== the album entry model =========================
    // Flat rows so ONE ListView windows headers AND grid rows:
    //   { t: 0, title, count, h } | { t: 1, albums: [...], h }
    // The trailing margins live in `h` rather than in ListView.spacing,
    // because the three gaps differ: 22 between grid rows, 32 under a section,
    // 16 under the LAST one.
    property var albumEntries: []
    function rebuildAlbums() {
        var secs = [
            { "title": QbzSession.tr("Discography", QbzSession.trRev),
              "albums": root.doc.discography || [] },
            { "title": QbzSession.tr("EPs & Singles", QbzSession.trRev),
              "albums": root.doc.epsSingles || [] },
            { "title": QbzSession.tr("Live Albums", QbzSession.trRev),
              "albums": root.doc.liveAlbums || [] }
        ]
        // Fixed order, each rendered only when non-empty (V:748-813).
        var live = []
        for (var s = 0; s < secs.length; s++)
            if (secs[s].albums.length > 0)
                live.push(secs[s])

        var out = []
        var cols = pane.columns
        for (s = 0; s < live.length; s++) {
            out.push({ "t": 0, "title": live[s].title,
                       "count": live[s].albums.length, "h": root.sectionHeadH })
            var al = live[s].albums
            var rows = Math.ceil(al.length / cols)
            for (var r = 0; r < rows; r++) {
                var lastRow = (r === rows - 1)
                var tail = lastRow ? (s === live.length - 1 ? 16 : 32) : root.gridGap
                out.push({ "t": 1, "albums": al.slice(r * cols, (r + 1) * cols),
                           "h": root.cardH + tail })
            }
        }
        root.albumEntries = out
    }
    onDocChanged: {
        // A new document is a new artist: forget which covers were asked for,
        // or the next pane starts with an `_asked` map full of the previous
        // artist's urls and re-requests nothing it needs.
        root._asked = ({})
        root.rebuildAlbums()
        root.reportAlbumsSoon()
    }
    // The column count is width-derived, so a resize re-slices the rows.
    Connections {
        target: pane
        function onColumnsChanged() { root.rebuildAlbums() }
    }
    // A live language switch must re-slice too. `rebuildAlbums` calls
    // `QbzSession.tr(...)` for the three section titles and stores the RESULT
    // in `albumEntries`, and the delegate renders that snapshot verbatim — so
    // the strings are data, not bindings, and `trRev` cannot reach them. Its
    // two other triggers (a new document, a resize) may never fire again after
    // the user changes language, which would leave the previous language's
    // headings on screen indefinitely. Same shape as the house pattern that
    // threads the revision into a builder (NavFlyout.qml:111,
    // ArtistView.qml:1363).
    Connections {
        target: QbzSession
        function onTrRevChanged() { root.rebuildAlbums() }
    }

    function reportAlbums() {
        if (!root.visible || !albumList.visible || root.albumEntries.length === 0)
            return
        var first = albumList.indexAt(4, albumList.contentY + 1)
        var last = albumList.indexAt(4, albumList.contentY + Math.max(1, albumList.height) - 1)
        if (first < 0)
            first = 0
        if (last < 0)
            last = Math.min(root.albumEntries.length - 1, first + 3)
        // One screen of buffer either side, which is what keeps a fast scroll
        // from showing placeholders — and the cap above is what keeps a fast
        // scroll from queueing the whole discography.
        first = Math.max(0, first - 1)
        last = Math.min(root.albumEntries.length - 1, last + 1)
        var urls = []
        for (var i = first; i <= last; i++) {
            var e = root.albumEntries[i]
            if (e.t !== 1)
                continue
            for (var a = 0; a < e.albums.length; a++) {
                var u = root.albumArtOf(e.albums[a])
                if (u !== "")
                    urls.push(u)
            }
        }
        root.requestCovers(urls)
    }
    Timer {
        id: albumSettle
        interval: 50
        repeat: false
        onTriggered: root.reportAlbums()
    }
    function reportAlbumsSoon() {
        root.reportAlbums()
        albumSettle.restart()
    }

    // ============================ the album card =========================
    // AlbumCardLite, not cards/AlbumCard.qml: 220 wide over a 220×220 cover at
    // radius 6, a 4px gap, then a meta row of the title/artist stack against
    // the quality badge. The shared card is 200×246 with a pin badge, a heart,
    // an award ribbon, a source badge and a nine-entry context menu — none of
    // which this surface has.
    //
    // Declared as an inline component, which QML requires at the file's top
    // level, and it reads NOTHING off an outer id: every input arrives as a
    // property (the rule rows/TrackRow.qml:359-363 records).
    component SceneAlbumCard: Item {
        id: acard

        property string albumId: ""
        property string title: ""
        property string artist: ""
        property string genre: ""
        property string year: ""
        property string artSource: ""
        property int bitDepth: 0
        property real samplingRate: 0
        /// The tier the RUST side already resolved (artist_scene_qt.rs), not a
        /// number for QML to re-derive. See the badge mount below.
        property string qualityTier: ""
        property string qualityDetail: ""

        signal activated()
        signal played()

        QbzTheme { id: cardTheme }

        width: 220
        height: 262

        Rectangle {
            id: cover
            width: 220
            height: 220
            radius: 6
            color: cardTheme.surfaceElevated
            clip: true

            RoundedImage {
                anchors.fill: parent
                source: acard.artSource
                radius: 6
            }

            // `.cover-wrap::after` — a full scrim at 0.6, 150ms.
            Rectangle {
                anchors.fill: parent
                radius: 6
                color: Qt.rgba(0, 0, 0, 0.6)
                opacity: coverArea.containsMouse ? 1.0 : 0.0
                Behavior on opacity { NumberAnimation { duration: 150 } }
            }

            // `.meta` — top-left, and rendered ONLY when there is something to
            // say (`{#if genre || releaseLabel}`); a card with neither shows no
            // overlay text at all.
            Column {
                visible: acard.genre !== "" || acard.year !== ""
                x: 12
                y: 12
                width: parent.width * 0.7
                spacing: 2
                opacity: coverArea.containsMouse ? 1.0 : 0.0
                Behavior on opacity { NumberAnimation { duration: 150 } }
                Text {
                    visible: acard.genre !== ""
                    width: parent.width
                    text: acard.genre
                    // Fixed light-on-scrim by design, like every other overlay
                    // in this port: the host is a 0.6 black wash under every
                    // theme (theme/QbzIcon.qml, "FIXED (theme-independent)").
                    color: Qt.rgba(1, 1, 1, 0.92)
                    font.pixelSize: 14
                    elide: Text.ElideRight
                }
                Text {
                    visible: acard.year !== ""
                    width: parent.width
                    text: acard.year
                    color: Qt.rgba(1, 1, 1, 0.92)
                    opacity: 0.8
                    font.pixelSize: 13
                    elide: Text.ElideRight
                }
            }

            // Card-open + hover detector, before the buttons so they win the
            // pointer (cards/ArtistCard.qml:172-174).
            MouseArea {
                id: coverArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: acard.activated()
            }

            // `.actions` — bottom-centre, `bottom: 44px`, so the 44px row's top
            // sits at 220 - 44 - 44.
            //
            // ONE button, where Tauri draws three. Its Heart is inert by
            // construction (this view never passes `onFavorite`), and its kebab
            // opens AlbumQuickMenu with exactly one live item, "Go to album" —
            // which is what a click on the card already does. The frozen mount
            // API for this component carries no favourite and no menu signal,
            // and 00-CONTRACT.md's DV-2 rationale ("dead UI is neither pretty
            // nor functional") settles the rest. Recorded as a deviation.
            CardOverlayRow {
                y: 132
                width: parent.width
                shown: coverArea.containsMouse
                CardOverlayButton {
                    name: "play-fill"
                    primary: true
                    anchors.verticalCenter: parent.verticalCenter
                    onClicked: acard.played()
                }
            }
        }

        // `.meta-row { align-items: center; gap: 8px }` over
        // `.text-stack { gap: 2px }`, 4px under the cover.
        Item {
            y: 224
            width: parent.width
            height: 38

            Column {
                id: textStack
                // The badge hides itself on an unknown tier (DV-9), and an
                // invisible Item still HAS a width — so the 8px gap and the
                // badge's own box only come off the stack when it is drawn.
                width: parent.width - (badge.visible ? badge.width + 8 : 0)
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Text {
                    width: parent.width
                    text: acard.title
                    color: cardTheme.textPrimary
                    font.pixelSize: 14
                    font.weight: cardTheme.weightMedium
                    elide: Text.ElideRight
                }
                Text {
                    width: parent.width
                    text: acard.artist
                    color: cardTheme.textMuted
                    font.pixelSize: 13
                    elide: Text.ElideRight
                }
            }

            // DV-9: Tauri passes no quality at all here, so QualityBadgeStatic
            // falls through its tier ladder and EVERY scene album card claims
            // CD. Misinformation about audio quality is load-bearing in this
            // app, so the real numbers ride the row and the badge hides itself
            // when the depth is unknown (the home_qt.rs:1183-1200 rule, which
            // QualityBadge already implements through its own
            // `visible: resolvedTier !== ""` — do NOT override that binding).
            QualityBadge {
                id: badge
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                // THE TIER COMES FROM RUST. Passing only the raw numbers let
                // QualityBadge re-derive it in QML, and its ladder ends in
                // `if (samplingRate >= 44.1) return "cd"` — so every album
                // whose depth was unknown fell out as CD. That is precisely
                // the reference defect DV-9 exists to remove: Tauri passed no
                // quality at all and stamped a CD badge on the entire grid.
                // `artist_scene_qt.rs` resolves the tier from the real depth
                // and BLANKS it when the depth is unknown, and the badge hides
                // itself on an empty tier — so an album we cannot vouch for
                // now shows nothing rather than a confident lie.
                //
                // Same shape as every other card in the app
                // (cards/AlbumCard.qml:687, local/LocalAlbumRow.qml:162,
                // library/FeedListRow.qml:278, cards/TrackCard.qml:223).
                tierOverride: acard.qualityTier
                label: acard.qualityDetail
                // Kept for the hover tooltip only, never for tier resolution.
                bitDepth: acard.bitDepth
                samplingRate: acard.samplingRate
            }
        }
    }
}
