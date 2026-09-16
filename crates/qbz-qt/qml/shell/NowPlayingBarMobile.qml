// NowPlayingBarMobile — Modern Apple Music-style floating mini-player card for mobile.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root

    radius: 12
    color: theme.ambientOn ? theme.surfaceElevatedA50 : theme.surfaceElevated
    border.width: 1
    border.color: theme.borderSubtle
    clip: true

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

    // Main Row Content
    Item {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: seekbarContainer.top

        // A. Album Artwork (Left)
        Item {
            id: artBox
            anchors.left: parent.left
            anchors.leftMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            width: 40
            height: 40

            Rectangle {
                anchors.fill: parent
                radius: 6
                color: theme.surfaceCard
                border.width: 1
                border.color: theme.borderSubtle
                clip: true

                RoundedImage {
                    visible: QbzPlayer.npHasTrack && QbzPlayer.npArtworkPath !== ""
                    anchors.fill: parent
                    source: QbzPlayer.npArtworkPath
                    radius: 6
                }

                QbzIcon {
                    visible: !QbzPlayer.npHasTrack || QbzPlayer.npArtworkPath === ""
                    name: "disc"
                    width: 20
                    height: 20
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

        // B. Right Transport Controls (Play/Pause + Skip)
        Row {
            id: rightControls
            anchors.right: parent.right
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4

            // Play / Pause Button
            Rectangle {
                id: playBtn
                width: 38
                height: 38
                radius: 19
                anchors.verticalCenter: parent.verticalCenter
                color: playArea.containsMouse ? theme.surfaceHover : "transparent"
                opacity: (QbzPlayer.npHasTrack || QbzQueue.hasPlayTarget) ? 1.0 : 0.4

                QbzIcon {
                    visible: !QbzPlayer.npLoading
                    name: QbzPlayer.npPlaying ? "pause" : "play-fill"
                    width: 22
                    height: 22
                    anchors.centerIn: parent
                    tintName: "textPrimary"
                }

                // Loading Spinner
                Item {
                    anchors.fill: parent
                    visible: QbzPlayer.npLoading
                    Rectangle {
                        anchors.centerIn: parent
                        width: 22
                        height: 22
                        radius: 11
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

            // Next / Skip
            QbzIconButton {
                name: "skip-forward"
                btnSize: 36
                iconSize: 20
                btnEnabled: QbzPlayer.npHasTrack
                anchors.verticalCenter: parent.verticalCenter
                onClicked: QbzPlayer.next()
            }
        }

        // C. Middle Song Info
        Item {
            id: infoArea
            anchors.left: artBox.right
            anchors.leftMargin: 10
            anchors.right: rightControls.left
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            height: 36

            Column {
                anchors.fill: parent
                anchors.verticalCenter: parent.verticalCenter
                spacing: 1

                // Song Title
                Text {
                    width: parent.width
                    text: QbzPlayer.npHasTrack ? QbzPlayer.npTitle : QbzSession.tr("Not Playing", QbzSession.trRev)
                    font.pixelSize: 13
                    font.weight: Font.DemiBold
                    color: theme.textPrimary
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }

                // Artist
                Text {
                    width: parent.width
                    text: QbzPlayer.npHasTrack ? QbzPlayer.npArtist : ""
                    font.pixelSize: 11
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

    // Bottom 2px Progress Bar
    Item {
        id: seekbarContainer
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 2

        Rectangle {
            anchors.fill: parent
            color: Qt.rgba(theme.textMuted.r, theme.textMuted.g, theme.textMuted.b, 0.2)

            Rectangle {
                anchors.left: parent.left
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: parent.width * Math.min(Math.max(QbzPlayer.npProgress, 0), 1)
                color: theme.accent
            }
        }
    }
}