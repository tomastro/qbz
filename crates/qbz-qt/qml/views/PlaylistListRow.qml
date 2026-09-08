// PlaylistListRow — 1:1 port of primitives/PlaylistListRow.slint: the LIST
// arm of the Qobuz Playlists browse page. 60px, radius 8, odd-row zebra,
// 44px thumb, name over "owner   •   N tracks", a play affordance and the ⋯
// menu. The whole row body opens the playlist.
//
// item contract: home_qt::HomeCard as the playlist pages publish it —
// { id, title, subtitle, artPath }.
//
// Menu inventory vs the .slint's five rows: Play / Play next / Play later /
// Add to queue are wired through QbzPlayer; "Follow on Qobuz" is NOT — the
// follow invokable takes the playlist that is currently OPEN
// (`playlistToggleFollow`), so a row cannot call it, exactly as
// PlaylistCard.qml documents for "Copy to your library". The entry stays out
// of the menu rather than rendering and doing nothing.

import QtQuick
import com.blitzfc.qbz
import "../cards"
import "../controls"
import "../theme"
import "../kiosk"

Rectangle {
    id: root
    property bool kioskHost: false
    property bool pinned: item.isPinned === true
    Connections { target: QbzLibrary; function onPinChanged(key, value) { if (key === "playlist:" + (root.item.id || "")) root.pinned = value } }

    property var item: ({})
    /// Cover override for hosts whose rows carry no `artPath` of their own.
    /// The Library feed is id-keyed (`artKey` -> a decoded file:// path in
    /// LibraryView's artMap); "" falls straight back to `item.artPath`, so the
    /// browse pages are untouched.
    property string artSource: ""
    /// Up to four member-track covers — the 2x2 mosaic a USER playlist shows
    /// when it has no artwork of its own (`library_qt::FeedItem.covers`). The
    /// browse pages publish none, so they keep the designed glyph placeholder.
    property var covers: []
    readonly property string artResolved:
        root.artSource !== "" ? root.artSource : (root.item.artPath || "")
    /// Row index — drives the even/odd zebra (coherent with TrackRow).
    /// NOT named `index`: a Repeater injects one into the delegate context
    /// and a same-named property here would shadow it.
    property int rowIndex: 0

    QbzTheme { id: theme }

    readonly property bool rowHovered: rowArea.containsMouse || playArea.containsMouse
        || moreArea.containsMouse

    width: parent ? parent.width : 0
    height: root.kioskHost ? 64 : 60
    radius: theme.radiusSm
    color: rowHovered ? theme.surfaceHover
         : (rowIndex % 2 === 1 ? theme.alphaTier(4) : "transparent")

    MouseArea {
        id: rowArea
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: Qt.PointingHandCursor
        onClicked: function (mouse) {
            if (mouse.button === Qt.RightButton)
                root.openMenu(rowArea, mouse.x, mouse.y)
            else
                QbzBridge.openPlaylist(root.item.id || "")
        }
    }

    Loader {
        id: rowMenuLoader
        active: false
        sourceComponent: CardMenu {
            kioskHost: root.kioskHost
            menuWidth: root.kioskHost ? 252 : 196
            entries: [
                { "label": QbzSession.tr("Play", QbzSession.trRev), "icon": "play-fill", "action": "play" },
                { "label": QbzSession.tr("Play next", QbzSession.trRev), "icon": "list-start", "action": "next" },
                { "label": QbzSession.tr("Play later", QbzSession.trRev), "icon": "list-plus", "action": "later" },
                { "label": QbzSession.tr("Add to queue", QbzSession.trRev), "icon": "list-end", "action": "queue" },
            ].concat(root.kioskHost ? [
                {label: QbzSession.tr(root.pinned ? "Unpin" : "Pin", QbzSession.trRev), icon: "pin", action: "pin"},
                {label: QbzSession.tr("Add to mixtape", QbzSession.trRev), icon: "cassette-tape", action: "mixtape"},
                {label: QbzSession.tr("Make available offline", QbzSession.trRev), icon: "cloud-download", action: "cache"}
            ] : [])
            onPicked: function (a) {
                var id = root.item.id || ""
                if (id === "") return
                if (a === "play") QbzPlayer.playPlaylistById(id)
                else if (a === "pin") QbzLibrary.togglePin("playlist", id, root.item.title || "", root.item.subtitle || "", root.item.artUrl || "")
                else if (a === "cache") QbzOffline.cachePlaylist(String(id))
                else if (a === "mixtape") QbzMyQbzAdd.open(JSON.stringify([{itemType: "playlist", source: "qobuz", sourceItemId: String(id), title: root.item.title || "", subtitle: root.item.subtitle || "", artworkUrl: root.item.artUrl || ""}]))
                else QbzPlayer.enqueuePlaylistById(id, a)
            }
        }
    }
    function openMenu(anchor, x, y) {
        rowMenuLoader.active = true
        rowMenuLoader.item.openAtCursor(anchor, x, y)
    }
    function releaseForReuse() {
        if (rowMenuLoader.item)
            rowMenuLoader.item.close()
        rowMenuLoader.active = false
    }
    ListView.onPooled: root.releaseForReuse()

    Row {
        anchors.fill: parent
        anchors.leftMargin: 10
        anchors.rightMargin: 10
        spacing: 12

        Rectangle {
            width: 44
            height: 44
            anchors.verticalCenter: parent.verticalCenter
            radius: 6
            color: theme.surfaceElevated
            clip: true
            // These rows come from `home_qt::map_playlist` (browse_qt.rs:612),
            // whose art is the playlist's own `image.rectangle` — 800x380.
            // `auto` pads that instead of cropping to the middle 47% of it,
            // and still crops the square `image.covers[0]` fallback.
            KioskArtwork {
                anchors.fill: parent
                visible: root.kioskHost
                source: root.kioskHost ? root.artResolved : ""
            }
            RoundedImage {
                anchors.fill: parent
                visible: !root.kioskHost && !rowCollage.visible
                source: root.kioskHost ? "" : root.artResolved
                radius: 6
                fit: "auto"
            }
            PlaylistCollage {
                id: rowCollage
                anchors.fill: parent
                visible: root.artResolved === "" && (root.covers || []).length > 0
                urls: root.covers || []
                radius: 6
            }
            QbzIcon {
                visible: root.artResolved === "" && !rowCollage.visible
                name: "list-music"
                width: 20
                height: 20
                anchors.centerIn: parent
                tintName: "muted"
            }
        }

        Column {
            width: parent.width - 44 - 2 * (root.kioskHost ? 44 : 30) - 4 * 12
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            Text {
                width: parent.width
                text: root.item.title || ""
                color: theme.textPrimary
                font.pixelSize: theme.fontBody
                font.weight: theme.weightMedium
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: root.item.subtitle || ""
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
                elide: Text.ElideRight
            }
        }

        Item {
            width: root.kioskHost ? 44 : 30
            height: parent.height
            QbzIcon {
                name: "play-fill"
                width: 16
                height: 16
                anchors.centerIn: parent
                tintName: root.rowHovered ? "textPrimary" : "muted"
            }
            MouseArea {
                id: playArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: QbzPlayer.playPlaylistById(root.item.id || "")
            }
        }
        Item {
            width: root.kioskHost ? 44 : 30
            height: parent.height
            QbzIcon {
                name: "ellipsis"
                width: 16
                height: 16
                anchors.centerIn: parent
                tintName: moreArea.containsMouse ? "textPrimary" : "secondary"
            }
            MouseArea {
                id: moreArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: function (mouse) { rowMenu.openAtCursor(moreArea, mouse.x, mouse.y) }
            }
        }
    }
}
