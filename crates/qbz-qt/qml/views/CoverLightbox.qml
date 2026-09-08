// Artwork lightbox — NEW in the Qt port (no Slint counterpart): the artwork
// at its best available quality, fitted to the window. Opened by left-click on
// the AlbumView cover, the PlaylistView cover or the ArtistView portrait, and
// by those menus' "View cover" / "View image" entries.
//
// Shape follows TrackInfoModal: a full-window Popup parented to
// Overlay.overlay, a dim scrim whose click closes, Escape closes. The image
// asks for a decode capped at the window size (sourceSize), so a mega
// variant does not cost a full-res decode to display at ~90% of the window.
//
// ASPECT-AWARE, and it has to be. This used to force a SQUARE box of
// `min(w,h) * 0.9` on the grounds that "covers are square". Album covers are;
// artist portraits frequently are NOT (1080x720 among the portraits measured
// in this machine's cache), and a landscape image squeezed into the min-axis
// square gave up roughly a third of the height available on a wide window.
// The box now takes the image's own ratio inside a 90% viewport, which lands
// on exactly the old square for a square source — nothing regresses.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../theme"

Popup {
    id: root

    parent: Overlay.overlay
    x: 0
    y: 0
    width: parent ? parent.width : 0
    height: parent ? parent.height : 0
    padding: 0
    closePolicy: Popup.CloseOnEscape

    // file:// (custom cover) or the remote best() URL. Set through openWith.
    property string artSource: ""

    function openWith(source) {
        // Builders hand over remote URLs, file:// urls, and (for custom
        // covers) bare absolute paths — normalize here, once.
        if (source.charAt(0) === "/") source = "file://" + source
        root.artSource = source
        root.open()
    }

    QbzTheme { id: theme }

    background: Rectangle {
        color: "#bf000000"
    }

    contentItem: Item {
        id: stage
        // Scrim click closes; clicks on the image itself are swallowed so
        // the lightbox stays up while the user inspects it.
        MouseArea {
            anchors.fill: parent
            onClicked: root.close()
            // Wheel-lock (the DiscoverConfigModal rule): the Popup is not
            // `modal`, so nothing grabs the wheel for it — without this the
            // page kept scrolling behind the lightbox.
            onWheel: function (wheel) { wheel.accepted = true }
        }

        // The 90% viewport the artwork is fitted into, so it never touches
        // the rim on either axis.
        readonly property real maxW: root.width * 0.9
        readonly property real maxH: root.height * 0.9
        // The SOURCE ratio. `implicitWidth/Height` report the decoded size,
        // which `sourceSize` has already scaled — but scaled preserving the
        // aspect, so the ratio is the source's. 1.0 until the image reports,
        // i.e. the square box this file used to force unconditionally.
        readonly property real ratio: (art.implicitWidth > 0 && art.implicitHeight > 0)
            ? (art.implicitWidth / art.implicitHeight) : 1.0
        readonly property real boxW: Math.min(stage.maxW, stage.maxH * stage.ratio)
        readonly property real boxH: Math.min(stage.maxH, stage.maxW / stage.ratio)

        Image {
            id: art
            anchors.centerIn: parent
            width: stage.boxW
            height: stage.boxH
            source: root.artSource
            asynchronous: true
            fillMode: Image.PreserveAspectFit
            // Capped off the VIEWPORT, not off boxW/boxH: the box is derived
            // from implicitWidth/Height, which sourceSize itself determines —
            // binding the cap to the box would be a loop. The viewport is the
            // real display bound anyway, so nothing is under-decoded.
            sourceSize: Qt.size(Math.round(stage.maxW), Math.round(stage.maxH))
            smooth: true
            visible: status === Image.Ready

            MouseArea {
                anchors.fill: parent
                onClicked: {} // swallow — the scrim's close must not fire here
            }
        }

        QbzSpinner {
            anchors.centerIn: parent
            visible: root.artSource !== "" && art.status === Image.Loading
        }

        Text {
            anchors.centerIn: parent
            visible: root.artSource !== "" && art.status === Image.Error
            text: QbzSession.tr("Could not load the image", QbzSession.trRev)
            color: theme.textSecondary
            font.pixelSize: theme.fontBody
        }
    }
}
