// Kiosk artist: bounded discography shelves; only mounted covers resolve.
import QtQuick
import com.blitzfc.qbz
import "../theme"
import "../controls"

Rectangle {
    id: root
    color: "transparent"
    QbzTheme { id: theme }
    readonly property var artist: { try { return JSON.parse(QbzArtist.artistJson) } catch(e) { return ({}) } }
    readonly property var sections: (artist.releaseSections || []).filter(function(s) { return (s.cards || []).length > 0 })
    property var cursors: ({})
    property var shelfPositions: ({})
    function saveShelf(key, value) { var next = Object.assign({}, shelfPositions); next[key] = value; shelfPositions = next }
    readonly property bool contentFocus: QbzKioskNav.navActive && QbzKioskNav.zone === "content"
    function publishNav() { QbzKioskNav.publishNav(0, 1, sections.length, true) }
    onSectionsChanged: Qt.callLater(publishNav)
    Component.onCompleted: publishNav()
    KioskNavigation {
        route: "artist"
        snapshot: ({ cursors: root.cursors, shelfPositions: root.shelfPositions, headerY: shelves.contentY <= 0 ? shelves.contentY : null })
        onRestore: function(saved) { root.cursors = saved.cursors || ({}); root.shelfPositions = saved.shelfPositions || ({}); if (typeof saved.headerY === "number" && saved.headerY <= 0) { root.negativeRestore = saved.headerY; Qt.callLater(root.restoreHeader) } }
    }
    property real negativeRestore: NaN
    function restoreHeader() {
        if (isFinite(negativeRestore) && !QbzArtist.artistLoading && artist.id) {
            shelves.contentY = negativeRestore
            negativeRestore = NaN
        }
    }
    onArtistChanged: Qt.callLater(restoreHeader)
    Connections {
        target: QbzArtist
        function onArtistLoadingChanged() {
            Qt.callLater(root.restoreHeader)
        }
    }
    Connections {
        target: QbzKioskNav
        function onIndexChanged() {
            if (root.contentFocus) shelves.positionViewAtIndex(QbzKioskNav.index, ListView.Contain)
        }
        function onHmoveChanged() {
            var i = QbzKioskNav.index
            if (!root.contentFocus || i < 0 || i >= root.sections.length || !QbzKioskNav.hmove) return
            var positions = Object.assign({}, root.cursors)
            positions[i] = Math.max(0, Math.min((positions[i] || 0) + QbzKioskNav.takeHmove(), root.sections[i].cards.length - 1))
            root.cursors = positions
        }
        function onActivateSeqChanged() {
            var i = QbzKioskNav.index
            if (!root.contentFocus || i < 0 || i >= root.sections.length) return
            var row = root.sections[i].cards[root.cursors[i] || 0]
            if (row) QbzAlbum.openAlbum(row.id)
        }
    }
    ListView {
        id: shelves
        anchors.fill: parent
        clip: true; spacing: 20; cacheBuffer: 0; reuseItems: true
        leftMargin: 16; rightMargin: 16; topMargin: 16; bottomMargin: 16
        boundsBehavior: Flickable.StopAtBounds
        model: root.sections
        header: Item {
            width: shelves.width - 32
            height: Math.max(164, info.implicitHeight + 24)
            Rectangle {
                id: portrait
                width: Math.min(140, shelves.width * 0.23); height: width
                radius: width / 2; color: theme.surfaceElevated
                KioskCoverSource { id: portraitSource; remote: root.artist.artUrl || ""; local: root.artist.artPath || ""; edge: portrait.width }
                KioskArtwork { anchors.fill: parent; source: portraitSource.source; radius: portrait.radius }
            }
            Column {
                id: info
                anchors.left: portrait.right; anchors.leftMargin: 16; anchors.right: parent.right
                spacing: 12
                Text {
                    width: parent.width; text: root.artist.name || ""
                    font.pixelSize: 25; font.weight: theme.weightBold; color: theme.textPrimary
                    wrapMode: Text.WordWrap; maximumLineCount: 2; elide: Text.ElideRight
                }
                Row {
                    spacing: 10
                    Repeater {
                        model: ["Play", "Radio"]
                        delegate: Rectangle {
                            required property string modelData
                            required property int index
                            width: Math.max(64, Math.min(130, (info.width - 10) / 2)); height: 64
                            radius: theme.radiusSm; color: index === 0 ? theme.accent : theme.surfaceElevated
                            Text { anchors.centerIn: parent; text: QbzSession.tr(modelData, QbzSession.trRev); color: index === 0 ? theme.accentText : theme.textPrimary; font.pixelSize: 16 }
                            MouseArea {
                                anchors.fill: parent
                                onClicked: {
                                    if (!root.artist.id) return
                                    if (index === 0) QbzHome.playArtistTopTracks(root.artist.id)
                                    else QbzHome.startArtistRadio(root.artist.id)
                                }
                            }
                        }
                    }
                }
            }
        }
        delegate: Column {
            required property var modelData
            required property int index
            width: shelves.width - 32; spacing: 12
            Text { width: parent.width; height: 28; text: modelData.title || ""; color: theme.textPrimary; font.pixelSize: 18; font.weight: theme.weightSemibold; elide: Text.ElideRight }
            KioskShelf {
                width: parent.width; rows: modelData.cards || []
                navFocused: root.contentFocus && QbzKioskNav.index === index
                cursor: root.cursors[index] || 0
                restoreX: root.shelfPositions[String(index)] || 0
                onScrollSettled: function(x) { root.saveShelf(String(index), x) }
                onOpen: function(id) { QbzAlbum.openAlbum(id) }
            }
        }
    }
    ScrollMemory { target: shelves; scope: "artist" }
    KioskSkeleton {
        anchors.fill: parent; kind: "detail"
        loading: QbzArtist.artistLoading && !root.artist.id
        error: root.artist.error || ""
        empty: !QbzArtist.artistLoading && !root.artist.id && root.sections.length === 0
    }
}
