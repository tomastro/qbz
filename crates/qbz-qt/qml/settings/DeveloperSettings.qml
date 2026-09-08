// Settings > Developer — the QML port of crates/qbz-ui/ui/settings/
// DeveloperSettings.slint.
//
// Shipped, in the Slint's order: QOBUZ CONNECT · LOGS. SETTINGS PORTABILITY
// moved to Settings > Import / Export (2026-09-02) with the rest of the
// data-moving rows (blacklist, account migration).
//
// Deltas vs the Slint (no dead rows):
// - LOGS offers TWO buttons where the Slint has one: "View logs" opens the
//   in-app viewer (copy / redact / upload) and "Open log file" hands the
//   on-disk sink to the desktop.
// - The inline DiagnosticsPanel is ported and mounted last, as in the
//   reference — with the GTK/WebKit/GSK/GDK rows CUT, because in a Qt process
//   they can only read "—" and their saved column would come from a file this
//   binary never writes. The panel's own header explains the cut.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Column {
    property bool kioskHost: false

    id: root

    property var doc: ({})
    readonly property var dev: doc.dev || ({})

    QbzTheme { id: theme }

    spacing: 4

    // ========================== QOBUZ CONNECT ============================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("QOBUZ CONNECT", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Connect diagnostics", QbzSession.trRev)
        description: QbzSession.tr("Live session topology and a rolling event log for debugging Qobuz Connect at runtime.", QbzSession.trRev)
        SettingsButton { kioskHost: root.kioskHost;
            text: QbzSession.tr("Open diagnostics", QbzSession.trRev)
            onClicked: QbzQConnect.diagSetOpen(true)
        }
    }

    SettingsSpacer { }

    // ============================== LOGS =================================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("LOGS", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Application logs", QbzSession.trRev)
        description: (root.dev.logPath || "") !== ""
            ? root.dev.logPath
            : QbzSession.tr("View the in-app log, copy it (secrets redacted), or upload it to share in a GitHub issue.", QbzSession.trRev)
        Row {
            spacing: 8
            // The description above has always promised "view the in-app log,
            // copy it (secrets redacted), or upload it" — until the viewer
            // landed, the only button here did none of those three.
            SettingsButton { kioskHost: root.kioskHost;
                text: QbzSession.tr("View logs", QbzSession.trRev)
                onClicked: QbzShell.logOpen()
            }
            SettingsButton { kioskHost: root.kioskHost;
                text: QbzSession.tr("Open log file", QbzSession.trRev)
                enabled: (root.dev.logPath || "") !== ""
                onClicked: QbzBridge.settingsString("open-log-file", "")
            }
        }
    }

    Text {
        visible: (root.dev.status || "") !== ""
        width: parent.width
        text: root.dev.status || ""
        color: theme.textMuted
        font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        wrapMode: Text.WordWrap
    }

    SettingsSpacer { }

    // ========================== DIAGNOSTICS ==============================
    // Last child, 1:1 with DeveloperSettings.slint:92.
    DiagnosticsPanel { kioskHost: root.kioskHost; }
}
