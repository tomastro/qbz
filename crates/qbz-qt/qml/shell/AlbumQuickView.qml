// Album Quick View — the app-wide, navigation-free preview opened from the
// left half of AlbumCard's top-right button group.
//
// The 80% × 75% rule is a CEILING, not a requested size: `card` measures the
// longest visible title and the number of tracks, shrinks for compact albums,
// and only reaches those limits when content needs the room. The track body is
// the sole scrolling region.
//
// Data lives on QbzAlbum.quickViewJson, separate from AlbumView's progressive
// document. The controller generation-guards opens and closes, so a late
// response cannot repaint another album or reopen a dismissed modal.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    id: root

    anchors.fill: parent
    visible: root.doc.open === true
    enabled: visible

    QbzTheme { id: theme }

    readonly property var doc: {
        try {
            return JSON.parse(QbzAlbum.quickViewJson || "{}")
        } catch (e) {
            return ({})
        }
    }
    readonly property var tracks: root.doc.tracks || []
    readonly property bool loading: root.doc.loading === true
    readonly property bool failed: root.doc.failed === true
    readonly property bool localAlbum: root.doc.isLocal === true
    readonly property string qualityTierLabel: root.doc.qualityTier === "hires" ? "HI-RES"
        : (root.doc.qualityTier === "mp3" ? "MP3"
        : (root.doc.qualityTier === "lossless" ? "LOSSLESS"
        : (root.doc.qualityTier === "" || root.doc.qualityTier === undefined ? "" : "CD")))
    readonly property string qualityTooltip: root.doc.qualityDetail
        ? root.qualityTierLabel + ": " + root.doc.qualityDetail : ""
    property var contextTrack: ({})

    readonly property int rowHeight: 40
    readonly property int numberWidth: 48
    readonly property int durationWidth: 84
    readonly property int contextWidth: 72
    readonly property int tableLeft: 18
    // 14px house scrollbar plus breathing room: the row's context button never
    // sits below the thumb.
    readonly property int tableRight: 32

    function tr(text) {
        return QbzSession.tr(text, QbzSession.trRev)
    }

    function longestTrackTitle() {
        var longest = ""
        for (var i = 0; i < root.tracks.length; i++) {
            var candidate = root.tracks[i].title || ""
            if (candidate.length > longest.length)
                longest = candidate
        }
        return longest
    }

    TextMetrics {
        id: albumTitleMetrics
        text: root.doc.title || root.tr("Loading album…")
        font.pixelSize: theme.fontSection
        font.weight: theme.weightSemibold
    }
    TextMetrics {
        id: artistMetrics
        text: root.doc.artist || ""
        font.pixelSize: theme.fontBody
    }
    TextMetrics {
        id: trackTitleMetrics
        text: root.longestTrackTitle()
        font.pixelSize: theme.fontBody - 1
        font.weight: theme.weightMedium
    }

    readonly property real preferredCardWidth: Math.max(520, Math.min(820,
        Math.max(albumTitleMetrics.advanceWidth + 120,
                 artistMetrics.advanceWidth + 120,
                 trackTitleMetrics.advanceWidth + root.numberWidth
                    + root.durationWidth + root.contextWidth + 92)))
    readonly property real bodyPreferredHeight: (root.loading || root.failed
        || root.tracks.length === 0) ? 112 : root.tracks.length * root.rowHeight
    // one 76px inline header + divider 1 + body + 10px bottom breathing room.
    // Never turn either maximum into a minimum.
    readonly property real preferredCardHeight: 87 + root.bodyPreferredHeight

    function closeQuickView() {
        albumMenu.close()
        trackMenu.close()
        QbzAlbum.closeQuickView()
    }

    function restoreShellFocus() {
        var node = root
        while (node.parent) {
            if (node.parent.isQbzShellRoot === true) {
                node.parent.forceActiveFocus()
                return
            }
            node = node.parent
        }
    }

    function playTrack(trackId) {
        if (!trackId)
            return
        if (root.localAlbum)
            QbzAlbum.quickViewLocalAction("play", trackId)
        else
            QbzPlayer.playAlbumFrom(root.doc.albumId || "", trackId)
    }

    function enqueueTrack(trackId, mode) {
        if (!trackId)
            return
        if (root.localAlbum)
            QbzAlbum.quickViewLocalAction(mode, trackId)
        else
            QbzPlayer.enqueueAlbumTrack(root.doc.albumId || "", trackId, mode)
    }

    function albumPlaybackAction(action) {
        if (root.localAlbum) {
            QbzAlbum.quickViewLocalAction(action, "")
        } else if (action === "play") {
            QbzPlayer.playAlbum(root.doc.albumId || "")
        } else if (action === "shuffle") {
            QbzPlayer.playAlbumShuffled(root.doc.albumId || "")
        } else {
            QbzPlayer.enqueueAlbum(root.doc.albumId || "", action)
        }
    }

    function openTrackMenu(track, anchor, x, y) {
        if (!track || !track.id)
            return
        root.contextTrack = track
        trackMenu.entries = root.trackMenuEntries(track)
        if (trackMenu.entries.length > 0)
            trackMenu.openAtCursor(anchor, x, y)
    }

    function albumMenuEntries() {
        var t = root.tr
        var entries = [
            { "label": t("Open album"), "icon": "library-big", "action": "open" },
            { "label": t("Play"), "icon": "play-fill", "action": "play" },
            { "label": t("Play next"), "icon": "list-start", "action": "next" }
        ]
        // Both paths own a real block-tail operation now — the QoL round gave
        // playback_qt::enqueue_album a "later" (#442) arm to match the local
        // queue's, so the row is unconditional.
        entries.push({ "label": t("Play later"), "icon": "list-plus", "action": "later" })
        entries.push({ "label": t("Add to queue"), "icon": "list-end", "action": "queue" })
        // "Add to playlist" is for BOTH paths (owner, 2026-08-31: playlists
        // can be 100% local or mixed — the local arm rides the quick view's
        // own row snapshot). Mixtape/share stay catalog-only.
        entries.push({ "sep": true })
        entries.push({ "label": t("Add to playlist"), "icon": "list-music", "action": "playlist" })
        if (!root.localAlbum) {
            entries.push({ "label": t("Add to mixtape"), "icon": "cassette-tape", "action": "mixtape" })
            entries.push({ "label": t("Share Qobuz link"), "icon": "link", "action": "share-qobuz" })
            entries.push({ "label": t("Share Album.link"), "icon": "link", "action": "share-albumlink" })
        }
        return entries
    }

    function trackMenuEntries(track) {
        var t = root.tr
        var entries = []
        if (track.available !== false) {
            entries.push({ "label": t("Play now"), "icon": "play-fill", "action": "play" })
            entries.push({ "label": t("Play next"), "icon": "list-start", "action": "next" })
            entries.push({ "label": t("Play later"), "icon": "list-plus", "action": "later" })
            entries.push({ "label": t("Add to queue"), "icon": "list-end", "action": "queue" })
            entries.push({ "sep": true })
            entries.push({ "label": t("Add to playlist"), "icon": "list-music", "action": "playlist" })
        }
        // Quick View deliberately has no favourite or offline-cache actions,
        // inline or in this menu. Context stays about playback/navigation.
        if (track.available !== false && !root.localAlbum) {
            entries.push({ "label": t("Share Qobuz link"), "icon": "link", "action": "share" })
            entries.push({ "label": t("Track info"), "icon": "info", "action": "info" })
        }
        return entries
    }

    function albumMenuAction(action) {
        var albumId = root.doc.albumId || ""
        if (albumId === "")
            return
        if (action === "open") {
            root.closeQuickView()
            QbzAlbum.openAlbum(albumId)
        } else if (action === "play") {
            root.albumPlaybackAction("play")
        } else if (action === "next" || action === "later") {
            root.albumPlaybackAction(action)
        } else if (action === "queue") {
            // Album enqueue has no block-tail mode; the canonical AlbumView
            // menu therefore exposes one append action, not a duplicate
            // Play-later row that would perform the same write.
            root.albumPlaybackAction(root.localAlbum ? "queue" : "later")
        } else if (action === "playlist") {
            if (root.localAlbum) {
                // Rust still holds this quick view's row snapshot — fire the
                // action BEFORE closing (close bumps the generation and the
                // context would be refused as stale).
                QbzAlbum.quickViewLocalAction("playlist", "")
                root.closeQuickView()
                return
            }
            var ids = []
            for (var i = 0; i < root.tracks.length; i++) {
                if (/^\d+$/.test(root.tracks[i].id || "")
                        && root.tracks[i].available !== false)
                    ids.push(root.tracks[i].id)
            }
            if (ids.length > 0) {
                root.closeQuickView()
                QbzPlaylistPicker.openForTracks(JSON.stringify(ids))
            }
        } else if (action === "mixtape") {
            root.closeQuickView()
            QbzAlbum.addToMixtape(albumId)
        } else if (action === "share-qobuz") {
            QbzAlbum.shareQobuzLink(albumId)
        } else if (action === "share-albumlink") {
            QbzAlbum.shareAlbumLink(albumId)
        }
    }

    function trackMenuAction(action) {
        var track = root.contextTrack || ({})
        if (!track.id)
            return
        if (action === "play") root.playTrack(track.id)
        else if (action === "next" || action === "later" || action === "queue")
            root.enqueueTrack(track.id, action)
        else if (action === "playlist") {
            if (root.localAlbum) {
                // Same before-close ordering as the album arm above.
                QbzAlbum.quickViewLocalAction("playlist", String(track.id))
                root.closeQuickView()
                return
            }
            root.closeQuickView()
            QbzPlaylistPicker.openForTrack(track.id)
        } else if (action === "share") {
            QbzAlbum.shareTrackQobuz(track.id)
        } else if (action === "info") {
            // TrackInfo reparents into Overlay.overlay and returns here on
            // close. Keeping this document open also keeps its owning object
            // visible while the child popup is active.
            trackInfo.openFor(track.id)
        }
    }

    onVisibleChanged: {
        if (visible)
            keyScope.forceActiveFocus()
        else
            root.restoreShellFocus()
    }
    FocusScope {
        id: keyScope
        anchors.fill: parent
        Keys.onEscapePressed: root.closeQuickView()
    }

    Rectangle {
        anchors.fill: parent
        color: "#bf000000"
        MouseArea {
            anchors.fill: parent
            onClicked: root.closeQuickView()
            // A plain Item overlay has no Popup modal grab: consume wheel
            // events explicitly so the page behind it cannot scroll.
            onWheel: function (wheel) { wheel.accepted = true }
        }
    }

    Rectangle {
        id: card
        width: Math.min(root.width * 0.8, root.preferredCardWidth)
        height: Math.min(root.height * 0.75, root.preferredCardHeight)
        anchors.centerIn: parent
        radius: theme.radiusMd
        color: theme.surfaceCard
        border.width: 1
        border.color: theme.borderSubtle
        clip: true

        // Swallow all unused card-space input before it reaches the backdrop
        // or the page below. The ListView, declared later, keeps its own wheel.
        MouseArea {
            anchors.fill: parent
            onWheel: function (wheel) { wheel.accepted = true }
        }

        Item {
            id: heading
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            height: 76

            Column {
                anchors.left: parent.left
                anchors.leftMargin: 20
                anchors.right: headerActions.left
                anchors.rightMargin: 20
                anchors.verticalCenter: parent.verticalCenter
                spacing: 4
                Text {
                    width: parent.width
                    text: root.loading ? root.tr("Loading album…") : (root.doc.title || "")
                    color: theme.textPrimary
                    font.pixelSize: theme.fontSection
                    font.weight: theme.weightSemibold
                    elide: Text.ElideRight
                }
                Text {
                    visible: !root.loading && (root.doc.artist || "") !== ""
                    width: parent.width
                    text: root.doc.artist || ""
                    color: theme.textMuted
                    font.pixelSize: theme.fontBody
                    elide: Text.ElideRight
                }
                // NPB Small's condensed, inline quality line — shared with
                // AudioStamp rather than the stacked QualityBadgeFull chip.
                // A physical album adds its real SourceIcon immediately beside
                // it; catalog albums keep the line unchanged.
                Row {
                    visible: !root.loading && (root.qualityTierLabel !== ""
                        || (root.localAlbum && (root.doc.source || "") !== ""))
                    spacing: 8
                    QualityInline {
                        visible: root.qualityTierLabel !== ""
                        maxWidth: Math.max(0, parent.parent.width
                            - (sourceIndicator.visible
                                ? sourceIndicator.width + parent.spacing : 0))
                        tierLabel: root.qualityTierLabel
                        detail: root.doc.qualityDetail || ""
                        tooltipText: root.qualityTooltip
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    SourceIcon {
                        id: sourceIndicator
                        visible: root.localAlbum && (root.doc.source || "") !== ""
                        kind: root.doc.source || "local"
                        mono: true
                        localTint: "muted"
                        anchors.verticalCenter: parent.verticalCenter
                    }
                }
            }

            // One compact inline group: album transport, a visual separator,
            // then the modal-only circular close action.
            Row {
                id: headerActions
                anchors.right: parent.right
                anchors.rightMargin: 20
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8

                QbzCircleAction {
                    primary: true
                    compactPrimary: true
                    diameterOverride: 28
                    name: "play-fill"
                    btnEnabled: !root.loading && !root.failed && root.tracks.length > 0
                    anchors.verticalCenter: parent.verticalCenter
                    onClicked: root.albumPlaybackAction("play")
                }
                QbzCircleAction {
                    name: "shuffle"
                    diameterOverride: 28
                    btnEnabled: !root.loading && !root.failed && root.tracks.length > 0
                    anchors.verticalCenter: parent.verticalCenter
                    onClicked: root.albumPlaybackAction("shuffle")
                }
                QbzCircleAction {
                    id: albumContextButton
                    name: "ellipsis"
                    diameterOverride: 28
                    btnEnabled: !root.loading && !root.failed && root.tracks.length > 0
                    anchors.verticalCenter: parent.verticalCenter
                    onClicked: function (mouse) {
                        albumMenu.entries = root.albumMenuEntries()
                        albumMenu.openAtCursor(albumContextButton,
                            mouse ? mouse.x : width / 2,
                            mouse ? mouse.y : height / 2)
                    }
                }
                Rectangle {
                    width: 1
                    height: 20
                    color: theme.borderStrong
                    anchors.verticalCenter: parent.verticalCenter
                }
                QbzCircleAction {
                    id: closeButton
                    name: "x"
                    diameterOverride: 28
                    anchors.verticalCenter: parent.verticalCenter
                    onClicked: root.closeQuickView()
                    HoverHandler { id: closeHover }
                    ToolTip.visible: closeHover.hovered
                    ToolTip.text: root.tr("Close")
                    ToolTip.delay: 350
                }
            }
        }

        Rectangle {
            id: divider
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: heading.bottom
            height: 1
            color: theme.borderSubtle
        }

        ListView {
            id: trackList
            visible: !root.loading && !root.failed && root.tracks.length > 0
            anchors.left: parent.left
            anchors.leftMargin: root.tableLeft
            anchors.right: parent.right
            anchors.rightMargin: root.tableRight
            anchors.top: divider.bottom
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 10
            clip: true
            reuseItems: true
            boundsBehavior: Flickable.StopAtBounds
            model: root.tracks

            delegate: Rectangle {
                id: trackRow
                required property var modelData
                required property int index
                width: ListView.view.width
                height: root.rowHeight
                radius: theme.radiusSm
                color: rowArea.containsMouse || numberArea.containsMouse
                        || contextArea.containsMouse
                    ? theme.surfaceHover
                    : (index % 2 === 1 ? theme.alphaTier(4) : "transparent")
                opacity: modelData.available === false ? 0.45 : 1.0

                MouseArea {
                    id: rowArea
                    anchors.fill: parent
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    hoverEnabled: true
                    onClicked: function (mouse) {
                        if (mouse.button === Qt.RightButton)
                            root.openTrackMenu(trackRow.modelData, rowArea,
                                               mouse.x, mouse.y)
                    }
                    onDoubleClicked: function (mouse) {
                        if (mouse.button === Qt.LeftButton
                                && trackRow.modelData.available !== false)
                            root.playTrack(trackRow.modelData.id)
                    }
                }

                Row {
                    anchors.fill: parent
                    Item {
                        id: numberCell
                        width: root.numberWidth
                        height: parent.height
                        Text {
                            anchors.centerIn: parent
                            visible: !numberArea.containsMouse
                                || trackRow.modelData.available === false
                            text: trackRow.modelData.number || (trackRow.index + 1)
                            color: theme.textMuted
                            font.pixelSize: theme.fontLegal
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            visible: numberArea.containsMouse
                                && trackRow.modelData.available !== false
                            width: 26
                            height: 26
                            radius: 13
                            color: numberArea.pressed
                                ? theme.accent : theme.accentHover
                            QbzIcon {
                                anchors.centerIn: parent
                                name: "play-fill"
                                width: 13
                                height: 13
                                tintName: theme.accentGlyphTint
                            }
                        }
                        // A permanent hit region owns the entire number cell.
                        // The old MouseArea existed only while its hover-made
                        // button was visible, which repeatedly stole/released
                        // hover from rowArea and made the button flicker.
                        MouseArea {
                            id: numberArea
                            anchors.fill: parent
                            acceptedButtons: Qt.LeftButton | Qt.RightButton
                            hoverEnabled: true
                            cursorShape: trackRow.modelData.available !== false
                                ? Qt.PointingHandCursor : Qt.ArrowCursor
                            onClicked: function (mouse) {
                                if (mouse.button === Qt.RightButton) {
                                    root.openTrackMenu(trackRow.modelData,
                                                       numberArea,
                                                       mouse.x, mouse.y)
                                } else if (trackRow.modelData.available !== false) {
                                    root.playTrack(trackRow.modelData.id)
                                }
                            }
                        }
                    }
                    Text {
                        width: Math.max(0, parent.width - root.numberWidth
                            - root.durationWidth - root.contextWidth)
                        height: parent.height
                        text: trackRow.modelData.title || ""
                        color: theme.textPrimary
                        font.pixelSize: theme.fontBody - 1
                        font.weight: theme.weightMedium
                        verticalAlignment: Text.AlignVCenter
                        elide: Text.ElideRight
                    }
                    Text {
                        width: root.durationWidth
                        height: parent.height
                        text: trackRow.modelData.duration || ""
                        color: theme.textMuted
                        font.pixelSize: theme.fontLegal
                        verticalAlignment: Text.AlignVCenter
                        horizontalAlignment: Text.AlignHCenter
                    }
                    Item {
                        width: root.contextWidth
                        height: parent.height
                        Rectangle {
                            anchors.centerIn: parent
                            width: 28
                            height: 28
                            radius: theme.radiusSm
                            color: contextArea.containsMouse
                                ? theme.surfaceElevated : "transparent"
                            QbzIcon {
                                anchors.centerIn: parent
                                name: "ellipsis"
                                width: 15
                                height: 15
                                tintName: contextArea.containsMouse
                                    ? "textPrimary" : "muted"
                            }
                            MouseArea {
                                id: contextArea
                                anchors.fill: parent
                                enabled: root.trackMenuEntries(trackRow.modelData).length > 0
                                hoverEnabled: true
                                cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                                onClicked: function (mouse) {
                                    root.openTrackMenu(trackRow.modelData,
                                                       contextArea,
                                                       mouse.x, mouse.y)
                                }
                            }
                        }
                    }
                }
            }
        }

        QbzScrollBar {
            target: trackList
            visible: trackList.visible && trackList.contentHeight > trackList.height
            anchors.right: parent.right
            anchors.rightMargin: 4
            anchors.top: divider.bottom
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 10
        }

        Column {
            visible: root.loading
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.verticalCenter: trackList.verticalCenter
            spacing: 14
            QbzLoadingDots {
                width: implicitWidth
                height: implicitHeight
                anchors.horizontalCenter: parent.horizontalCenter
                phase: Math.floor(QbzShell.pulseMs / 300) % 3
            }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: root.tr("Loading album…")
                color: theme.textMuted
                font.pixelSize: theme.fontBody
            }
        }

        QbzEmptyState {
            visible: !root.loading && (root.failed || root.tracks.length === 0)
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.verticalCenter: trackList.verticalCenter
            iconName: root.failed ? "triangle-alert" : "list-music"
            iconSize: 30
            iconOpacity: 0.55
            titleMuted: true
            titleWeight: theme.weightMedium
            title: root.failed ? root.tr("Could not load album")
                               : root.tr("This album has no tracks.")
        }
    }

    CardMenu {
        id: albumMenu
        menuWidth: 224
        onPicked: function (action) { root.albumMenuAction(action) }
    }
    CardMenu {
        id: trackMenu
        menuWidth: 224
        onPicked: function (action) { root.trackMenuAction(action) }
    }

    // Instantiated once, not once per row. The info popup reparents to the
    // window overlay; keeping Quick View alive gives it a valid return target.
    TrackInfoModal { id: trackInfo }
}
