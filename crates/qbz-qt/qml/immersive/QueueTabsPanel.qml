// QueueTabsPanel — the immersive Queue/History panel (§6.5 of the
// 2026-08-02 immersive-port contract), port of
// crates/qbz-ui/ui/immersive/ImmersiveQueuePanel.slint:253-543. ONE component
// used in BOTH placements: the FOCUS full-screen mount (mode == 5,
// `centered: false`) and the SPLIT right slot (split-panel == 3,
// `centered: true`) — mirroring Tauri's single QueuePanel.svelte.
//
// Two sub-tabs ("Queue" / "History"). The QUEUE tab shows a highlighted NOW
// PLAYING row plus a scrolling UP NEXT list sliced off the FLAT coverflow
// document; the HISTORY tab shows the most-recent-first history list. The
// playback logic is NOT duplicated — rows only fire QbzQueue invokables:
// up-next row at flat index f -> queuePlayUpcomingFlat(f - index - 1) (the
// QUEUE-WIDE flat invokable, §4.4/trap 23 — NEVER the page-local
// queuePlayUpcoming); history row i -> queuePlayHistory(i) (already
// queue-wide, most-recent-first).
//
// BADGES (verified formulas, §6.5): Queue = max(0, coverflow.tracks.length -
// coverflow.index - 1) — from the FLAT coverflow model, NOT the paged
// queueJson (:267-268); History = queueJson.history.length (:304).
//
// Covers: both documents carry artUrl only; resolution rides the shared
// artwork pipeline (QbzShell.sidebarArtworkWindow dispatch +
// QbzLibrary.libraryArtworkReady -> coverMap), the CoverflowPanel.qml:81-104
// idiom. The Slint `upcoming-count` art-decode window widening is
// Slint-Rust-internal — nothing to port (D9).
//
// The panel re-pulls the queue snapshot when it becomes visible
// (QueueState.panel-opened, :253-255) — with Qt's eager instantiation +
// visible-gating the visible false->true edge IS the conditional mount.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    // true in the split (centered) placement, false in focus full-screen
    // (:244).
    property bool centered: true

    // --- Data -------------------------------------------------------------
    // Guarded try/catch (trap 15); the bridges seed full-shape defaults so
    // the fallbacks are paranoia, not the steady state.
    readonly property var queueDoc: {
        try {
            var d = JSON.parse(QbzQueue.queueJson)
            if (d)
                return d
        } catch (e) {
        }
        return ({})
    }
    readonly property var cfDoc: {
        try {
            var d = JSON.parse(QbzQueue.coverflowJson)
            if (d && d.tracks)
                return d
        } catch (e) {
        }
        return { "index": 0, "tracks": [] }
    }
    // Doc FIELD reads go through plain functions, NEVER chained readonly
    // vars: `readonly property var x: queueDoc.field` stays UNDEFINED in the
    // app context (measured 2026-08-03, offscreen probe) — the binding is
    // never installed. The TrackInfoModal.qml parseDoc() idiom re-reads the
    // doc per call and the binding dependency is captured through the call.
    function historyRows() {
        return root.queueDoc.history || []
    }
    function hasCurrentTrack() {
        return root.queueDoc.hasCurrent === true
    }
    function currentTrack() {
        return hasCurrentTrack() ? root.queueDoc.current : null
    }

    // The upcoming entries are the slice of the coverflow tracks AFTER the
    // now-playing index; their COUNT drives the tab badge + the section
    // header. Floored at 0 (:266-268).
    readonly property int upcomingCount: Math.max(0,
        root.cfDoc.tracks.length - root.cfDoc.index - 1)

    // The active sub-tab. Slint stores this in the shared QueueState.tab;
    // panel-local state is the same observable behavior within the panel
    // (the desktop QueuePanel's tab state is likewise its own).
    property int tab: 0

    // Becoming visible = the Slint conditional mount -> re-pull the queue
    // (:253-255).
    onVisibleChanged: if (visible) QbzQueue.queuePanelOpened()

    // --- Cover resolution (shared artwork pipeline) ------------------------
    property var coverMap: ({})
    Connections {
        target: QbzLibrary
        function onLibraryArtworkReady(key, path) {
            var m = root.coverMap
            m[key] = path
            root.coverMap = Object.assign({}, m)
        }
    }
    Component.onCompleted: root.dispatchCovers()
    onCfDocChanged: root.dispatchCovers()
    onQueueDocChanged: root.dispatchCovers()
    function dispatchCovers() {
        var urls = []
        var tracks = root.cfDoc.tracks || []
        for (var i = 0; i < tracks.length; i++)
            if (tracks[i].artUrl)
                urls.push(tracks[i].artUrl)
        var hist = root.historyRows()
        for (var j = 0; j < hist.length; j++)
            if (hist[j].artUrl)
                urls.push(hist[j].artUrl)
        var cur = root.currentTrack()
        if (cur && cur.artUrl)
            urls.push(cur.artUrl)
        if (urls.length > 0)
            QbzShell.sidebarArtworkWindow(JSON.stringify(urls))
    }
    function coverFor(row) {
        return (row && row.artUrl) ? (root.coverMap[row.artUrl] || "") : ""
    }

    // Content width: focus full-screen caps to a comfortable reading column
    // (min(vw-96, 720)); split already lives in the right half and fills it
    // (:260-262).
    readonly property real contentWidth: root.centered
        ? root.width : Math.min(root.width - 96, 720)

    // --- Inline components -------------------------------------------------

    // Explicit "E" badge — the immersive (white-on-glass) variant (:28-44).
    component ImmExplicitBadge: Rectangle {
        width: 16
        height: 16
        radius: 3
        color: "#29ffffff"
        Text {
            anchors.centerIn: parent
            text: "E"
            color: "#d9ffffff"
            font.pixelSize: 9
            font.weight: Font.DemiBold // 600
        }
    }

    // One Queue / History sub-tab — plain TEXT, NO pill background (ADR-008);
    // the active tab is marked by a 2px underline (:51-98).
    component SubTab: Rectangle {
        id: subTab
        property string label: ""
        property int count: 0
        property bool active: false
        signal clicked()

        width: tabRow.implicitWidth
        height: 34
        color: "transparent"

        Row {
            id: tabRow
            height: parent.height
            spacing: 5
            Text {
                text: subTab.label
                color: subTab.active ? "#ffffff"
                    : (tabArea.containsMouse ? "#e6ffffff" : "#99ffffff")
                font.pixelSize: 18
                font.weight: subTab.active ? Font.Bold : Font.Medium // 700 / 500
                Behavior on color { ColorAnimation { duration: 150 } }
            }
            Text {
                visible: subTab.count > 0
                text: "(" + subTab.count + ")"
                color: subTab.active ? "#b3ffffff" : "#66ffffff"
                font.pixelSize: 15
                font.weight: Font.Medium
                Behavior on color { ColorAnimation { duration: 150 } }
            }
        }
        // Active underline — 2px flush to the bottom (:84-92).
        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: 2
            radius: 1
            color: subTab.active ? "#ffffff"
                : (tabArea.containsMouse ? "#33ffffff" : "transparent")
            Behavior on color { ColorAnimation { duration: 150 } }
        }
        MouseArea {
            id: tabArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: subTab.clicked()
        }
    }

    // Section header — small, uppercase, tracked-out (:102-107). The msgids
    // are already caps.
    component SectionLabel: Text {
        color: "#b3ffffff"
        font.pixelSize: 13
        font.weight: Font.Bold // 700
        font.letterSpacing: 1.2
    }

    // One UP NEXT / History row (:113-238): 56px, 40x40 cover, title/artist,
    // duration, hover-revealed circular play button. Fires play() on click
    // OR the play button — the mount maps it to the right QbzQueue call.
    component ImmQueueRow: Rectangle {
        id: immRow
        property var item: null
        property bool current: false     // NOW PLAYING highlight
        property string artSource: ""
        signal play()

        width: parent ? parent.width : 0
        height: 56
        radius: 10
        // `.track-item.current`: brighter fill + 1px hairline so it reads as
        // the anchor over ANY album atmosphere; plain rows lift on hover
        // (:122-127).
        color: current ? "#24ffffff"
            : (rowArea.containsMouse ? "#1affffff" : "transparent")
        border.width: current ? 1 : 0
        border.color: "#3dffffff"
        Behavior on color { ColorAnimation { duration: 150 } }

        // The row TouchArea is declared FIRST (Slint :130-133) so the
        // content — the hover-revealed play button's own MouseArea — stacks
        // ABOVE it; declared last it would swallow the button's clicks.
        MouseArea {
            id: rowArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: immRow.play()
        }

        Row {
            anchors.fill: parent
            anchors.leftMargin: 10
            anchors.rightMargin: 8
            spacing: 12

            // 40x40 cover with a 1px inset hairline (:142-159).
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 40
                height: 40
                radius: 5
                color: "#14ffffff"
                border.width: 1
                border.color: "#1fffffff"
                clip: true
                Image {
                    anchors.fill: parent
                    source: immRow.artSource
                    visible: source !== "" && status === Image.Ready
                    fillMode: Image.PreserveAspectCrop
                    asynchronous: true
                }
            }

            // Title + artist (:161-190).
            Column {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - 40 - 12 - 36 - 12
                spacing: 3
                Row {
                    spacing: 6
                    // The Row's width must be EXPLICIT (the Column's): the
                    // title's elide width binds against it, so an implicit
                    // (content-derived) Row width loops the binding and the
                    // title collapses to 0px — measured over RFB 2026-08-03
                    // (rows rendered artist-only).
                    width: parent.width
                    Text {
                        text: item ? (item.title || "") : ""
                        color: "#ffffff"
                        font.pixelSize: 17
                        font.weight: current ? Font.DemiBold : Font.Medium // 600 / 500
                        elide: Text.ElideRight
                        width: Math.min(implicitWidth,
                            parent.width - (expBadge.visible ? 22 : 0))
                    }
                    ImmExplicitBadge {
                        id: expBadge
                        visible: item && item.explicit === true
                        anchors.verticalCenter: parent.verticalCenter
                    }
                }
                Text {
                    text: item ? (item.artist || "") : ""
                    color: "#b3ffffff"
                    font.pixelSize: 15
                    elide: Text.ElideRight
                    width: parent.width
                }
            }

            // Duration + hover-revealed play affordance share one trailing
            // slot (:192-236).
            Item {
                anchors.verticalCenter: parent.verticalCenter
                width: 36
                height: 30
                Text {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    text: item ? (item.duration || "") : ""
                    color: "#99ffffff"
                    font.pixelSize: 14
                    font.weight: Font.Medium
                    opacity: (!current && rowArea.containsMouse) ? 0.0 : 1.0
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                }
                // 30px circular play button; hidden for the current row
                // (:213-235).
                Rectangle {
                    visible: !current
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    width: 30
                    height: 30
                    radius: 15
                    color: playArea.containsMouse ? "#59ffffff" : "#33ffffff"
                    opacity: rowArea.containsMouse || playArea.containsMouse ? 1.0 : 0.0
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                    Behavior on color { ColorAnimation { duration: 120 } }
                    QbzIcon {
                        name: "play-fill"
                        width: 13
                        height: 13
                        anchors.centerIn: parent
                        anchors.horizontalCenterOffset: 1
                        tintName: "white"
                    }
                    MouseArea {
                        id: playArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: immRow.play()
                    }
                }
            }
        }
    }

    // --- Centered content column (:271-279) ---------------------------------
    // padding-top 56 (clear the top chrome) / padding-bottom 132 (clear the
    // floating player) — applied in BOTH placements, as Slint; the 8px side
    // padding is focus-only (:277-278).
    Item {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.topMargin: 56
        anchors.bottomMargin: 132
        width: root.contentWidth

        Column {
            anchors.fill: parent
            anchors.leftMargin: root.centered ? 0 : 8
            anchors.rightMargin: root.centered ? 0 : 8
            spacing: 16

            // --- Sub-tab header (:287-354) ----------------------------------
            // Tabs LEFT, Clear flush at the TRAILING edge; a fixed 34px row
            // height puts the labels and the button on one baseline.
            Item {
                width: parent.width
                height: 34

                Row {
                    anchors.left: parent.left
                    height: parent.height
                    spacing: 18
                    SubTab {
                        label: QbzSession.tr("Queue", QbzSession.trRev)
                        count: root.upcomingCount
                        active: root.tab === 0
                        onClicked: root.tab = 0
                    }
                    SubTab {
                        label: QbzSession.tr("History", QbzSession.trRev)
                        count: root.historyRows().length
                        active: root.tab === 1
                        onClicked: root.tab = 1
                    }
                }

                // Clear-queue button — Queue tab only, when there is
                // something upcoming (:316-353). NO pill — a soft bordered
                // rectangle (ADR-008).
                Rectangle {
                    visible: root.tab === 0 && root.upcomingCount > 0
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    width: clearRow.implicitWidth + 24
                    height: 32
                    radius: 8
                    color: clearArea.containsMouse ? "#29ffffff" : "#14ffffff"
                    border.width: 1
                    border.color: clearArea.containsMouse ? "#40ffffff" : "#26ffffff"
                    Behavior on color { ColorAnimation { duration: 150 } }
                    Behavior on border.color { ColorAnimation { duration: 150 } }
                    Row {
                        id: clearRow
                        anchors.centerIn: parent
                        spacing: 7
                        QbzIcon {
                            name: "trash-2"
                            width: 14
                            height: 14
                            anchors.verticalCenter: parent.verticalCenter
                            tintName: "white"
                            opacity: clearArea.containsMouse ? 1.0 : 0.8
                        }
                        Text {
                            text: QbzSession.tr("Clear", QbzSession.trRev)
                            color: clearArea.containsMouse ? "#ffffff" : "#ccffffff"
                            font.pixelSize: 15
                            font.weight: Font.Medium
                            anchors.verticalCenter: parent.verticalCenter
                        }
                    }
                    MouseArea {
                        id: clearArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: QbzQueue.queueClear()
                    }
                }
            }
            // Header underline (:356-359).
            Rectangle {
                width: parent.width
                height: 1
                color: "#21ffffff"
            }

            // --- Body (:361-552) --------------------------------------------
            Item {
                width: parent.width
                height: parent.height - 34 - 1 - 32

                // ===== QUEUE tab =====
                Item {
                    anchors.fill: parent
                    visible: root.tab === 0

                    // NOW PLAYING + UP NEXT, OR the empty state (:368).
                    Column {
                        anchors.fill: parent
                        visible: root.hasCurrentTrack() || root.upcomingCount > 0
                        spacing: 16

                        // NOW PLAYING section (:372-381).
                        Column {
                            id: nowCol
                            visible: root.hasCurrentTrack()
                            width: parent.width
                            spacing: 10
                            SectionLabel {
                                text: QbzSession.tr("NOW PLAYING", QbzSession.trRev)
                            }
                            ImmQueueRow {
                                item: root.currentTrack()
                                current: true
                                // The unified now-playing cover is sharper;
                                // fall back to the row's own art.
                                artSource: QbzPlayer.npArtworkPath !== ""
                                    ? QbzPlayer.npArtworkPath
                                    : root.coverFor(root.currentTrack())
                            }
                        }

                        // UP NEXT section (:383-455). Fills the body left
                        // under the NOW PLAYING section.
                        Column {
                            visible: root.upcomingCount > 0
                            width: parent.width
                            height: parent.height
                                - (nowCol.visible ? nowCol.implicitHeight + 16 : 0)
                            spacing: 10

                            Row {
                                id: upHeader
                                spacing: 8
                                SectionLabel {
                                    text: QbzSession.tr("UP NEXT", QbzSession.trRev)
                                }
                                SectionLabel {
                                    text: "· " + root.upcomingCount
                                    color: "#80ffffff"
                                    font.weight: Font.Medium
                                    font.letterSpacing: 0.4
                                }
                            }

                            Item {
                                width: parent.width
                                height: parent.height - upHeader.implicitHeight - 10
                                Flickable {
                                    id: upFlick
                                    anchors.fill: parent
                                    // Trailing gutter so rows never tuck
                                    // under the overlaid scrollbar (:413).
                                    contentWidth: width - 16
                                    contentHeight: upList.implicitHeight
                                    boundsBehavior: Flickable.StopAtBounds
                                    clip: true

                                    Column {
                                        id: upList
                                        width: upFlick.contentWidth
                                        // spacing 0 — the inter-row gap is
                                        // baked into each VISIBLE wrapper's
                                        // height (60 = 56 row + 4 gap), with
                                        // the row anchored to the bottom.
                                        // History entries (idx <= index)
                                        // collapse to 0px so a long history
                                        // adds NO phantom top space
                                        // (:414-421).
                                        spacing: 0
                                        // The model is the WHOLE flat queue,
                                        // so the slot stays (its height is
                                        // what makes the collapsed-history
                                        // arithmetic above work) but the ROW
                                        // inside is built only near the
                                        // viewport. Without this, opening this
                                        // tab on a long queue built one
                                        // ImmQueueRow per track — including
                                        // every 0-height history slot, which
                                        // is the majority of them. See
                                        // qbz-nix-docs/qt-frontend/
                                        // 2026-08-18-unbounded-build-sweep.
                                        Repeater {
                                            model: root.cfDoc.tracks
                                            delegate: Item {
                                                id: upSlot
                                                required property int index
                                                required property var modelData
                                                width: upList.width
                                                height: upSlot.index > root.cfDoc.index ? 60 : 0
                                                visible: upSlot.index > root.cfDoc.index
                                                // y of this row inside upList,
                                                // derived the same way the
                                                // heights above stack it.
                                                readonly property real slotY:
                                                    (upSlot.index - root.cfDoc.index - 1) * 60
                                                // One screenful of margin each
                                                // way, so a flick reveals rows
                                                // that already exist.
                                                readonly property bool inBand:
                                                    upSlot.visible
                                                    && upSlot.slotY > upFlick.contentY - upFlick.height
                                                    && upSlot.slotY < upFlick.contentY + 2 * upFlick.height
                                                Loader {
                                                    anchors.left: parent.left
                                                    anchors.right: parent.right
                                                    anchors.bottom: parent.bottom
                                                    active: upSlot.inBand
                                                    sourceComponent: ImmQueueRow {
                                                        item: upSlot.modelData
                                                        artSource: root.coverFor(upSlot.modelData)
                                                        // Flat index f ->
                                                        // queuePlayUpcomingFlat(
                                                        // f - index - 1)
                                                        // (:429-441, trap 23).
                                                        onPlay: QbzQueue.queuePlayUpcomingFlat(
                                                            upSlot.index - root.cfDoc.index - 1)
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                QbzScrollBar {
                                    target: upFlick
                                    anchors.right: parent.right
                                    anchors.top: parent.top
                                    anchors.bottom: parent.bottom
                                }
                            }
                        }
                    }

                    // Empty state — nothing playing and nothing upcoming
                    // (:459-476).
                    Column {
                        visible: !root.hasCurrentTrack() && root.upcomingCount === 0
                        anchors.centerIn: parent
                        width: parent.width
                        spacing: 8
                        Text {
                            width: parent.width
                            text: QbzSession.tr("Your queue is empty", QbzSession.trRev)
                            color: "#ccffffff"
                            font.pixelSize: 19
                            font.weight: Font.DemiBold // 600
                            horizontalAlignment: Text.AlignHCenter
                        }
                        Text {
                            width: parent.width
                            text: QbzSession.tr("Play an album or track to get started", QbzSession.trRev)
                            color: "#80ffffff"
                            font.pixelSize: 15
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.WordWrap
                        }
                    }
                }

                // ===== HISTORY tab =====
                Item {
                    anchors.fill: parent
                    visible: root.tab === 1

                    Column {
                        anchors.fill: parent
                        visible: root.historyRows().length > 0
                        spacing: 10

                        // Section header — parity with QueueSidebar
                        // "RECENTLY PLAYED" (:485-499).
                        Row {
                            id: histHeader
                            spacing: 8
                            SectionLabel {
                                text: QbzSession.tr("RECENTLY PLAYED", QbzSession.trRev)
                            }
                            SectionLabel {
                                text: "· " + root.historyRows().length
                                color: "#80ffffff"
                                font.weight: Font.Medium
                                font.letterSpacing: 0.4
                            }
                        }

                        Item {
                            width: parent.width
                            height: parent.height - histHeader.implicitHeight - 10
                            Flickable {
                                id: histFlick
                                anchors.fill: parent
                                contentWidth: width - 16
                                contentHeight: histList.implicitHeight
                                boundsBehavior: Flickable.StopAtBounds
                                clip: true
                                Column {
                                    id: histList
                                    width: histFlick.contentWidth
                                    spacing: 4
                                    // history is already most-recent-first,
                                    // and queuePlayHistory(i) indexes it
                                    // directly — no coverflow math here
                                    // (:512-518).
                                    Repeater {
                                        model: root.historyRows()
                                        delegate: ImmQueueRow {
                                            required property int index
                                            required property var modelData
                                            item: modelData
                                            artSource: root.coverFor(modelData)
                                            onPlay: QbzQueue.queuePlayHistory(index)
                                        }
                                    }
                                }
                            }
                            QbzScrollBar {
                                target: histFlick
                                anchors.right: parent.right
                                anchors.top: parent.top
                                anchors.bottom: parent.bottom
                            }
                        }
                    }

                    Column {
                        visible: root.historyRows().length === 0
                        anchors.centerIn: parent
                        width: parent.width
                        spacing: 8
                        Text {
                            width: parent.width
                            text: QbzSession.tr("Nothing played yet", QbzSession.trRev)
                            color: "#ccffffff"
                            font.pixelSize: 19
                            font.weight: Font.DemiBold
                            horizontalAlignment: Text.AlignHCenter
                        }
                        Text {
                            width: parent.width
                            text: QbzSession.tr("Tracks you play will appear here", QbzSession.trRev)
                            color: "#80ffffff"
                            font.pixelSize: 15
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.WordWrap
                        }
                    }
                }
            }
        }
    }
}
