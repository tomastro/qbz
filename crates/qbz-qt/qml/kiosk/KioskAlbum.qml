// Album header and a recycled track viewport. Playback actions remain album-context actions.
import QtQuick
import com.blitzfc.qbz
import "../theme"
import "../controls"

Rectangle {
    id: root
    color: "transparent"
    QbzTheme { id: theme }
    readonly property var albumDoc: { try { return JSON.parse(QbzAlbum.albumJson) } catch(e) { return ({}) } }
    readonly property var header: albumDoc.header || ({})
    readonly property var tracks: albumDoc.tracks || []
    readonly property bool contentFocus: QbzKioskNav.navActive && QbzKioskNav.zone === "content"
    function publishNav() { QbzKioskNav.publishNav(0, 1, tracks.length, false) }
    function playTrack(id) { QbzPlayer.playAlbumFrom(header.id || "", id) }
    onTracksChanged: Qt.callLater(publishNav)
    Component.onCompleted: publishNav()
    property real negativeRestore: NaN
    function restoreHeader() {
        if (isFinite(negativeRestore) && !QbzAlbum.albumLoading && header.id) {
            list.contentY = negativeRestore
            negativeRestore = NaN
        }
    }
    onHeaderChanged: Qt.callLater(restoreHeader)
    Connections {
        target: QbzAlbum
        function onAlbumLoadingChanged() {
            Qt.callLater(root.restoreHeader)
        }
    }
    KioskNavigation {
        route: "album"
        snapshot: ({ headerY: list.contentY <= 0 ? list.contentY : null })
        onRestore: function(saved) {
            if (typeof saved.headerY === "number" && saved.headerY <= 0) {
                root.negativeRestore = saved.headerY
                Qt.callLater(root.restoreHeader)
            }
        }
    }
    Connections {
        target: QbzKioskNav
        function onIndexChanged() { if (root.contentFocus) list.positionViewAtIndex(QbzKioskNav.index, ListView.Contain) }
        function onActivateSeqChanged() {
            var i = QbzKioskNav.index
            if (root.contentFocus && i >= 0 && i < root.tracks.length) root.playTrack(root.tracks[i].id || "")
        }
    }
    ListView {
        id: list
        anchors.fill: parent
        leftMargin: 16; rightMargin: 16; topMargin: 16; bottomMargin: 16
        clip: true; cacheBuffer: 0; reuseItems: true; spacing: 2
        boundsBehavior: Flickable.StopAtBounds
        model: root.tracks
        header: Item {
            width: list.width - 32
            height: Math.max(180, headerInfo.implicitHeight) + 22
            Rectangle {
                id: coverTile
                width: Math.min(180, list.width * 0.25); height: width
                radius: theme.radiusMd; color: theme.surfaceElevated
                KioskCoverSource { id: cover; remote: root.header.artUrl || ""; local: root.header.artPath || ""; edge: coverTile.width }
                KioskArtwork { anchors.fill: parent; source: cover.source; radius: theme.radiusMd }
            }
            Column {
                id: headerInfo
                anchors.left: coverTile.right; anchors.leftMargin: 16; anchors.right: parent.right
                spacing: 8
                Text {
                    width: parent.width; text: root.header.title || ""
                    color: theme.textPrimary; font.pixelSize: 25; font.weight: theme.weightBold
                    wrapMode: Text.WordWrap; maximumLineCount: 2; elide: Text.ElideRight
                }
                Item {
                    width: parent.width; height: 44
                    Text { anchors.fill: parent; verticalAlignment: Text.AlignVCenter; text: root.header.artist || ""; color: theme.textSecondary; font.pixelSize: 17; elide: Text.ElideRight }
                    MouseArea { anchors.fill: parent; onClicked: QbzArtist.openArtist(root.header.artistId || "") }
                }
                // Action row (owner feedback 2026-09-07): icon buttons, not
                // text; Play/Shuffle plus the three queue insertions
                // (play next / play later / add to queue), all the SAME height;
                // and the quality badge floats inline at the right of the same
                // row instead of owning a line above it.
                Item {
                    width: parent.width
                    height: 48
                    Row {
                        id: albumActions
                        anchors.left: parent.left
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 8
                        QbzIconButton { name: "play-fill"; btnSize: 48; iconSize: 22; activeBackground: true; active: true; onClicked: QbzPlayer.playAlbum(root.header.id || "") }
                        QbzIconButton { name: "shuffle"; btnSize: 48; iconSize: 20; active: QbzPlayer.npShuffle; onClicked: QbzPlayer.playAlbumShuffled(root.header.id || "") }
                        QbzIconButton { name: "list-start"; btnSize: 48; iconSize: 20; onClicked: QbzPlayer.enqueueAlbum(root.header.id || "", "next") }
                        QbzIconButton { name: "list-plus"; btnSize: 48; iconSize: 20; onClicked: QbzPlayer.enqueueAlbum(root.header.id || "", "later") }
                        QbzIconButton { name: "list-end"; btnSize: 48; iconSize: 20; onClicked: QbzPlayer.enqueueAlbum(root.header.id || "", "queue") }
                    }
                    QualityBadgeFull {
                        readonly property string rawTier: root.header.qualityTier || ""
                        visible: tier !== "" || detail !== ""
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        tier: rawTier === "max" ? "hires" : rawTier === "lossy" ? "mp3" : rawTier
                        detail: root.header.qualityDetail || ""
                        showIcon: true
                        scaleFactor: 1.15
                    }
                }
            }
        }
        delegate: Rectangle {
            id: row
            required property var modelData
            required property int index
            width: list.width - 32; height: 64; radius: theme.radiusSm
            readonly property bool current: QbzPlayer.npTrackId === (modelData.id || "")
            readonly property bool navFocused: root.contentFocus && QbzKioskNav.index === index
            // Zebra striping (owner feedback 2026-09-07): odd rows carry a faint
            // fill so a long tracklist reads as banded rows. Current/nav/focus
            // states win over the stripe.
            color: navFocused ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.18)
                : current ? theme.surfaceElevated
                : (index % 2 === 1 ? theme.surfaceHover : "transparent")
            border.width: navFocused ? 2 : 0; border.color: theme.accent
            Text { id: number; x: 8; width: 34; height: parent.height; text: modelData.number || (index + 1); color: theme.textMuted; font.pixelSize: 14; verticalAlignment: Text.AlignVCenter; horizontalAlignment: Text.AlignRight }
            Text { id: duration; anchors.right: parent.right; anchors.rightMargin: 12; height: parent.height; text: modelData.duration || ""; color: theme.textMuted; font.pixelSize: 13; verticalAlignment: Text.AlignVCenter }
            Text { anchors.left: number.right; anchors.leftMargin: 14; anchors.right: duration.left; anchors.rightMargin: 14; height: parent.height; text: modelData.title || ""; color: row.current ? theme.accent : theme.textPrimary; font.pixelSize: 16; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
            MouseArea { anchors.fill: parent; onClicked: root.playTrack(modelData.id || "") }
        }
    }
    ScrollMemory { target: list; scope: "album" }
    KioskSkeleton {
        anchors.fill: parent; kind: "detail"
        loading: QbzAlbum.albumLoading && root.tracks.length === 0
        error: root.albumDoc.error || ""
        empty: !QbzAlbum.albumLoading && !root.header.id && root.tracks.length === 0
    }
}
