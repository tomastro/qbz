// Now-Playing-view mode menu (PlayerBar.slint layout-menu, phase 18) —
// one QbzContextMenu shared by the full PlayerBar and the Small bar:
// New / Classic / Small / Large (with the current mode checked), then the
// window-mode rows. All three window-mode rows are LIVE: Immersive since the
// 2026-08-02 immersive-port §7 B2, Kiosk since the 2026-08-02 kiosk-port §8.1,
// Miniplayer since the 2026-08-03 miniplayer/tray-port §4.10 B2.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../controls"
import "../theme"

QbzContextMenu {
    id: root
    menuWidth: 196

    // The theme tokens. Declared HERE even though the BASE type
    // (controls/QbzContextMenu.qml) already has a `QbzTheme { id: theme }`:
    // a QML id belongs to the document that declares it, and a derived
    // document is a DIFFERENT scope — the base's context is created as a
    // child of this one, never as its parent, so `theme` was resolving to
    // nothing here and every binding below threw a silent ReferenceError
    // (leaving the row Rectangles at their default WHITE fill). The four
    // sibling menus built on the same base — CardMenu, AudioSettingsMenu,
    // PmFolderMenu, SidebarRowMenu — all declare their own; this file was
    // the lone outlier.
    QbzTheme { id: theme }

    Repeater {
        model: [
            { "label": QbzSession.tr("New", QbzSession.trRev), "icon": "panel-left", "mode": 0 },
            { "label": QbzSession.tr("Classic", QbzSession.trRev), "icon": "panel-right-close", "mode": 1 },
            { "label": QbzSession.tr("Small", QbzSession.trRev), "icon": "rows-3", "mode": 2 },
            { "label": QbzSession.tr("Large", QbzSession.trRev), "icon": "layout-grid", "mode": 3 },
        ]
        delegate: Rectangle {
            required property var modelData
            width: parent ? parent.width : 0
            height: 33
            radius: 5
            color: miArea.containsMouse ? theme.surfaceHover : "transparent"
            Row {
                anchors.fill: parent
                anchors.leftMargin: 8
                spacing: 8
                QbzIcon {
                    name: modelData.icon
                    width: 15
                    height: 15
                    anchors.verticalCenter: parent.verticalCenter
                    tintName: "secondary"
                }
                Text {
                    height: parent.height
                    width: parent.width - 23 - 22
                    text: modelData.label
                    color: theme.textSecondary
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                }
                QbzIcon {
                    visible: QbzShell.npbMode === modelData.mode
                    name: "check"
                    width: 13
                    height: 13
                    anchors.verticalCenter: parent.verticalCenter
                    tintName: "accent"
                }
            }
            MouseArea {
                id: miArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                    root.close()
                    QbzShell.npbSetMode(modelData.mode)
                }
            }
        }
    }

    Item {
        // Park this divider with the hidden waveform row. The live divider
        // below remains the single boundary before the window-mode actions.
        visible: false
        width: parent ? parent.width : 0
        height: 7
        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            height: 1
            color: theme.borderSubtle
        }
    }
    Rectangle {
        // The waveform seekbar is parked for a later release. Preserve this
        // control and its preference wiring, but do not expose a dead toggle.
        visible: false
        width: parent ? parent.width : 0
        height: 33
        radius: 5
        color: waveformArea.containsMouse ? theme.surfaceHover : "transparent"
        Row {
            anchors.fill: parent
            anchors.leftMargin: 8
            spacing: 8
            QbzIcon {
                name: "audio-lines"
                width: 15
                height: 15
                anchors.verticalCenter: parent.verticalCenter
                tintName: "secondary"
            }
            Text {
                height: parent.height
                width: parent.width - 23 - 22
                text: QbzSession.tr("Track waveform", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: 13
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            QbzIcon {
                visible: QbzShell.seekbarWaveform
                name: "check"
                width: 13
                height: 13
                anchors.verticalCenter: parent.verticalCenter
                tintName: "accent"
            }
        }
        MouseArea {
            id: waveformArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: {
                root.close()
                QbzBridge.settingsBool("seekbar-waveform", !QbzShell.seekbarWaveform)
            }
        }
    }

    // Divider + the window-mode rows (Miniplayer / Immersive / Kiosk), all
    // three live.
    Item {
        width: parent ? parent.width : 0
        height: 7
        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            height: 1
            color: theme.borderSubtle
        }
    }
    Repeater {
        // Window-mode rows. `live` flips per-row inside the shared delegate
        // (2026-08-02 immersive-port §7); Immersive went live in that
        // contract's B2, Kiosk in the 2026-08-02-kiosk-port §8.1, Miniplayer
        // in the 2026-08-03 miniplayer/tray-port §4.10 B2.
        //
        // Each row carries its own `action` because the delegate's MouseArea
        // used to call QbzImmersive.openFromMenu() unconditionally: flipping
        // any other row's `live` would have made it open Immersive.
        //
        // The Kiosk label mirrors the profile, 1:1 with the reference
        // (`shell/PlayerBar.slint:797`): in kiosk it reads "Desktop mode",
        // because the row is the way back out.
        //
        // THE MINIPLAYER ROW IS ABSENT UNDER THE KIOSK PROFILE, not dimmed —
        // 1:1 with `crates/qbz-ui/ui/shell/PlayerBarSmall.slint:819`, which
        // wraps its ContextMenuItem in `if !ShellState.kiosk-profile`. The
        // reference's reason is the appliance shell: a second floating,
        // draggable, always-on-top window has no way back on a touch-only
        // kiosk. Building the model in JS (not a ternary inside one row) is
        // what makes the row genuinely absent; the binding re-runs when
        // QbzShell.kioskProfile changes, which the live desktop/kiosk toggle
        // does. This closes ticket T-13 (contract §4.10.2): the kiosk contract
        // landed first, so this diff owes the gate and pays it here.
        model: {
            var rows = []
            if (!QbzShell.kioskProfile)
                rows.push({ "label": QbzSession.tr("Miniplayer", QbzSession.trRev),
                            "icon": "picture-in-picture-2", "live": true, "action": "miniplayer" })
            rows.push({ "label": QbzSession.tr("Immersive", QbzSession.trRev),
                        "icon": "maximize-2", "live": true, "action": "immersive" })
            rows.push({ "label": QbzShell.kioskProfile
                                 ? QbzSession.tr("Desktop mode", QbzSession.trRev)
                                 : QbzSession.tr("Kiosk mode", QbzSession.trRev),
                        "icon": "hard-drive", "live": true, "action": "kiosk" })
            return rows
        }
        delegate: Rectangle {
            required property var modelData
            width: parent ? parent.width : 0
            height: 33
            radius: 5
            opacity: modelData.live ? 1.0 : 0.45
            color: (modelData.live && modeArea.containsMouse)
                   ? theme.surfaceHover : "transparent"
            Row {
                anchors.fill: parent
                anchors.leftMargin: 8
                spacing: 8
                QbzIcon { name: modelData.icon; width: 15; height: 15; anchors.verticalCenter: parent.verticalCenter; tintName: "secondary" }
                Text {
                    height: parent.height
                    text: modelData.label
                    color: theme.textSecondary
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
            }
            MouseArea {
                id: modeArea
                anchors.fill: parent
                enabled: modelData.live
                hoverEnabled: true
                cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                onClicked: {
                    root.close()
                    if (modelData.action === "immersive")
                        QbzImmersive.openFromMenu()
                    else if (modelData.action === "kiosk")
                        QbzSession.toggleProfile()
                    else if (modelData.action === "miniplayer")
                        // Suppressed under gamescope INSIDE enter() — one
                        // predicate for this row and for Shift+M (§4.10.1).
                        QbzMini.enter()
                }
            }
        }
    }
}
