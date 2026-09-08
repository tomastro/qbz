// Settings > Developer > Diagnostics — the QML port of
// crates/qbz-ui/ui/settings/DiagnosticsPanel.slint, mounted as the LAST child
// of DeveloperSettings.qml exactly as the reference mounts it
// (DeveloperSettings.slint:92). Not a modal, not a view: a settings panel.
//
// The whole document arrives on ONE bridge property (QbzShell.diagnosticsJson,
// seeded with the full shape at construction) and is parsed behind a guard, the
// LogViewerModal pattern. `diagnostics_qt` publishes only while the master
// collapsible is expanded, which is why toggling it calls diagSetOpen().
//
// --- ROWS DELIBERATELY CUT FROM THE REFERENCE, and why -------------------
// The reference's Graphics and Environment sections carry WebKit2GTK, GTK,
// GSK Renderer, GDK Scale, GDK DPI Scale, Compositing Mode, Force DMA-BUF,
// Force X11 and the four WEBKIT_*/GDK_*/GSK_* env vars. Those are Tauri-era
// truths that Slint inherited verbatim. In a Qt process libwebkit2gtk and
// libgtk are not mapped at all, so their RUNTIME column can only ever read
// "—", and worse, their SAVED column would come out of a graphics.json this
// binary never reads or writes — foreign, stale state presented as this app's
// configuration. This port's standing rule is no dead rows, so they are gone.
// Do not "restore parity" by adding them back.
//
// `UI Loop Latency` is cut for a different reason: its producer
// (crates/qbz/src/ui_watchdog.rs) measures Slint's event-loop dispatch and has
// no Qt analogue. A Qt watchdog is its own piece of work.
//
// Everything true of a Qt process stays: OS/arch/kernel/distro/install method,
// the loaded glibc + ALSA + PipeWire + PulseAudio versions, the whole Audio
// section (saved vs the LIVE sink), GPU + renderer + desktop + Wayland + VM,
// the playback block, the QConnect block and the cast scan.
//
// Colours: the two status glyphs are 6-digit literals and safe to copy from
// the reference; everything else is a theme token. No 8-digit hex here — Slint
// writes #RRGGBBAA and Qt reads #AARRGGBB.
//
// No Timer, no Behavior, no animation: the "copied" flash and the 10s cast
// scan are both tokio sleeps in diagnostics_qt, which is what the shared-pulse
// rule wants.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Column {
    property bool kioskHost: false

    id: root

    QbzTheme { id: theme }

    width: parent ? parent.width : 0
    spacing: 4

    readonly property var doc: {
        try {
            return JSON.parse(QbzShell.diagnosticsJson)
        } catch (e) {
            return ({})
        }
    }
    property bool expanded: false

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    // ---------------------------------------------------------------- rows
    component DiagRowView: Item {
        id: rowView
        property var model: ({})
        property bool showSaved: false
        width: parent ? parent.width : 0
        height: Math.max(20, label.implicitHeight)

        Text {
            id: label
            width: rowView.width * 0.38
            text: rowView.model.label || ""
            color: theme.textMuted
            font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            elide: Text.ElideRight
        }
        Text {
            id: savedCol
            visible: rowView.showSaved
            x: rowView.width * 0.38
            width: rowView.showSaved ? rowView.width * 0.26 : 0
            text: rowView.model.saved || ""
            color: theme.textSecondary
            font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            elide: Text.ElideRight
        }
        Text {
            x: rowView.width * 0.38 + (rowView.showSaved ? rowView.width * 0.26 : 0)
            width: rowView.width - x - 20
            text: rowView.model.runtime || ""
            color: theme.textPrimary
            font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            elide: Text.ElideRight
        }
        // 1 = agrees, 2 = disagrees, anything else = not applicable.
        Text {
            x: rowView.width - 16
            width: 16
            horizontalAlignment: Text.AlignRight
            text: rowView.model.status === 1 ? "✓"
                : (rowView.model.status === 2 ? "✗" : "·")
            color: rowView.model.status === 1 ? "#4caf50"
                : (rowView.model.status === 2 ? "#f44336" : theme.textMuted)
            font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        }
    }

    // ------------------------------------------------------------ sections
    component DiagSection: Column {
        id: section
        property string title: ""
        property var rows: []
        property bool showSaved: false
        property bool startOpen: true
        property bool open: startOpen
        width: parent ? parent.width : 0
        spacing: 2
        visible: (section.rows || []).length > 0

        Item {
            id: sectionHeader
            width: parent.width
            height: 28
            activeFocusOnTab: visible && enabled
            Accessible.role: Accessible.Button
            Accessible.name: section.title
            Accessible.onPressAction: section.open = !section.open
            Keys.onPressed: function (event) {
                if (!event.isAutoRepeat
                        && (event.key === Qt.Key_Space
                            || event.key === Qt.Key_Return
                            || event.key === Qt.Key_Enter)) {
                    section.open = !section.open
                    event.accepted = true
                }
            }
            Rectangle {
                anchors.fill: parent
                radius: theme.radiusSm
                color: "transparent"
                border.width: sectionHeader.activeFocus ? 2 : 0
                border.color: theme.accent
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: section.title
                color: theme.textPrimary
                font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
                font.weight: theme.weightSemibold
            }
            QbzIcon {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: 13
                height: 13
                name: section.open ? "chevron-up" : "chevron-down"
                tintName: "muted"
            }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onPressed: sectionHeader.forceActiveFocus()
                onClicked: section.open = !section.open
            }
        }
        // Column headers, only while open and only where a saved column exists.
        Item {
            visible: section.open && section.showSaved
            width: parent.width
            height: visible ? 18 : 0
            Text {
                width: parent.width * 0.38
                text: root.t("Setting")
                color: theme.textDisabled
                font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            }
            Text {
                x: parent.width * 0.38
                text: root.t("Saved")
                color: theme.textDisabled
                font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            }
            Text {
                x: parent.width * 0.64
                text: root.t("Runtime")
                color: theme.textDisabled
                font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            }
        }
        Repeater {
            model: section.open ? (section.rows || []) : []
            delegate: DiagRowView {
                required property var modelData
                model: modelData
                showSaved: section.showSaved
            }
        }
        Item { width: 1; height: 6 }
    }

    // ------------------------------------------------------------- header
    Item {
        id: panelHeader
        width: parent.width
        height: 44
        activeFocusOnTab: visible && enabled
        Accessible.role: Accessible.Button
        Accessible.name: root.t("Diagnostics")
        Accessible.onPressAction: panelHeader.activate()
        function activate() {
            root.expanded = !root.expanded
            QbzShell.diagSetOpen(root.expanded)
            if (root.expanded && root.doc.loaded !== true && root.doc.loading !== true)
                QbzShell.diagRefresh()
        }
        Keys.onPressed: function (event) {
            if (!event.isAutoRepeat
                    && (event.key === Qt.Key_Space
                        || event.key === Qt.Key_Return
                        || event.key === Qt.Key_Enter)) {
                panelHeader.activate()
                event.accepted = true
            }
        }
        Rectangle {
            anchors.fill: parent
            radius: theme.radiusSm
            color: "transparent"
            border.width: panelHeader.activeFocus ? 2 : 0
            border.color: theme.accent
        }
        Column {
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            Text {
                text: root.t("Diagnostics")
                color: theme.textPrimary
                font.pixelSize: root.kioskHost ? (theme.fontBody) * 1.2 : (theme.fontBody)
                font.weight: theme.weightMedium
            }
            Text {
                text: root.t("Runtime vs saved configuration snapshot.")
                color: theme.textMuted
                font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            }
        }
        QbzIcon {
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            width: 15
            height: 15
            name: root.expanded ? "chevron-up" : "chevron-down"
            tintName: "muted"
        }
        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onPressed: panelHeader.forceActiveFocus()
            onClicked: panelHeader.activate()
        }
    }

    // The panel leaving the screen must stop the publisher too — Settings can
    // be navigated away from with the section still expanded.
    onVisibleChanged: if (!visible && root.expanded) QbzShell.diagSetOpen(false)

    // ---------------------------------------------------------------- body
    Column {
        visible: root.expanded
        width: parent.width
        spacing: 4

        Row {
            spacing: 8
            SettingsButton { kioskHost: root.kioskHost;
                text: root.doc.loading === true ? root.t("Scanning…") : root.t("Refresh")
                enabled: root.doc.loading !== true
                onClicked: QbzShell.diagRefresh()
            }
            SettingsButton { kioskHost: root.kioskHost;
                text: root.doc.copied === true ? root.t("Exported") : root.t("Export to clipboard")
                enabled: root.doc.loaded === true
                onClicked: QbzShell.diagExportClipboard()
            }
        }

        Text {
            visible: (root.doc.error || "") !== ""
            width: parent.width
            text: root.doc.error || ""
            color: "#f44336"
            font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            wrapMode: Text.WordWrap
        }

        Text {
            text: "QBZ v" + (root.doc.appVersion || "")
            color: theme.textMuted
            font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        }

        Item { width: 1; height: 4 }

        DiagSection {
            title: root.t("System")
            rows: root.doc.system || []
        }
        DiagSection {
            title: root.t("Playback")
            rows: root.doc.playback || []
        }
        DiagSection {
            title: root.t("Qobuz Connect")
            rows: root.doc.qconnect || []
        }

        // Cast is bespoke in the reference too: its rows only exist after a
        // scan, so it carries its own button and stays visible while empty.
        Column {
            width: parent.width
            spacing: 2
            Item {
                width: parent.width
                height: 28
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.t("Cast Discovery")
                    color: theme.textPrimary
                    font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
                    font.weight: theme.weightSemibold
                }
            }
            SettingsButton { kioskHost: root.kioskHost;
                text: root.doc.castScanning === true
                    ? root.t("Scanning…")
                    : root.t("Scan for devices")
                enabled: root.doc.castScanning !== true
                onClicked: QbzShell.diagCastScan()
            }
            Repeater {
                model: root.doc.cast || []
                delegate: DiagRowView {
                    required property var modelData
                    model: modelData
                }
            }
            Item { width: 1; height: 6 }
        }

        DiagSection {
            title: root.t("Audio")
            rows: root.doc.audio || []
            showSaved: true
        }
        DiagSection {
            title: root.t("Graphics")
            rows: root.doc.graphics || []
            showSaved: true
        }
        DiagSection {
            title: root.t("Environment")
            rows: root.doc.env || []
            startOpen: false
        }
    }
}
