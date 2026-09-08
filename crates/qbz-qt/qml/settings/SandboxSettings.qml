// Settings > Flatpak / Snap — the QML port of crates/qbz-ui/ui/settings/
// SandboxSettings.slint: the one-time permission commands a sandboxed
// install needs. Shown only when the process IS sandboxed
// (settings_qt/devtools.rs detects /.flatpak-info and $SNAP, the Slint's
// SandboxState.install-method).
//
// Commands are plain strings (never translated); the copy uses a hidden
// TextEdit's own clipboard (QML has no clipboard API of its own, and this
// port has no clipboard invokable to reuse).

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Column {
    property bool kioskHost: false

    id: root

    property var doc: ({})
    readonly property string method: (doc.dev || ({})).installMethod || ""

    QbzTheme { id: theme }

    spacing: 4

    component RowTitle: Text {
        width: parent ? parent.width : 0
        color: theme.textPrimary
        font.pixelSize: root.kioskHost ? (theme.fontBody) * 1.2 : (theme.fontBody)
        font.weight: theme.weightMedium
        wrapMode: Text.WordWrap
    }
    component RowNote: Text {
        width: parent ? parent.width : 0
        color: theme.textMuted
        font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        wrapMode: Text.WordWrap
    }
    // A full-width shell-command block with copy feedback. `command` may hold
    // several lines; the whole block copies as one payload.
    component CommandBlock: Rectangle {
        id: block
        property string command: ""
        property bool copied: false

        width: parent ? parent.width : 0
        height: Math.max(54, cmdText.implicitHeight + 20)
        radius: theme.radiusSm
        border.width: 1
        border.color: theme.borderSubtle
        color: theme.surfaceElevated

        Text {
            id: cmdText
            anchors.left: parent.left
            anchors.right: copyBtn.left
            anchors.verticalCenter: parent.verticalCenter
            anchors.leftMargin: 10
            anchors.rightMargin: 12
            text: block.command
            color: theme.textPrimary
            font.family: "monospace"
            font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            wrapMode: Text.WrapAnywhere
        }
        // Off-screen carrier: TextEdit.copy() is the only clipboard write QML
        // offers without a C++ seam.
        TextEdit {
            id: carrier
            visible: false
            text: block.command
        }
        SettingsButton { kioskHost: root.kioskHost;
            id: copyBtn
            anchors.right: parent.right
            anchors.rightMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            text: block.copied ? QbzSession.tr("Copied!", QbzSession.trRev)
                : QbzSession.tr("Copy", QbzSession.trRev)
            onClicked: {
                carrier.selectAll()
                carrier.copy()
                carrier.deselect()
                block.copied = true
                copiedTimer.restart()
            }
        }
        Timer {
            id: copiedTimer
            interval: 1500
            repeat: false
            onTriggered: block.copied = false
        }
    }

    // ============================== FLATPAK ==============================
    Column {
        visible: root.method === "flatpak"
        width: parent.width
        spacing: 4

        GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("FLATPAK SANDBOX", QbzSession.trRev) }
        RowNote {
            text: QbzSession.tr("QBZ is running inside a Flatpak sandbox. Some features need one-time permission grants — run the commands in a terminal, then restart QBZ. Grants persist across updates.", QbzSession.trRev)
        }
        Item { width: 1; height: 8 }

        RowTitle { text: QbzSession.tr("Exclusive DAC access (bit-perfect)", QbzSession.trRev) }
        RowNote {
            text: QbzSession.tr("Recommended for exclusive mode: lets QBZ ask PipeWire to hand over the DAC cleanly (D-Bus device reservation). Without it, PipeWire keeps holding the DAC and other apps keep mixing through it even with ALSA + hw + Exclusive selected.", QbzSession.trRev)
        }
        CommandBlock {
            command: "flatpak override --user --own-name=org.freedesktop.ReserveDevice1.* com.blitzfc.qbz"
        }
        Item { width: 1; height: 8 }

        RowTitle { text: QbzSession.tr("Music on NAS or external drives", QbzSession.trRev) }
        RowNote {
            text: QbzSession.tr("Grants access to a folder outside the sandbox. Repeat the command for each folder you need.", QbzSession.trRev)
        }
        CommandBlock {
            command: "flatpak override --user --filesystem=/path/to/your/music com.blitzfc.qbz"
        }
    }

    // ================================ SNAP ===============================
    Column {
        visible: root.method === "snap"
        width: parent.width
        spacing: 4

        GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("SNAP SANDBOX", QbzSession.trRev) }
        RowNote {
            text: QbzSession.tr("QBZ is running inside a Snap sandbox. Some audio interfaces need one-time connections — run these commands in a terminal, then restart QBZ.", QbzSession.trRev)
        }
        Item { width: 1; height: 8 }

        RowTitle { text: QbzSession.tr("Required audio connections", QbzSession.trRev) }
        CommandBlock {
            command: "sudo snap connect qbz-player:alsa\nsudo snap connect qbz-player:pulseaudio\nsudo snap connect qbz-player:pipewire\nsudo snap connect qbz-player:mpris"
        }
        Item { width: 1; height: 8 }

        RowTitle { text: QbzSession.tr("Optional — external drives & NAS", QbzSession.trRev) }
        CommandBlock {
            command: "sudo snap connect qbz-player:removable-media"
        }
    }
}
