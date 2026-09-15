// NowPlayingBarMobile — Streamlined Now Playing Bar for Smartphone / Portrait view.
// Replaces the 3-column desktop PlayerBar with a clean, touch-friendly mobile miniplayer.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root

    color: ambientOn ? theme.surfaceCardA50 : theme.surfaceCard
    readonly property bool ambientOn: theme.ambientOn
    property Item tooltip: null

    QbzTheme { id: theme }

    // Helpers
    function fmt(seconds) {
        if (!seconds || isNaN(seconds) || seconds < 0) return "0:00"
        var s = Math.floor(seconds)
        var m = Math.floor(s / 60)
        var sec = s % 60
        return m + ":" + (sec < 10 ? "0" : "") + sec
    }

    // 1. TOP 3px Seekbar / Progress
    Item {
        id: seekbarContainer
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: 3
        z: 10

        Rectangle {
            id: seekBg
            anchors.fill: parent
            color: theme.surfaceElevated

            // Buffered progress
            Rectangle {
                anchors.left: parent.left
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: parent.width * Math.min(Math.max(QbzPlayer.npCacheProgress, 0), 1)
                color: Qt.rgba(theme.textMuted.r, theme.textMuted.g, theme.textMuted.b, 0.35)
            }

            // Played progress
            Rectangle {
                anchors.left: parent.left
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: parent.width * Math.min(Math.max(QbzPlayer.npProgress, 0), 1)
                color: theme.accent
            }
        }

        // Expanded touch target for scrubbing
        MouseArea {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            anchors.topMargin: -8
            anchors.bottomMargin: -8
            enabled: QbzPlayer.npHasTrack
            onPressed: function(mouse) {
                if (QbzPlayer.npHasTrack && width > 0) {
                    var frac = Math.min(Math.max(mouse.x / width, 0), 1)
                    QbzPlayer.seek(frac)
                }
            }
            onPositionChanged: function(mouse) {
                if (pressed && QbzPlayer.npHasTrack && width > 0) {
                    var frac = Math.min(Math.max(mouse.x / width, 0), 1)
                    QbzPlayer.seek(frac)
                }
            }
        }
    }

    // 2. Main Content Area
    Item {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: seekbarContainer.bottom
        anchors.bottom: parent.bottom

        // A. Album Artwork (Left)
        Item {
            id: artBox
            anchors.left: parent.left
            anchors.leftMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            width: 48
            height: 48

            Rectangle {
                anchors.fill: parent
                radius: 6
                color: theme.surfaceElevated
                border.width: 1
                border.color: theme.borderSubtle

                RoundedImage {
                    visible: QbzPlayer.npHasTrack && QbzPlayer.npArtworkPath !== ""
                    anchors.fill: parent
                    source: QbzPlayer.npArtworkPath
                    radius: 6
                }

                QbzIcon {
                    visible: !QbzPlayer.npHasTrack || QbzPlayer.npArtworkPath === ""
                    name: "disc"
                    width: 24
                    height: 24
                    anchors.centerIn: parent
                    tintName: "muted"
                }
            }

            MouseArea {
                anchors.fill: parent
                onClicked: {
                    if (QbzPlayer.npHasTrack) {
                        QbzShell.toggleQueue()
                    }
                }
            }
        }

        // B. Right Transport Controls
        Row {
            id: rightControls
            anchors.right: parent.right
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2

            // Previous
            QbzIconButton {
                name: "skip-back"
                btnSize: 36
                iconSize: 18
                btnEnabled: QbzPlayer.npHasTrack
                anchors.verticalCenter: parent.verticalCenter
                onClicked: QbzPlayer.previous()
            }

            // Big Play / Pause Circle
            Rectangle {
                id: playBtn
                width: 42
                height: 42
                radius: 21
                anchors.verticalCenter: parent.verticalCenter
                color: playArea.containsMouse ? theme.accentHover : theme.accent
                opacity: (QbzPlayer.npHasTrack || QbzQueue.hasPlayTarget) ? 1.0 : 0.4

                QbzIcon {
                    visible: !QbzPlayer.npLoading
                    name: QbzPlayer.npPlaying ? "pause" : "play-fill"
                    width: 20
                    height: 20
                    anchors.centerIn: parent
                    tintName: theme.accentGlyphTint
                }

                // Loading Spinner
                Item {
                    anchors.fill: parent
                    visible: QbzPlayer.npLoading
                    Rectangle {
                        anchors.centerIn: parent
                        width: 26
                        height: 26
                        radius: 13
                        color: "transparent"
                        border.width: 2
                        border.color: theme.accentText
                        opacity: 0.35 + 0.45 * (1 + Math.sin(QbzShell.pulseMs * 0.005)) / 2
                    }
                }

                MouseArea {
                    id: playArea
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        if (QbzPlayer.npHasTrack || QbzQueue.hasPlayTarget) {
                            QbzPlayer.togglePlay()
                        }
                    }
                }
            }

            // Next
            QbzIconButton {
                name: "skip-forward"
                btnSize: 36
                iconSize: 18
                btnEnabled: QbzPlayer.npHasTrack
                anchors.verticalCenter: parent.verticalCenter
                onClicked: QbzPlayer.next()
            }

            // Queue toggle
            QbzIconButton {
                name: "list-ordered"
                btnSize: 36
                iconSize: 18
                active: QbzShell.queueOpen
                anchors.verticalCenter: parent.verticalCenter
                onClicked: QbzShell.toggleQueue()
            }
        }

        // C. Middle Song Info (Expands to fill all space between Art and Controls)
        Item {
            id: infoArea
            anchors.left: artBox.right
            anchors.leftMargin: 10
            anchors.right: rightControls.left
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            height: 40

            Column {
                anchors.fill: parent
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2

                // Song Title
                Text {
                    width: parent.width
                    text: QbzPlayer.npHasTrack ? QbzPlayer.npTitle : QbzSession.tr("Not Playing", QbzSession.trRev)
                    font.pixelSize: 14
                    font.weight: Font.DemiBold
                    color: theme.textPrimary
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }

                // Artist & Quality / Duration
                Text {
                    width: parent.width
                    readonly property string artistText: QbzPlayer.npHasTrack ? QbzPlayer.npArtist : ""
                    readonly property string qualityText: QbzPlayer.npQualityTier ? (" • " + QbzPlayer.npQualityTier) : ""
                    text: (artistText !== "" ? (artistText + qualityText) : QbzSession.tr("Select a track", QbzSession.trRev))
                    font.pixelSize: 12
                    color: theme.textMuted
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }
            }

            MouseArea {
                anchors.fill: parent
                onClicked: {
                    if (QbzPlayer.npHasTrack) {
                        QbzShell.toggleQueue()
                    }
                }
            }
        }
    }
}