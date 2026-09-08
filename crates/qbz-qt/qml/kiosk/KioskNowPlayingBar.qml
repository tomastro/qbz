// Touch transport: shared bridge state/actions, a single 64px footprint.
// Detailed queue, seek and secondary actions live in the Now Playing route;
// Qobuz Connect and Cast live in the back bar (KioskShell.qml).
import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root
    readonly property int barHeight: 64
    property Item tooltip: null
    implicitHeight: barHeight
    color: theme.surfaceCard
    QbzTheme { id: theme }

    component TouchButton: Rectangle {
        id: button
        property string name: ""
        property string label: ""
        property bool available: true
        property bool active: false
        signal clicked()
        width: root.barHeight
        height: root.barHeight
        color: active ? theme.surfaceElevated : "transparent"
        border.width: activeFocus ? 2 : 0
        border.color: theme.accent
        opacity: available ? 1 : 0.35
        activeFocusOnTab: available && visible
        Accessible.role: Accessible.Button
        Accessible.name: label
        Accessible.onPressAction: if (available) clicked()
        QbzIcon {
            anchors.centerIn: parent
            width: 28; height: 28
            name: button.name
            tintName: button.active ? "accent" : "textPrimary"
        }
        Keys.onPressed: function (event) {
            if (available && !event.isAutoRepeat && (event.key === Qt.Key_Space
                    || event.key === Qt.Key_Return || event.key === Qt.Key_Enter)) {
                clicked()
                event.accepted = true
            }
        }
        MouseArea {
            anchors.fill: parent
            enabled: button.available
            onPressed: button.forceActiveFocus()
            onClicked: button.clicked()
        }
    }

    Item {
        id: track
        anchors.left: parent.left
        anchors.right: transport.left
        height: parent.height
        activeFocusOnTab: true
        Accessible.role: Accessible.Button
        Accessible.name: QbzSession.tr("Now Playing", QbzSession.trRev)
        Accessible.onPressAction: QbzShell.navigateTo("nowplaying")
        KioskArtwork {
            id: artwork
            x: 8; anchors.verticalCenter: parent.verticalCenter
            width: 48; height: 48
            source: QbzPlayer.npArtworkPath
        }
        Column {
            anchors.left: artwork.right
            anchors.leftMargin: 12
            anchors.right: parent.right
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4
            Text {
                width: parent.width
                text: QbzPlayer.npHasTrack ? QbzPlayer.npTitle : QbzSession.tr("Now Playing", QbzSession.trRev)
                color: theme.textPrimary; font.pixelSize: 16
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: QbzPlayer.npLoading ? QbzSession.tr("Loading…", QbzSession.trRev) : QbzPlayer.npArtist
                color: theme.textSecondary; font.pixelSize: 14
                elide: Text.ElideRight
            }
        }
        Rectangle { anchors.fill: parent; color: "transparent"; border.width: track.activeFocus ? 2 : 0; border.color: theme.accent }
        MouseArea { anchors.fill: parent; onClicked: QbzShell.navigateTo("nowplaying") }
        Keys.onPressed: function (event) {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                QbzShell.navigateTo("nowplaying")
                event.accepted = true
            }
        }
    }
    // The transport cluster. Owner feedback 2026-09-07: the hidden "more"
    // menu is gone — Connect and Cast moved to the back bar (KioskShell),
    // Settings has its NavRail tile, Immersive has the Visualizer tile — so
    // every remaining control is a permanent 64px button: shuffle/repeat
    // (wide panels only; Now Playing carries them everywhere), the three
    // transport buttons, an always-visible Volume button whose slider opens
    // VERTICALLY above the bar, the Now Playing shortcut and the way back
    // to Desktop mode.
    Row {
        id: transport
        anchors.right: parent.right
        height: parent.height
        TouchButton {
            visible: root.width >= 1024
            name: "shuffle"; label: QbzSession.tr("Shuffle", QbzSession.trRev)
            available: QbzPlayer.npHasTrack; active: QbzPlayer.npShuffle
            onClicked: QbzPlayer.toggleShuffle()
        }
        TouchButton {
            name: "skip-back"; label: QbzSession.tr("Previous", QbzSession.trRev)
            available: QbzPlayer.npHasTrack
            onClicked: QbzPlayer.previous()
        }
        TouchButton {
            name: QbzPlayer.npPlaying ? "pause" : "play-fill"
            label: QbzPlayer.npPlaying ? QbzSession.tr("Pause", QbzSession.trRev) : QbzSession.tr("Play", QbzSession.trRev)
            active: true
            available: QbzPlayer.npHasTrack || QbzQueue.hasPlayTarget
            onClicked: QbzPlayer.togglePlay()
        }
        TouchButton {
            name: "skip-forward"; label: QbzSession.tr("Next", QbzSession.trRev)
            available: QbzPlayer.npHasTrack
            onClicked: QbzPlayer.next()
        }
        TouchButton {
            visible: root.width >= 1024
            name: QbzPlayer.npRepeatMode === 2 ? "repeat-1" : "repeat"
            label: QbzSession.tr("Repeat", QbzSession.trRev)
            available: QbzPlayer.npHasTrack; active: QbzPlayer.npRepeatMode > 0
            onClicked: QbzPlayer.cycleRepeat()
        }
        TouchButton {
            id: volumeButton
            name: QbzPlayer.npMuted ? "volume-x" : "volume-2"
            label: QbzSession.tr("Volume", QbzSession.trRev)
            active: volumePopup.opened
            onClicked: root.openVolume()
        }
        TouchButton {
            name: "list-music"; label: QbzSession.tr("Now Playing", QbzSession.trRev)
            onClicked: QbzShell.navigateTo("nowplaying")
        }
        TouchButton {
            name: "monitor"; label: QbzSession.tr("Desktop mode", QbzSession.trRev)
            onClicked: QbzSession.toggleProfile()
        }
    }

    // Volume lock, both halves (PlayerBar.qml:86-96): the ALSA-Direct
    // hardware derivation while local, and the peer's "no remote volume".
    readonly property bool volumeLocked:
        (QbzPlayer.npVolumeLocked && !QbzPlayer.npIsRemote) || QbzPlayer.npRemoteVolumeLocked

    /// Opens the vertical volume sheet ABOVE its button, horizontally centred
    /// on it and clamped 8px inside the window on every side. The sheet's
    /// length is derived from the window height, so it can never be taller
    /// than the space above the bar — on a 480px panel the slider shortens
    /// rather than the window eating it.
    function openVolume() {
        if (volumePopup.opened) {
            volumePopup.close()
            return
        }
        var win = volumePopup.parent
        var g = volumeButton.mapToItem(null, volumeButton.width / 2, 0)
        volumePopup.x = Math.max(8, Math.min(g.x - volumePopup.width / 2,
                                             win.width - volumePopup.width - 8))
        volumePopup.y = Math.max(8, g.y - volumePopup.height - 8)
        volumePopup.open()
    }

    Popup {
        id: volumePopup
        parent: Overlay.overlay
        // Slider travel: whatever fits between the bar and the top of the
        // window, between 120 and 260px.
        readonly property real sliderLength: Math.max(120, Math.min(260,
            (parent ? parent.height : 480) - root.barHeight - 64 - 24 - 32))
        width: 64 + 24
        height: sliderLength + 8 + 64 + 24
        padding: 12
        closePolicy: Popup.CloseOnPressOutside | Popup.CloseOnEscape
        background: Rectangle { color: theme.surfaceCard; radius: 8; border.color: theme.borderSubtle; border.width: 1 }
        onOpened: volumeControl.forceActiveFocus()
        Column {
            width: 64
            spacing: 8
            // The shared horizontal slider, turned on its side: a 64px-tall
            // slider of `sliderLength` width rotated a quarter turn
            // counter-clockwise occupies a 64 x sliderLength box centred on
            // the same point, and its +x (louder) axis now points UP. Qt
            // maps pointer events through the transform, so the drag math
            // in QbzSlider is untouched.
            Item {
                width: 64
                height: volumePopup.sliderLength
                QbzSlider {
                    id: volumeControl
                    kioskHost: true
                    width: parent.height
                    height: 44
                    anchors.centerIn: parent
                    rotation: -90
                    minimum: 0; maximum: 1000
                    enabled: !root.volumeLocked
                    value: Math.round(QbzPlayer.npVolume * 1000)
                    onChanged: function (v) { QbzPlayer.setVolume(v / 1000.0) }
                    onReleased: function (v) { QbzPlayer.persistVolume(v / 1000.0) }
                }
            }
            TouchButton {
                name: QbzPlayer.npMuted ? "volume-x" : "volume-2"
                label: QbzSession.tr("Mute", QbzSession.trRev)
                available: !root.volumeLocked
                active: QbzPlayer.npMuted
                onClicked: QbzPlayer.toggleMute()
            }
        }
    }
    Rectangle {
        width: parent.width; height: 2
        color: theme.surfaceElevated
        Rectangle { width: parent.width * Math.max(0, Math.min(1, QbzPlayer.npProgress)); height: 2; color: theme.accent }
    }
}
