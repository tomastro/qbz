// KioskTrackRow — lightweight track row for the kiosk Search/Library track
// tabs (shell/KioskTrackRow.slint). Art + title/artist + duration, tap to
// play. No number cell, no favourite, no quality badge, no menu, no
// checkbox — the desktop rows/TrackRow.qml column model is exactly the weight
// the kiosk shell drops.
//
// `track` is a plain JS object carrying { id, title, artist, duration,
// artwork }; `artwork` is the RESOLVED local cover path the hosting view
// supplies (the Qt documents carry only the remote `artUrl`).
//
// `playing` marks the row whose track is the one on the deck. The Slint
// KioskTrackRow has no such state, so its treatment is taken from the kiosk's
// own current-track idiom on the album page (KioskAlbum.slint:171-175, :195):
// an accent@0.10 fill that sits BELOW focus and hover in the precedence, and
// the title in the accent colour.
//
// ── TOUCH HEIGHT ──────────────────────────────────────────────────────────
// 64 px, not the 62 this file shipped. Contract §2.6 puts the PRIMARY touch
// target at "al menos 64 px lógicos cuando hay espacio", and a track row is
// the primary target of every list surface in the kiosk — Library/Search
// Tracks, the album track list, the queue. Two pixels is not a visual change;
// it is the difference between meeting the floor and missing it by a hair on
// every list in the shell.
//
// The whole row is ONE hit area, so the target IS the height: there is no
// number cell, favourite, badge, menu or checkbox to subdivide it (that
// absence is the point of this component). `rowHeight` stays a property so a
// denser surface can opt out with evidence rather than by editing this file.
//
// The art tile stays 46 and centres in the taller row. It is a thumbnail, not
// a target, and growing it would enlarge every artwork request in the shell
// for no touch benefit.
//
// ARTWORK: KioskArtwork, not theme/RoundedImage — see KioskCard's header. In
// a 46 px tile the difference is the largest in the shell: the desktop path
// would decode a 600 px original to discover it should have asked for ~50.

import QtQuick
import "../theme"

Rectangle {
    id: root

    property var track: ({})
    /// Keyboard/gamepad nav — same ring idiom as KioskCard, 2px here.
    property bool navFocused: false
    property bool playing: false
    /// The primary touch target of every kiosk list (contract §2.6: ≥64 px).
    property real rowHeight: 64
    /// Thumbnail edge. Not a touch target; it drives the artwork request.
    property real artSize: 46
    signal clicked()

    /// The thumbnail is DECODED and on screen — the KioskCard.artReady
    /// contract. A row with no artwork slot never becomes ready, which is
    /// correct: `_artwork` is "" and there is nothing to wait for.
    readonly property bool artReady: art.ready

    QbzTheme { id: theme }

    readonly property string _title: root.track && root.track.title ? root.track.title : ""
    readonly property string _artist: root.track && root.track.artist ? root.track.artist : ""
    readonly property string _duration: root.track && root.track.duration ? root.track.duration : ""
    readonly property string _artwork: root.track && root.track.artwork ? root.track.artwork : ""

    height: root.rowHeight
    radius: theme.radiusSm
    color: root.navFocused
        ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.18)
        : rowArea.containsMouse
            ? theme.alphaTier(6)
            : root.playing
                ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.10)
                : "transparent"
    border.width: root.navFocused ? 2 : 0
    border.color: theme.accent

    Row {
        id: cells
        anchors.left: root.left
        anchors.leftMargin: 8
        anchors.right: root.right
        anchors.rightMargin: 14
        anchors.top: root.top
        anchors.bottom: root.bottom
        spacing: 12

        // Art slot — a fixed square thumbnail centred in the row height.
        Item {
            id: artCell
            width: root.artSize
            height: cells.height

            Rectangle {
                id: artTile
                width: root.artSize
                height: root.artSize
                y: (artCell.height - artTile.height) / 2
                radius: 5
                color: theme.surfaceElevated
                clip: true

                KioskArtwork {
                    id: art
                    anchors.fill: parent
                    source: root._artwork
                    radius: 5
                    fit: "crop"
                }
            }
        }

        Item {
            id: textCol
            width: Math.max(0, cells.width - artCell.width - durationLabel.width - 2 * cells.spacing)
            height: cells.height

            Column {
                anchors.verticalCenter: textCol.verticalCenter
                width: textCol.width
                spacing: 2

                Text {
                    width: textCol.width
                    text: root._title
                    color: root.playing ? theme.accent : theme.textPrimary
                    font.pixelSize: 15
                    font.weight: theme.weightMedium
                    elide: Text.ElideRight
                    wrapMode: Text.NoWrap
                    maximumLineCount: 1
                }

                Text {
                    width: textCol.width
                    text: root._artist
                    color: theme.textMuted
                    font.pixelSize: 13
                    elide: Text.ElideRight
                    wrapMode: Text.NoWrap
                    maximumLineCount: 1
                }
            }
        }

        Text {
            id: durationLabel
            height: cells.height
            text: root._duration
            color: theme.textMuted
            font.pixelSize: 13
            verticalAlignment: Text.AlignVCenter
        }
    }

    MouseArea {
        id: rowArea
        anchors.fill: parent
        hoverEnabled: true
        onClicked: root.clicked()
    }
}
