// Local Library > the open session ("Open" — a folder outside the index, a CD,
// a SACD image), kiosk body.
//
// WHY NOT views/local/LocalEphemeralPane.qml. The desktop pane was evaluated
// for reuse first, per the hardening brief, and it does not fit:
//   - it nests an unbounded `Repeater` over the session's albums inside which
//     a second unbounded `Repeater` mounts a desktop `TrackRow` per track. The
//     owner's own reference case is a 247-track box set, so the pane mounts
//     247 desktop rows — each with a menu, a favourite, a download affordance
//     and a drag handler — to show six;
//   - it is laid out at desktop density (a 224px header cover, 32px page
//     inset, hover tooltips) and its only Close is a 28px circle;
//   - it duck-types a `view` that must answer `artPathOf`, `artWanted`,
//     `skelPhase`, `artSettleMs`, `artPulse`, `reportEphemeralWindow` and
//     `releaseWindow` — the desktop LocalLibraryView's own private surface.
// So this is the kiosk presentation of the SAME document and the SAME actions
// (`ephemeralPlayAll`, `ephemeralPlayTrack`, `ephemeralClear`): one windowing
// ListView over the session flattened to album bands + 64px track rows.
//
// LIFECYCLE is the desktop's, unchanged: closing the session drops this tab
// and the host falls back to Albums, and a closed session is never resurrected
// by Back/Forward (the host refuses a restored `ephemeral` tab when no session
// is active).

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    id: root

    property var view: null

    QbzTheme { id: theme }

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    readonly property var session: root.view ? root.view.ephemeral : null
    readonly property var albums: root.session && root.session.albums
        ? root.session.albums : []
    readonly property bool multiAlbum: root.session
        && root.session.multiAlbum === true
    readonly property int trackTotal: root.session && root.session.trackCount
        ? root.session.trackCount : 0

    /// The session flattened to ONE list: an album band per block (only when
    /// the session holds more than one), then that block's tracks. Rebuilt
    /// once per document, never per frame.
    readonly property var entries: {
        var out = []
        var blocks = root.albums
        for (var i = 0; i < blocks.length; i++) {
            var block = blocks[i]
            if (root.multiAlbum)
                out.push({ "t": 0, "block": block })
            var tracks = block.tracks || []
            for (var j = 0; j < tracks.length; j++)
                out.push({ "t": 1, "track": tracks[j], "block": block })
        }
        return out
    }

    readonly property real rowH: 64
    readonly property real bandH: 56
    readonly property real pad: root.view ? root.view.pad : 16

    function publishNav() {
        if (root.view)
            root.view.publishNav(1, root.entries.length)
    }
    onEntriesChanged: root.publishNav()

    // ---------------------------------------------------------------------
    // Artwork follows the visible entry band plus one row on either side.
    // A multi-album folder can be arbitrarily large; its whole cover set must
    // never be requested merely because the session opened.
    // ---------------------------------------------------------------------
    function report() {
        if (!root.view || !list) return
        if (!root.visible || root.albums.length === 0) {
            root.view.releaseWindow("ephemeral")
            return
        }
        var resident = [], seen = ({})
        function keep(album) {
            if (album && album.artKey && !seen[album.artKey]) {
                seen[album.artKey] = true
                resident.push(album)
            }
        }
        if (list.contentY < 0) keep(root.albums[0])
        var first = list.indexAt(4, list.contentY + 1)
        var last = list.indexAt(4, list.contentY + Math.max(1, list.height) - 1)
        if (first < 0) first = 0
        if (last < 0) last = first + Math.ceil(list.height / root.rowH)
        first = Math.max(0, first - 1)
        last = Math.min(root.entries.length - 1, last + 1)
        for (var i = first; i <= last; i++) keep(root.entries[i].block)
        root.view.queueWindowReport(resident, 0, resident.length - 1, "ephemeral")
    }
    onAlbumsChanged: root.report()
    onVisibleChanged: root.report()
    Component.onCompleted: {
        root.report()
        root.publishNav()
    }
    Component.onDestruction: if (root.view) root.view.releaseWindow("ephemeral")
    Connections {
        target: root.view
        function onArtworkRefresh() { root.report() }
    }

    function coverOf(block) {
        if (!block || !root.view)
            return ""
        return root.view.artPathOf(block.artKey)
    }

    // ---------------------------------------------------------------------
    // Focus. The index space is the flattened entries; a band does nothing on
    // Enter, exactly as the A-Z bands in the Artists list do.
    // ---------------------------------------------------------------------
    readonly property int focusedItem: QbzKioskNav.index - QbzKioskNav.tabs
    readonly property bool itemFocused: QbzKioskNav.navActive
        && QbzKioskNav.zone === "content"
        && root.focusedItem >= 0
        && root.focusedItem < root.entries.length

    Connections {
        target: QbzKioskNav
        function onIndexChanged() {
            if (root.itemFocused)
                Qt.callLater(function () {
                    list.positionViewAtIndex(root.focusedItem, ListView.Contain)
                })
        }
        function onActivateSeqChanged() {
            if (!root.itemFocused)
                return
            var entry = root.entries[root.focusedItem]
            if (entry && entry.t === 1 && entry.track)
                QbzLocal.ephemeralPlayTrack(entry.track.id)
        }
    }

    // =====================================================================
    // Header — the session, and the three actions it has.
    // =====================================================================


    // =====================================================================
    // The session's tracks
    // =====================================================================
    ListView {
        id: list
        header: Item {
        id: header
        width: list.width
        // 8 + a 64px identity row + 12 + a 64px action row + 8. Written out
        // because both rows are fixed-height touch targets and the block must
        // not be able to overlap itself at 480px.
        height: 156

        Row {
            id: identity
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.topMargin: 8
            height: 64
            spacing: 12

            Rectangle {
                id: cover
                width: 64
                height: 64
                radius: theme.radiusSm
                color: theme.surfaceElevated
                clip: true

                KioskArtwork {
                    anchors.fill: parent
                    radius: theme.radiusSm
                    fit: "crop"
                    source: root.albums.length > 0 ? root.coverOf(root.albums[0]) : ""
                }
            }

            Column {
                width: parent.width - 64 - 12
                spacing: 4

                Text {
                    width: parent.width
                    text: QbzLocal.localEphemeralLabel
                    color: theme.textPrimary
                    font.pixelSize: 18
                    font.weight: theme.weightBold
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }
                Text {
                    width: parent.width
                    text: root.trackTotal + " " + root.t("tracks")
                    color: theme.textMuted
                    font.pixelSize: 13
                    elide: Text.ElideRight
                }
            }
        }

        // Play / Shuffle / Close. 64px tall, the contract's primary target,
        // and Close is deliberately the last one in the row rather than
        // between the two play actions.
        Row {
            anchors.left: parent.left
            anchors.top: identity.bottom
            anchors.topMargin: 12
            height: 64
            spacing: 10

            Repeater {
                model: [
                    { "id": "play", "label": root.t("Play all") },
                    { "id": "shuffle", "label": root.t("Shuffle") },
                    { "id": "close", "label": root.t("Close") }
                ]

                delegate: Rectangle {
                    id: action
                    required property var modelData

                    width: Math.max(96, actionText.implicitWidth + 36)
                    height: 64
                    radius: theme.radiusSm
                    color: action.modelData.id === "play"
                        ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.18)
                        : theme.surfaceElevated
                    border.width: 1
                    border.color: action.modelData.id === "play"
                        ? theme.accent : theme.borderSubtle

                    Text {
                        id: actionText
                        anchors.centerIn: parent
                        text: action.modelData.label
                        color: theme.textPrimary
                        font.pixelSize: 15
                        font.weight: theme.weightSemibold
                    }

                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            if (action.modelData.id === "play")
                                QbzLocal.ephemeralPlayAll(false)
                            else if (action.modelData.id === "shuffle")
                                QbzLocal.ephemeralPlayAll(true)
                            else
                                QbzLocal.ephemeralClear()
                        }
                    }
                }
            }
        }
    }
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.topMargin: 8
        anchors.leftMargin: root.pad
        anchors.rightMargin: root.pad
        visible: !QbzLocal.localEphemeralLoading && root.entries.length > 0
        clip: true
        topMargin: 4
        bottomMargin: 8
        cacheBuffer: Math.min(root.rowH, Math.max(0, list.height / 2))
        boundsBehavior: Flickable.StopAtBounds
        reuseItems: true
        model: root.entries
        onContentYChanged: root.report()
        onCountChanged: Qt.callLater(root.report)
        onHeightChanged: root.report()

        delegate: Item {
            id: entrySlot
            required property var modelData
            required property int index

            readonly property bool band: entrySlot.modelData
                && entrySlot.modelData.t === 0

            width: list.width
            height: entrySlot.band ? root.bandH : root.rowH

            // Album band — only present in a multi-album session.
            Row {
                anchors.fill: parent
                anchors.topMargin: 8
                visible: entrySlot.band
                spacing: 10

                Rectangle {
                    width: 40
                    height: 40
                    radius: theme.radiusSm
                    color: theme.surfaceElevated
                    clip: true

                    KioskArtwork {
                        anchors.fill: parent
                        radius: theme.radiusSm
                        fit: "crop"
                        source: entrySlot.band
                            ? root.coverOf(entrySlot.modelData.block) : ""
                    }
                }
                Column {
                    width: parent.width - 40 - 10 - 56
                    spacing: 1

                    Text {
                        width: parent.width
                        text: entrySlot.band && entrySlot.modelData.block
                            ? (entrySlot.modelData.block.title || "") : ""
                        color: theme.textPrimary
                        font.pixelSize: 14
                        font.weight: theme.weightSemibold
                        elide: Text.ElideRight
                        maximumLineCount: 1
                    }
                    Text {
                        width: parent.width
                        text: entrySlot.band && entrySlot.modelData.block
                            ? (entrySlot.modelData.block.artist || "") : ""
                        color: theme.textMuted
                        font.pixelSize: 12
                        elide: Text.ElideRight
                        maximumLineCount: 1
                    }
                }
                // Per-album Play, which only a multi-album session offers.
                Rectangle {
                    width: 48
                    height: 40
                    radius: theme.radiusSm
                    color: theme.surfaceElevated
                    border.width: 1
                    border.color: theme.borderSubtle

                    QbzIcon {
                        anchors.centerIn: parent
                        name: "play-fill"
                        width: 18
                        height: 18
                        tintName: "textPrimary"
                    }
                    MouseArea {
                        anchors.fill: parent
                        anchors.margins: -8
                        onClicked: {
                            if (entrySlot.band && entrySlot.modelData.block)
                                QbzLocal.ephemeralPlayAlbum(
                                    entrySlot.modelData.block.groupKey)
                        }
                    }
                }
            }

            KioskTrackRow {
                width: parent.width
                height: root.rowH
                visible: !entrySlot.band
                track: entrySlot.band || !entrySlot.modelData
                    ? ({})
                    : ({
                        "id": entrySlot.modelData.track.id,
                        "title": entrySlot.modelData.track.title || "",
                        "artist": entrySlot.modelData.track.artist || "",
                        "duration": entrySlot.modelData.track.duration || "",
                        "artwork": ""
                    })
                navFocused: root.itemFocused && root.focusedItem === entrySlot.index
                onClicked: {
                    if (!entrySlot.band)
                        QbzLocal.ephemeralPlayTrack(entrySlot.modelData.track.id)
                }
            }
        }
    }

    ScrollMemory { target: list; scope: "local:ephemeral" }

    KioskSkeleton {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        kind: "list"
        rowHeight: root.rowH
        rowArtSize: 46
        pad: root.pad
        loading: QbzLocal.localEphemeralLoading
        empty: !QbzLocal.localEphemeralLoading && root.entries.length === 0
        emptyText: root.t("No results")
    }
}
