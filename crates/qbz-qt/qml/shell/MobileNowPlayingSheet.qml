// MobileNowPlayingSheet — Apple Music style full-screen Now Playing modal sheet.
// Slides up from bottom when tapping the floating mini-player.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root

    property bool open: false
    property real topSafeInset: 0
    property real bottomSafeInset: 0
    signal queueRequested()
    signal lyricsRequested()

    readonly property bool isFavorite: {
        try {
            var d = JSON.parse(QbzQueue.queueJson)
            return !!(d && d.current && d.current.isFavorite)
        } catch (e) {
            return false
        }
    }

    function fmt(seconds) {
        if (!seconds || isNaN(seconds) || seconds < 0) return "0:00"
        var s = Math.floor(seconds)
        var m = Math.floor(s / 60)
        var sec = s % 60
        return m + ":" + (sec < 10 ? "0" : "") + sec
    }

    width: parent ? parent.width : 0
    height: parent ? parent.height : 0
    y: root.open ? 0 : (parent ? parent.height : 0)
    visible: root.open || (parent && y < parent.height)
    color: "#0d0d10"
    clip: true

    Behavior on y {
        NumberAnimation {
            duration: 300
            easing.type: Easing.OutCubic
        }
    }

    // Capture Android back key to close sheet
    focus: root.open
    Keys.onReleased: function(event) {
        if (event.key === Qt.Key_Back && root.open) {
            root.open = false
            event.accepted = true
        }
    }

    QbzTheme { id: theme }

    // Ambient background art
    Item {
        anchors.fill: parent
        opacity: 0.35
        clip: true

        Image {
            id: bgArt
            anchors.fill: parent
            source: QbzPlayer.npArtworkPath
            fillMode: Image.PreserveAspectCrop
            visible: QbzPlayer.npArtworkPath !== ""
        }
    }

    // Dark gradient overlay
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: Qt.rgba(0, 0, 0, 0.45) }
            GradientStop { position: 0.5; color: Qt.rgba(0, 0, 0, 0.70) }
            GradientStop { position: 1.0; color: Qt.rgba(0, 0, 0, 0.92) }
        }
    }

    // Top Pull-Down Pill Bar
    Item {
        id: topHandle
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.topMargin: root.topSafeInset
        height: 48
        z: 10

        Rectangle {
            anchors.centerIn: parent
            width: 38
            height: 5
            radius: 2.5
            color: Qt.rgba(1, 1, 1, 0.35)
        }

        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            property real startY: 0
            onPressed: function(mouse) { startY = mouse.y }
            onPositionChanged: function(mouse) {
                if (mouse.y - startY > 20) {
                    root.open = false
                }
            }
            onClicked: root.open = false
        }
    }

    // Main Column Layout
    Item {
        anchors.top: topHandle.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.leftMargin: 24
        anchors.rightMargin: 24
        anchors.bottomMargin: 20 + root.bottomSafeInset

        // 1. Center Artwork Card
        Item {
            id: artworkContainer
            anchors.top: parent.top
            anchors.horizontalCenter: parent.horizontalCenter
            width: Math.min(parent.width - 16, parent.height * 0.40)
            height: width

            Rectangle {
                anchors.fill: parent
                radius: 14
                color: "#1c1c20"
                border.width: 1
                border.color: Qt.rgba(1, 1, 1, 0.12)
                clip: true

                RoundedImage {
                    visible: QbzPlayer.npHasTrack && QbzPlayer.npArtworkPath !== ""
                    anchors.fill: parent
                    source: QbzPlayer.npArtworkPath
                    radius: 14
                }

                QbzIcon {
                    visible: !QbzPlayer.npHasTrack || QbzPlayer.npArtworkPath === ""
                    name: "disc"
                    width: 72
                    height: 72
                    anchors.centerIn: parent
                    tintName: "muted"
                }
            }
        }

        // 2. Track Title, Artist, and Action Buttons (Star, More)
        Item {
            id: metaRow
            anchors.top: artworkContainer.bottom
            anchors.topMargin: 24
            anchors.left: parent.left
            anchors.right: parent.right
            height: 54

            Column {
                anchors.left: parent.left
                anchors.right: actionButtons.left
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                spacing: 4

                Text {
                    width: parent.width
                    text: QbzPlayer.npHasTrack ? QbzPlayer.npTitle : QbzSession.tr("Not Playing", QbzSession.trRev)
                    font.pixelSize: 21
                    font.weight: Font.Bold
                    color: "#ffffff"
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }

                Text {
                    width: parent.width
                    text: QbzPlayer.npHasTrack ? QbzPlayer.npArtist : ""
                    font.pixelSize: 16
                    font.weight: Font.Normal
                    color: Qt.rgba(1, 1, 1, 0.65)
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }
            }

            Row {
                id: actionButtons
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8

                // Star (Favorite) Button
                Rectangle {
                    width: 40
                    height: 40
                    radius: 20
                    color: starArea.containsMouse ? Qt.rgba(1, 1, 1, 0.15) : Qt.rgba(1, 1, 1, 0.08)

                    QbzIcon {
                        anchors.centerIn: parent
                        name: root.isFavorite ? "star" : "star"
                        width: 22
                        height: 22
                        tintName: root.isFavorite ? "accent" : "textPrimary"
                    }

                    MouseArea {
                        id: starArea
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            if (QbzPlayer.npHasTrack && QbzPlayer.npTrackId !== "") {
                                QbzQueue.queueToggleFavorite("track", QbzPlayer.npTrackId)
                            }
                        }
                    }
                }

                // More (...) Button
                Rectangle {
                    width: 40
                    height: 40
                    radius: 20
                    color: moreArea.containsMouse ? Qt.rgba(1, 1, 1, 0.15) : Qt.rgba(1, 1, 1, 0.08)

                    QbzIcon {
                        anchors.centerIn: parent
                        name: "ellipsis"
                        width: 22
                        height: 22
                        tintName: "textPrimary"
                    }

                    MouseArea {
                        id: moreArea
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            if (QbzPlayer.npAlbumId !== "") {
                                root.open = false
                                QbzAlbum.openAlbum(QbzPlayer.npAlbumId)
                            }
                        }
                    }
                }
            }
        }

        // 3. Scrubber Bar & Times
        Item {
            id: scrubberArea
            anchors.top: metaRow.bottom
            anchors.topMargin: 20
            anchors.left: parent.left
            anchors.right: parent.right
            height: 38

            // Progress Track
            Rectangle {
                id: seekTrack
                anchors.top: parent.top
                anchors.left: parent.left
                anchors.right: parent.right
                height: 5
                radius: 2.5
                color: Qt.rgba(1, 1, 1, 0.22)

                Rectangle {
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    width: parent.width * Math.max(0, Math.min(1, QbzPlayer.npProgress))
                    radius: 2.5
                    color: Qt.rgba(1, 1, 1, 0.85)
                }

                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -10
                    onPressed: function(mouse) {
                        if (QbzPlayer.npHasTrack) {
                            var frac = Math.max(0, Math.min(1, mouse.x / seekTrack.width))
                            QbzPlayer.seek(frac)
                        }
                    }
                    onPositionChanged: function(mouse) {
                        if (pressed && QbzPlayer.npHasTrack) {
                            var frac = Math.max(0, Math.min(1, mouse.x / seekTrack.width))
                            QbzPlayer.seek(frac)
                        }
                    }
                }
            }

            // Labels under seek bar
            Item {
                anchors.top: seekTrack.bottom
                anchors.topMargin: 8
                anchors.left: parent.left
                anchors.right: parent.right
                height: 16

                Text {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.fmt(QbzPlayer.npElapsedSecs)
                    font.pixelSize: 12
                    color: Qt.rgba(1, 1, 1, 0.55)
                }

                // Format / Quality Badge
                Rectangle {
                    anchors.centerIn: parent
                    height: 18
                    radius: 4
                    color: Qt.rgba(1, 1, 1, 0.12)
                    width: qualityText.implicitWidth + 12
                    visible: QbzPlayer.npHasTrack

                    Text {
                        id: qualityText
                        anchors.centerIn: parent
                        text: (QbzPlayer.npQualityTier === "hires" || QbzPlayer.npQualityEffectiveTier === "hires")
                            ? "Hi-Res"
                            : (QbzPlayer.npQualityTier === "lossless" ? "Lossless" : "CD")
                        font.pixelSize: 10
                        font.weight: Font.DemiBold
                        color: Qt.rgba(1, 1, 1, 0.80)
                    }
                }

                Text {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    text: "-" + root.fmt(Math.max(0, QbzPlayer.npDurationSecs - QbzPlayer.npElapsedSecs))
                    font.pixelSize: 12
                    color: Qt.rgba(1, 1, 1, 0.55)
                }
            }
        }

        // 4. Playback Controls (Prev, Play/Pause, Next)
        Item {
            id: transportControls
            anchors.top: scrubberArea.bottom
            anchors.topMargin: 20
            anchors.left: parent.left
            anchors.right: parent.right
            height: 72

            Row {
                anchors.centerIn: parent
                spacing: 36

                // Previous
                Rectangle {
                    width: 48
                    height: 48
                    radius: 24
                    color: prevArea.containsMouse ? Qt.rgba(1, 1, 1, 0.12) : "transparent"
                    anchors.verticalCenter: parent.verticalCenter

                    QbzIcon {
                        anchors.centerIn: parent
                        name: "skip-back"
                        width: 32
                        height: 32
                        tintName: "textPrimary"
                    }

                    MouseArea {
                        id: prevArea
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: QbzPlayer.previous()
                    }
                }

                // Play / Pause
                Rectangle {
                    width: 68
                    height: 68
                    radius: 34
                    color: Qt.rgba(1, 1, 1, 0.15)
                    anchors.verticalCenter: parent.verticalCenter

                    QbzIcon {
                        anchors.centerIn: parent
                        anchors.horizontalCenterOffset: QbzPlayer.npPlaying ? 0 : 2
                        name: QbzPlayer.npPlaying ? "pause" : "play-fill"
                        width: 36
                        height: 36
                        tintName: "textPrimary"
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: QbzPlayer.togglePlay()
                    }
                }

                // Next
                Rectangle {
                    width: 48
                    height: 48
                    radius: 24
                    color: nextArea.containsMouse ? Qt.rgba(1, 1, 1, 0.12) : "transparent"
                    anchors.verticalCenter: parent.verticalCenter

                    QbzIcon {
                        anchors.centerIn: parent
                        name: "skip-forward"
                        width: 32
                        height: 32
                        tintName: "textPrimary"
                    }

                    MouseArea {
                        id: nextArea
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: QbzPlayer.next()
                    }
                }
            }
        }

        // 5. Bottom Toolbar (Lyrics, Device/Cast, Queue)
        Item {
            id: bottomBar
            anchors.bottom: parent.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            height: 50

            Row {
                anchors.centerIn: parent
                spacing: 64

                // Lyrics
                Rectangle {
                    width: 44
                    height: 44
                    radius: 22
                    color: lyricsArea.containsMouse ? Qt.rgba(1, 1, 1, 0.12) : "transparent"

                    QbzIcon {
                        anchors.centerIn: parent
                        name: "music-note-slider"
                        width: 24
                        height: 24
                        tintName: QbzShell.lyricsOpen ? "accent" : "secondary"
                    }

                    MouseArea {
                        id: lyricsArea
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.lyricsRequested()
                    }
                }

                // Cast / Speaker Output
                Rectangle {
                    width: 44
                    height: 44
                    radius: 22
                    color: castArea.containsMouse ? Qt.rgba(1, 1, 1, 0.12) : "transparent"

                    QbzIcon {
                        anchors.centerIn: parent
                        name: "speaker"
                        width: 24
                        height: 24
                        tintName: "secondary"
                    }

                    MouseArea {
                        id: castArea
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            QbzShell.navigateTo("settings")
                            root.open = false
                        }
                    }
                }

                // Queue
                Rectangle {
                    width: 44
                    height: 44
                    radius: 22
                    color: queueArea.containsMouse ? Qt.rgba(1, 1, 1, 0.12) : "transparent"

                    QbzIcon {
                        anchors.centerIn: parent
                        name: "list-ordered"
                        width: 24
                        height: 24
                        tintName: QbzShell.queueOpen ? "accent" : "secondary"
                    }

                    MouseArea {
                        id: queueArea
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.queueRequested()
                    }
                }
            }
        }
    }
}