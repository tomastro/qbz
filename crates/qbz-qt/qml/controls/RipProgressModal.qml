// What the rip is doing right now.
//
// The pane's one-line "Ripping 3/7 · 45%" says how far along it is and nothing
// else. A rip is minutes of a drive spinning against a disc the user is
// holding, and the two questions that line cannot answer are the ones that
// matter: which track is being read, and is it safe to take the CD out.
//
// SOFT LOCK, not a hard one. There is no way to keep a tray shut from here, so
// the honest thing is to say it plainly and keep saying it while the drive is
// busy — the modal cannot be dismissed into forgetting, because the pane's
// indicator reopens it.
//
// Mounted ONCE in AppShell next to the wizard: it outlives the pane (the user
// can navigate away mid-rip and the job keeps going), so it must not be
// parented into a view that gets destroyed.
//
// Opened by QbzShell.ripProgressOpen — QML-side state, because nothing about
// "is this panel showing" belongs in Rust.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    readonly property var doc: {
        try { return JSON.parse(QbzLocal.localRipStatus) } catch (e) { return ({}) }
    }
    readonly property bool active: doc.active === true
    // The flag rides the job's own document (see `rip_qt::Status`): the pane
    // that opens this panel and the shell that mounts it are in different
    // subtrees, and the bridge is the only channel both can reach.
    visible: doc.panelOpen === true

    function tr(s) { return QbzSession.tr(s, QbzSession.trRev) }

    QbzTheme { id: theme }

    Rectangle {
        anchors.fill: parent
        color: "#bf000000"
        MouseArea {
            anchors.fill: parent
            onClicked: QbzLocal.ripPanel(false)
            // Wheel-lock (the DiscoverConfigModal rule).
            onWheel: function (wheel) { wheel.accepted = true }
        }
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 80, 560)
        height: Math.min(parent.height * 0.85, 560)
        radius: theme.radiusLg
        color: theme.surfaceCard
        border.width: 1
        border.color: theme.borderSubtle
        clip: true

        MouseArea {
            anchors.fill: parent
            // Wheel-lock (the DiscoverConfigModal rule).
            onWheel: function (wheel) { wheel.accepted = true }
        }

        // ---- Header -----------------------------------------------------
        Item {
            id: header
            width: parent.width
            height: 76
            Column {
                anchors.left: parent.left
                anchors.leftMargin: 24
                anchors.right: closeX.left
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                spacing: 3
                Text {
                    width: parent.width
                    text: root.doc.album || root.tr("Rip this CD")
                    color: theme.textPrimary
                    font.pixelSize: theme.fontSection
                    font.weight: theme.weightSemibold
                    elide: Text.ElideRight
                }
                Text {
                    width: parent.width
                    text: root.tr("Writing FLAC files to") + " " + (root.doc.destination || "")
                    color: theme.textMuted
                    font.pixelSize: theme.fontLegal
                    elide: Text.ElideMiddle
                }
            }
            Item {
                id: closeX
                anchors.right: parent.right
                anchors.rightMargin: 24
                anchors.verticalCenter: parent.verticalCenter
                width: 28
                height: 28
                QbzIcon {
                    anchors.centerIn: parent
                    name: "x"
                    width: 17
                    height: 17
                    tintName: cArea.containsMouse ? "textPrimary" : "muted"
                }
                MouseArea {
                    id: cArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: QbzLocal.ripPanel(false)
                    // It hides the PANEL, never the job — saying so is the
                    // difference between a close button and a cancel button,
                    // and this one is not a cancel button.
                    onContainsMouseChanged: tips.hover(containsMouse, closeX, "rip-hide",
                        root.tr("Hide this panel — the rip keeps going"))
                }
            }
        }
        Rectangle { y: header.height; width: parent.width; height: 1; color: theme.borderSubtle }

        // ---- The soft lock ----------------------------------------------
        Rectangle {
            id: warn
            y: header.height + 1
            width: parent.width
            height: 52
            color: theme.surfaceElevated
            visible: root.active
            Row {
                anchors.left: parent.left
                anchors.leftMargin: 24
                anchors.right: parent.right
                anchors.rightMargin: 24
                anchors.verticalCenter: parent.verticalCenter
                spacing: 10
                QbzIcon {
                    name: "circle-alert"
                    width: 16
                    height: 16
                    anchors.verticalCenter: parent.verticalCenter
                    tintName: "amber"
                }
                Text {
                    width: parent.width - 26
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.tr("Ripping — don't eject the disc until this finishes.")
                    color: theme.textSecondary
                    font.pixelSize: theme.fontLegal
                    wrapMode: Text.WordWrap
                }
            }
        }

        // ---- Overall bar --------------------------------------------------
        Item {
            id: overall
            y: header.height + 1 + (warn.visible ? warn.height : 0)
            width: parent.width
            height: 58
            Column {
                anchors.left: parent.left
                anchors.leftMargin: 24
                anchors.right: parent.right
                anchors.rightMargin: 24
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8
                Row {
                    width: parent.width
                    Text {
                        text: root.active
                            ? root.tr("Track") + " " + ((root.doc.index || 0) + 1)
                              + "/" + (root.doc.count || 0)
                            : root.tr("Finished")
                        color: theme.textSecondary
                        font.pixelSize: theme.fontLegal
                    }
                    Item { width: parent.width - 160; height: 1 }
                    Text {
                        width: 160
                        horizontalAlignment: Text.AlignRight
                        text: Math.round((root.doc.overall || 0) * 100) + "%"
                        color: theme.textMuted
                        font.pixelSize: theme.fontLegal
                    }
                }
                Rectangle {
                    width: parent.width
                    height: 4
                    radius: 2
                    color: theme.surfaceHover
                    Rectangle {
                        width: Math.max(0, Math.min(parent.width,
                                                    parent.width * (root.doc.overall || 0)))
                        height: parent.height
                        radius: 2
                        color: theme.accent
                    }
                }
            }
        }
        Rectangle {
            y: overall.y + overall.height
            width: parent.width
            height: 1
            color: theme.borderSubtle
        }

        // ---- Per-track list -----------------------------------------------
        ListView {
            y: overall.y + overall.height + 1
            width: parent.width
            height: card.height - y - (footer.visible ? footer.height : 0)
            clip: true
            model: root.doc.tracks || []
            boundsBehavior: Flickable.StopAtBounds
            delegate: Item {
                id: trackRow
                required property var modelData
                required property int index
                // done | current | pending — derived from ONE number rather
                // than published per row, so the list can never disagree with
                // the bar above it.
                readonly property string state: index < (root.doc.index || 0)
                    ? "done"
                    : (index === (root.doc.index || 0) && root.active ? "current" : "pending")
                width: ListView.view.width
                height: 44

                Row {
                    anchors.fill: parent
                    anchors.leftMargin: 24
                    anchors.rightMargin: 24
                    spacing: 10

                    Item {
                        width: 16
                        height: 16
                        anchors.verticalCenter: parent.verticalCenter
                        QbzIcon {
                            anchors.fill: parent
                            visible: trackRow.state === "done"
                            name: "circle-check-big"
                            tintName: "accent"
                        }
                        QbzSpinner {
                            anchors.centerIn: parent
                            size: 14
                            visible: trackRow.state === "current"
                        }
                        Text {
                            anchors.centerIn: parent
                            visible: trackRow.state === "pending"
                            text: trackRow.modelData.number
                            color: theme.textMuted
                            font.pixelSize: theme.fontLegal
                        }
                    }
                    Column {
                        width: parent.width - 26 - 54
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 4
                        Text {
                            width: parent.width
                            text: trackRow.modelData.title
                            color: trackRow.state === "pending"
                                ? theme.textMuted : theme.textPrimary
                            font.pixelSize: theme.fontLegal
                            elide: Text.ElideRight
                        }
                        // The per-track bar, on the CURRENT row only. A row of
                        // seven bars all sitting at 0% or 100% is noise; the
                        // one that is moving is the one worth drawing.
                        Rectangle {
                            visible: trackRow.state === "current"
                            width: parent.width
                            height: 3
                            radius: 2
                            color: theme.surfaceHover
                            Rectangle {
                                width: Math.max(0, Math.min(parent.width,
                                    parent.width * (root.doc.fraction || 0)))
                                height: parent.height
                                radius: 2
                                color: theme.accent
                            }
                        }
                    }
                    Text {
                        width: 44
                        anchors.verticalCenter: parent.verticalCenter
                        horizontalAlignment: Text.AlignRight
                        visible: trackRow.state === "current"
                        text: Math.round((root.doc.fraction || 0) * 100) + "%"
                        color: theme.textMuted
                        font.pixelSize: theme.fontLegal
                    }
                }
                Rectangle {
                    anchors.bottom: parent.bottom
                    width: parent.width
                    height: 1
                    color: theme.borderSubtle
                }
            }
        }

        // ---- Stop --------------------------------------------------------
        //
        // It stops the JOB and deletes nothing, and both halves are said out
        // loud. A "cancel" that silently removed four finished tracks would
        // throw away minutes the user already waited for, and a "cancel" that
        // left them without saying so is a folder they later find and cannot
        // explain.
        Item {
            id: footer
            width: parent.width
            height: 84
            anchors.bottom: parent.bottom
            visible: root.active

            Rectangle { width: parent.width; height: 1; color: theme.borderSubtle }

            Text {
                anchors.left: parent.left
                anchors.leftMargin: 24
                anchors.right: stopBtn.left
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                wrapMode: Text.WordWrap
                text: root.tr("Files already written stay on disk. Delete them yourself if you don't want them.")
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
            }
            SettingsButton {
                id: stopBtn
                anchors.right: parent.right
                anchors.rightMargin: 24
                anchors.verticalCenter: parent.verticalCenter
                btnHeight: 34
                minWidth: 0
                enabled: root.doc.cancelling !== true
                text: root.doc.cancelling === true
                    ? root.tr("Stopping…") : root.tr("Stop ripping")
                onClicked: QbzLocal.ripCancel()
                HoverHandler {
                    onHoveredChanged: tips.hover(hovered, stopBtn, "rip-stop",
                        root.tr("Stop after the current track. Nothing is deleted."))
                }
            }
        }
    }

    QbzTooltip {
        id: tips
        anchors.fill: parent
        z: 900
    }
}