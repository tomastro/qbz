// One horizontal viewport. Recycles cards; offscreen shelves never mount it.
import QtQuick
import com.blitzfc.qbz

ListView {
    id: root
    property var rows: []
    property string kind: "album"
    property real artSize: 128
    property bool navFocused: false
    property int cursor: 0
    property real restoreX: 0
    signal scrollSettled(real position)
    function restorePosition() { if (!moving && restoreX > 0) contentX = restoreX }
    Component.onCompleted: Qt.callLater(restorePosition)
    onRestoreXChanged: Qt.callLater(restorePosition)
    onMovementEnded: scrollSettled(contentX)
    signal open(string id)
    orientation: ListView.Horizontal
    model: rows
    spacing: 14
    height: artSize + 58
    clip: true
    boundsBehavior: Flickable.StopAtBounds
    cacheBuffer: 0
    reuseItems: true
    onCursorChanged: if (navFocused) positionViewAtIndex(cursor, ListView.Contain)
    onNavFocusedChanged: if (navFocused) positionViewAtIndex(cursor, ListView.Contain)
    delegate: Loader {
        id: slot
        required property var modelData
        required property int index
        width: root.artSize
        height: root.height
        active: true
        sourceComponent: root.kind === "artist" ? artistCard : albumCard
        KioskCoverSource {
            id: cover
            remote: slot.modelData.artUrl || ""
            local: slot.modelData.artPath || slot.modelData.artwork || ""
            edge: root.artSize
        }
        Component {
            id: albumCard
            KioskCard {
                artSize: root.artSize
                album: ({ id: slot.modelData.id || "", title: slot.modelData.title || "",
                    artist: slot.modelData.artist || "", artwork: cover.source,
                    qualityTier: slot.modelData.qualityTier || "" })
                navFocused: root.navFocused && slot.index === root.cursor
                onClicked: function(id) { root.open(id) }
            }
        }
        Component {
            id: artistCard
            KioskArtistCard {
                artSize: root.artSize
                artist: ({ id: slot.modelData.id || "", title: slot.modelData.title || slot.modelData.name || "", artwork: cover.source })
                navFocused: root.navFocused && slot.index === root.cursor
                onClicked: function(id) { root.open(id) }
            }
        }
    }
}
