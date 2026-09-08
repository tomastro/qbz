// Full-view account-migration progress. The operation belongs to Rust and
// keeps running if this panel is dismissed; this overlay only makes the
// long-lived phases, account direction and elapsed time explicit.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    property bool kioskHost: false

    id: root

    property var doc: ({})
    readonly property var ie: doc.importExport || ({})
    readonly property bool running: ie.migrationRunning === true
    readonly property bool succeeded: ie.migrationSucceeded === true
    readonly property int currentStep: Math.max(1, ie.migrationStep || 1)
    readonly property int stepTotal: Math.max(1, ie.migrationStepTotal || 6)
    readonly property int itemDone: Math.max(0, ie.migrationProgressDone || 0)
    readonly property int itemTotal: Math.max(0, ie.migrationProgressTotal || 0)
    readonly property real itemProgress: itemTotal > 0
        ? Math.min(1, itemDone / itemTotal) : (running ? 0.08 : 0)
    readonly property real overallProgress: (!running && succeeded) ? 1
        : Math.max(0, Math.min(1,
            ((currentStep - 1) + itemProgress) / stepTotal))

    property bool openedForRun: false
    property bool dismissed: true
    property int elapsedSeconds: 0

    QbzTheme { id: theme }

    function beginRun() {
        root.openedForRun = true
        root.dismissed = false
        root.elapsedSeconds = 0
        Qt.callLater(function () { closeButton.forceActiveFocus() })
    }
    function close() {
        root.dismissed = true
        root.restoreShellFocus()
    }
    function restoreShellFocus() {
        var item = root
        while (item.parent) {
            if (item.parent.isQbzShellRoot === true) {
                item.parent.forceActiveFocus()
                return
            }
            item = item.parent
        }
    }
    function elapsedLabel() {
        const minutes = Math.floor(root.elapsedSeconds / 60)
        const seconds = root.elapsedSeconds % 60
        return minutes + ":" + (seconds < 10 ? "0" : "") + seconds
    }
    function stepLabel(step) {
        switch (step) {
        case 1: return QbzSession.tr("Reading this account…", QbzSession.trRev)
        case 2: return QbzSession.tr("Adding favorites…", QbzSession.trRev)
        case 3: return QbzSession.tr("Adding playlists…", QbzSession.trRev)
        case 4: return QbzSession.tr("Following playlists…", QbzSession.trRev)
        case 5: return QbzSession.tr("Copying local profile…", QbzSession.trRev)
        default: return QbzSession.tr("Refreshing migrated data…", QbzSession.trRev)
        }
    }

    onRunningChanged: if (running) beginRun()
    Component.onCompleted: if (running) beginRun()

    Timer {
        interval: 1000
        repeat: true
        running: root.running
        onTriggered: root.elapsedSeconds += 1
    }

    visible: openedForRun && !dismissed
    enabled: visible
    z: 3200

    Keys.onEscapePressed: function (event) {
        root.close()
        event.accepted = true
    }

    Rectangle {
        anchors.fill: parent
        radius: theme.radiusMd
        color: "#bf000000"
        MouseArea {
            anchors.fill: parent
            onClicked: root.close()
            onWheel: function (wheel) { wheel.accepted = true }
        }
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 80, 620)
        height: Math.min(parent.height * 0.9, 680)
        radius: theme.radiusLg
        color: theme.surfaceCard
        border.width: 1
        border.color: theme.borderSubtle

        MouseArea {
            anchors.fill: parent
            onWheel: function (wheel) { wheel.accepted = true }
        }

        Item {
            id: header
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.right: parent.right
            height: 68

            Text {
                anchors.left: parent.left
                anchors.leftMargin: 24
                anchors.right: headerSpinner.left
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                text: QbzSession.tr("ACCOUNT MIGRATION", QbzSession.trRev)
                color: theme.textPrimary
                font.pixelSize: root.kioskHost ? (theme.fontSection) * 1.2 : (theme.fontSection)
                font.weight: theme.weightSemibold
                elide: Text.ElideRight
            }
            QbzSpinner {
                id: headerSpinner
                anchors.right: parent.right
                anchors.rightMargin: 24
                anchors.verticalCenter: parent.verticalCenter
                size: 20
                visible: root.running
                spinning: root.running
            }
        }
        Rectangle {
            id: headerDivider
            anchors.top: header.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            height: 1
            color: theme.borderSubtle
        }

        Item {
            id: footer
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: 70

            SettingsButton { kioskHost: root.kioskHost;
                id: closeButton
                anchors.right: parent.right
                anchors.rightMargin: 24
                anchors.verticalCenter: parent.verticalCenter
                text: QbzSession.tr("Close", QbzSession.trRev)
                onClicked: root.close()
            }
        }
        Rectangle {
            id: footerDivider
            anchors.bottom: footer.top
            anchors.left: parent.left
            anchors.right: parent.right
            height: 1
            color: theme.borderSubtle
        }

        Flickable {
            anchors.top: headerDivider.bottom
            anchors.bottom: footerDivider.top
            anchors.left: parent.left
            anchors.right: parent.right
            clip: true
            contentWidth: width
            contentHeight: body.implicitHeight + 48
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: body
                x: 24
                y: 24
                width: parent.width - 48
                spacing: 16

                WarningBanner {
                    width: parent.width
                    variant: "warning"
                    title: QbzSession.tr("This only happens once. Please wait for it to finish and do not close the window.", QbzSession.trRev)
                }

                Text {
                    width: parent.width
                    text: QbzSession.tr("You can close this panel; the migration will continue in the background.", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
                    wrapMode: Text.WordWrap
                }

                Rectangle {
                    width: parent.width
                    height: identityCol.implicitHeight + 24
                    radius: theme.radiusMd
                    color: theme.surfaceElevated
                    border.width: 1
                    border.color: theme.borderSubtle

                    Column {
                        id: identityCol
                        x: 12
                        y: 12
                        width: parent.width - 24
                        spacing: 8

                        Text {
                            width: parent.width
                            text: QbzSession.tr("From", QbzSession.trRev)
                                + ": " + (root.ie.migrationSource || "—")
                            color: theme.textSecondary
                            font.pixelSize: root.kioskHost ? (theme.fontBody) * 1.2 : (theme.fontBody)
                            wrapMode: Text.WordWrap
                        }
                        Text {
                            width: parent.width
                            text: QbzSession.tr("To", QbzSession.trRev)
                                + ": " + (root.ie.migrationTarget || "—")
                            color: theme.textPrimary
                            font.pixelSize: root.kioskHost ? (theme.fontBody) * 1.2 : (theme.fontBody)
                            font.weight: theme.weightSemibold
                            wrapMode: Text.WordWrap
                        }
                    }
                }

                Column {
                    width: parent.width
                    spacing: 8

                    Item {
                        width: parent.width
                        height: 20
                        Text {
                            anchors.left: parent.left
                            anchors.right: elapsed.right
                            anchors.rightMargin: 12
                            text: QbzSession.tr("Step {} of {}", QbzSession.trRev)
                                .replace("{}", root.currentStep)
                                .replace("{}", root.stepTotal)
                            color: theme.textPrimary
                            font.pixelSize: root.kioskHost ? (theme.fontBody) * 1.2 : (theme.fontBody)
                            font.weight: theme.weightSemibold
                        }
                        Text {
                            id: elapsed
                            anchors.right: parent.right
                            text: QbzSession.tr("Elapsed: {}", QbzSession.trRev)
                                .replace("{}", root.elapsedLabel())
                            color: theme.textMuted
                            font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
                        }
                    }
                    Rectangle {
                        width: parent.width
                        height: 10
                        radius: 5
                        color: theme.borderSubtle
                        clip: true
                        Rectangle {
                            width: parent.width * root.overallProgress
                            height: parent.height
                            radius: 5
                            color: theme.accent
                        }
                    }
                    Text {
                        width: parent.width
                        text: root.ie.migrationStatus || root.stepLabel(root.currentStep)
                        color: theme.textSecondary
                        font.pixelSize: root.kioskHost ? (theme.fontBody) * 1.2 : (theme.fontBody)
                        wrapMode: Text.WordWrap
                    }
                }

                Column {
                    width: parent.width
                    spacing: 6

                    Repeater {
                        model: root.stepTotal
                        delegate: Item {
                            required property int index
                            readonly property int number: index + 1
                            readonly property bool complete: number < root.currentStep
                                || (!root.running && root.succeeded)
                            readonly property bool active: number === root.currentStep
                                && root.running
                            width: parent ? parent.width : 0
                            height: 30

                            Rectangle {
                                id: stepDot
                                anchors.left: parent.left
                                anchors.verticalCenter: parent.verticalCenter
                                width: 24
                                height: 24
                                radius: 12
                                color: parent.complete || parent.active
                                    ? theme.accent : "transparent"
                                border.width: parent.complete || parent.active ? 0 : 1
                                border.color: theme.borderSubtle

                                Text {
                                    anchors.centerIn: parent
                                    text: stepDot.parent.complete ? "✓" : stepDot.parent.number
                                    color: stepDot.parent.complete || stepDot.parent.active
                                        ? theme.onAccent : theme.textMuted
                                    font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
                                    font.weight: theme.weightSemibold
                                }
                            }
                            Text {
                                anchors.left: stepDot.right
                                anchors.leftMargin: 10
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                text: root.stepLabel(parent.number)
                                color: parent.active ? theme.textPrimary : theme.textSecondary
                                font.pixelSize: root.kioskHost ? (theme.fontBody) * 1.2 : (theme.fontBody)
                                font.weight: parent.active
                                    ? theme.weightSemibold : theme.weightRegular
                                elide: Text.ElideRight
                            }
                        }
                    }
                }
            }
        }
    }
}
