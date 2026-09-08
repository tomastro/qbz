// Right-side Lyrics panel — QML port of
// crates/qbz-ui/ui/shell/LyricsSidebar.slint.
//
// Header (LYRICS overline + "{title} - {artist}" meta + display-settings
// trigger + close X), body by status (loading / no-lyrics / offline-miss /
// error / the shared lines view), the floating translate toggle over the
// body's bottom-right corner, and the source-attribution footer.
//
// The sync is NOT the 1 Hz playback poll any more: LyricsSyncEngine ticks at
// ~30Hz off the player's millisecond position and pulls the active line +
// the karaoke fraction from Rust (word-anchored on Qobuz wsync documents).
// The 280px-adapted type scale from the reference is applied here (S 11/13 ·
// M 13/15 · L 16/19 · XL 20/24) and handed to the shared lines view together
// with the persisted display prefs.
//
// Translation failures and copy confirmation stay panel-local as an inline
// notice strip; the shared toast host is reserved for app-wide actions.
//
// LyricsLinesView and LyricsSyncEngine are host-agnostic: Immersive focus /
// split modes and the miniplayer mount the same document and sync math with
// their own layout knobs.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root
    color: "transparent"
    // Clipped ONLY when stacked with the other panel. The drag ghost really
    // does leave this rect, but in the single-panel case the panel coincides
    // exactly with queueColumn, which already clips (AppShell.qml) — so the
    // scissor is pure duplication there, and duplication costs a batch root.
    clip: QbzShell.queueOpen && QbzShell.lyricsOpen

    QbzTheme { id: theme }

    // The document Rust publishes: {status, synced, provider, error, lines}.
    readonly property var doc: JSON.parse(QbzLyrics.docJson)
    readonly property var lines: doc.lines !== undefined && doc.lines !== null ? doc.lines : []
    readonly property bool synced: doc.synced === true
    readonly property bool ready: doc.status === 2 && root.lines.length > 0

    // 280px-adapted type scale (LyricsSidebar.slint size matrix).
    readonly property int sizeInactive: QbzLyrics.sizeIndex === 0 ? 11
        : QbzLyrics.sizeIndex === 2 ? 16
        : QbzLyrics.sizeIndex === 3 ? 20 : 13
    readonly property int sizeActive: QbzLyrics.sizeIndex === 0 ? 13
        : QbzLyrics.sizeIndex === 2 ? 19
        : QbzLyrics.sizeIndex === 3 ? 24 : 15

    Component.onCompleted: QbzLyrics.boot()

    // ---- sync engine -------------------------------------------------
    LyricsSyncEngine {
        id: syncEngine
        // Tick only while the panel is on screen and the doc can be followed.
        gateOpen: root.visible && root.ready && root.synced
    }

    // Every doc commit resets the ladder and re-lands on the right line
    // (the Slint `kick()` on commit / panel open).
    Connections {
        target: QbzLyrics
        function onDocJsonChanged() {
            syncEngine.reset()
            syncEngine.kick()
        }
    }

    Column {
        anchors.fill: parent
        spacing: 0

        // --- Header: overline/meta + controls trigger + close ----------
        Item {
            width: parent.width
            height: 56

            Column {
                anchors.left: parent.left
                anchors.leftMargin: theme.spacingSm
                anchors.right: controlsRow.left
                anchors.rightMargin: theme.spacingSm
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Text {
                    text: QbzSession.tr("LYRICS", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: 11
                    font.weight: theme.weightSemibold
                    font.letterSpacing: 1.5
                }
                Text {
                    visible: QbzLyrics.trackTitle !== ""
                    width: parent.width
                    text: QbzLyrics.trackTitle + " - " + QbzLyrics.trackArtist
                    color: theme.textSecondary
                    font.pixelSize: 12
                    elide: Text.ElideRight
                }
            }
            Row {
                id: controlsRow
                anchors.right: parent.right
                anchors.rightMargin: theme.spacingSm
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8
                // Display settings — opens the controls flyout.
                Rectangle {
                    id: dsButton
                    width: 32
                    height: 32
                    radius: theme.radiusSm
                    color: dsArea.containsMouse ? theme.surfaceHover : "transparent"
                    QbzIcon {
                        anchors.centerIn: parent
                        name: "sliders-horizontal"
                        width: 16
                        height: 16
                        tintName: dsArea.containsMouse ? "textPrimary" : "secondary"
                    }
                    MouseArea {
                        id: dsArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: dsFlyout.opened ? dsFlyout.close() : dsFlyout.open()
                        ToolTip.visible: containsMouse && !dsFlyout.opened
                        ToolTip.delay: 500
                        ToolTip.text: QbzSession.tr("Display settings", QbzSession.trRev)
                    }
                }
                // Close X (Slint-convention, homologated with Queue).
                Rectangle {
                    width: 28
                    height: 28
                    radius: theme.radiusSm
                    color: closeArea.containsMouse ? theme.surfaceHover : "transparent"
                    QbzIcon {
                        anchors.centerIn: parent
                        name: "x"
                        width: 15
                        height: 15
                        tintName: closeArea.containsMouse ? "textPrimary" : "secondary"
                    }
                    MouseArea {
                        id: closeArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: QbzShell.toggleLyrics()
                    }
                }
            }
        }
        Rectangle {
            width: parent.width
            height: 1
            color: theme.borderSubtle
        }

        // --- Body by status ----------------------------------------------
        Item {
            id: body
            width: parent.width
            height: parent.height - 57 - sourceLine.height - noticeStrip.height

            // Loading.
            Column {
                visible: root.doc.status === 1
                anchors.centerIn: parent
                spacing: 10
                QbzSpinner { size: 24; anchors.horizontalCenter: parent.horizontalCenter }
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: QbzSession.tr("Fetching lyrics...", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: theme.fontLegal
                }
            }

            // No lyrics (idle / not-found / ready-with-zero-lines).
            Column {
                visible: root.doc.status === 0 || root.doc.status === 3
                    || (root.doc.status === 2 && root.lines.length === 0)
                anchors.centerIn: parent
                width: parent.width - 32
                spacing: 12
                Text {
                    width: parent.width
                    text: QbzSession.tr("No lyrics available", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: theme.fontLegal
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                }
                Column {
                    width: parent.width
                    spacing: 2
                    Text {
                        width: parent.width
                        text: QbzSession.tr("Missing lyrics? You can contribute them to LRCLIB:", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: 10
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                    }
                    Text {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: "lrclibup.boidu.dev"
                        color: lrcArea.containsMouse ? theme.accentHover : theme.accent
                        font.pixelSize: 10
                        MouseArea {
                            id: lrcArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: QbzShell.openExternalUrl("https://lrclibup.boidu.dev/")
                        }
                    }
                }
            }

            // Offline with nothing cached.
            Text {
                visible: root.doc.status === 5
                anchors.centerIn: parent
                width: parent.width - 32
                text: QbzSession.tr("Lyrics unavailable offline", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
            }

            // Error.
            Text {
                visible: root.doc.status === 4
                anchors.centerIn: parent
                width: parent.width - 32
                text: root.doc.error !== undefined ? root.doc.error : ""
                color: theme.danger
                font.pixelSize: theme.fontLegal
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
            }

            // The lines (status 2, lines > 0).
            LyricsLinesView {
                id: linesView
                anchors.fill: parent
                visible: root.ready
                lines: root.lines
                synced: root.synced
                showTranslation: QbzLyrics.showTranslation
                sync: syncEngine
                sizeInactive: root.sizeInactive
                sizeActive: root.sizeActive
                dimmingMode: QbzLyrics.dimmingMode
                autoFollow: QbzLyrics.autoFollow
                uppercase: QbzLyrics.uppercase
                fontIndex: QbzLyrics.fontIndex
                liteFill: QbzLyrics.liteFill
                activeColor: QbzLyrics.useCustomColor ? QbzLyrics.customColor : theme.accent
                onSeekRequested: function (timeMs) {
                    if (QbzPlayer.npDurationSecs > 0)
                        QbzPlayer.seek(timeMs / 1000 / QbzPlayer.npDurationSecs)
                }
            }

            // Floating translate toggle — bottom-right overlay of the body,
            // mounted only while the current doc advertises translations.
            Rectangle {
                visible: QbzLyrics.translationAvailable && root.ready
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.rightMargin: 12
                anchors.bottomMargin: 12
                width: 32
                height: 32
                radius: 4
                color: trArea.containsMouse ? theme.surfaceHover : "transparent"
                opacity: QbzLyrics.translationBusy ? 0.5 : 1.0
                QbzIcon {
                    anchors.centerIn: parent
                    name: "translate"
                    width: 20
                    height: 20
                    tintName: QbzLyrics.showTranslation
                        ? "accent" : (trArea.containsMouse ? "textPrimary" : "muted")
                }
                MouseArea {
                    id: trArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: QbzLyrics.toggleTranslation()
                    ToolTip.visible: containsMouse
                    ToolTip.delay: 500
                    ToolTip.text: QbzLyrics.showTranslation
                        ? QbzSession.tr("Hide lyrics translation", QbzSession.trRev)
                        : QbzSession.tr("Show lyrics translation", QbzSession.trRev)
                }
            }
        }

        // --- Panel-local inline notice ------------------------------------
        Rectangle {
            id: noticeStrip
            width: parent.width
            height: QbzLyrics.notice !== "" ? noticeText.implicitHeight + 14 : 0
            visible: height > 0
            color: theme.surfaceElevated
            Text {
                id: noticeText
                anchors.left: parent.left
                anchors.right: dismiss.left
                anchors.leftMargin: theme.spacingSm
                anchors.rightMargin: 4
                anchors.verticalCenter: parent.verticalCenter
                text: QbzLyrics.notice
                color: theme.textSecondary
                font.pixelSize: 11
                wrapMode: Text.WordWrap
            }
            QbzIcon {
                id: dismiss
                anchors.right: parent.right
                anchors.rightMargin: theme.spacingSm
                anchors.verticalCenter: parent.verticalCenter
                name: "x"
                width: 12
                height: 12
                tintName: "muted"
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: QbzLyrics.clearNotice()
                }
            }
            // Auto-dismiss, toast-style.
            Timer {
                interval: 5000
                repeat: false
                running: QbzLyrics.notice !== ""
                onTriggered: QbzLyrics.clearNotice()
            }
        }

        // --- Source attribution ------------------------------------------
        Row {
            id: sourceLine
            visible: root.ready
            width: parent.width
            leftPadding: theme.spacingSm
            rightPadding: theme.spacingSm
            topPadding: 3
            bottomPadding: 5
            spacing: 3
            Text {
                text: QbzSession.tr("Lyrics source", QbzSession.trRev) + ":"
                color: theme.textMuted
                font.pixelSize: 9
                anchors.verticalCenter: parent.verticalCenter
            }
            Text {
                text: root.doc.provider === "lrclib" ? "LRCLIB"
                    : root.doc.provider === "ovh" ? "lyrics.ovh" : "Qobuz"
                color: theme.accent
                font.pixelSize: 9
                font.weight: theme.weightSemibold
                anchors.verticalCenter: parent.verticalCenter
            }
        }
    }

    // --- Display-settings flyout -----------------------------------------
    Popup {
        id: dsFlyout
        // Right edge aligned with the close button's right edge, 8px below
        // the trigger (the reference's anchor math).
        x: root.width - theme.spacingSm - 260
        y: 8 + 32 + 8
        width: 260
        height: flyoutBody.implicitHeight
        padding: 0
        closePolicy: Popup.CloseOnPressOutside | Popup.CloseOnEscape
        background: null
        contentItem: LyricsControlsFlyout {
            id: flyoutBody
            canCopy: root.ready
        }
    }
}
