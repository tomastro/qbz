// Settings > Import / Export — everything that moves data in or out of this
// QBZ install (2026-09-02):
//
//   SETTINGS PORTABILITY  the app's own settings bundle (moved here from
//                         Developer; the include-auth gate travels with it)
//   BLACKLIST             the portable blacklist, a JSON one user can hand to
//                         another QBZ user; import is additive
//   ACCOUNT MIGRATION     favorites + playlists from one Qobuz account into
//                         another, plus the local profile (next commits)
//
// All state comes from the settings document (`doc.importExport`); the rows
// call `QbzBridge.settingsString(...)` and Rust republishes the status lines.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Column {
    property bool kioskHost: false

    id: root

    property var doc: ({})
    /// View-level migration setup surface. Kept nullable for isolated previews.
    property var migrationSetupModal: null
    readonly property var ie: doc.importExport || ({})

    QbzTheme { id: theme }

    spacing: 4

    // ====================== SETTINGS PORTABILITY =========================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("SETTINGS PORTABILITY", QbzSession.trRev) }
    Text {
        width: parent.width
        text: QbzSession.tr("This bundle holds the app's own settings: audio, playback, appearance, integrations. It does not contain your Qobuz favorites, playlists, blacklist or purchases.", QbzSession.trRev)
        color: theme.textMuted
        font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        wrapMode: Text.WordWrap
    }
    // The `--include-auth` gate. DELIBERATELY NOT PERSISTED: the reference's
    // default-OFF is re-asserted every time its modal opens, so the user
    // re-decides per export. `includeAuth` is session state and resets
    // whenever the panel is left.
    property bool includeAuth: false
    onVisibleChanged: if (!visible) root.includeAuth = false

    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Include sign-in credentials", QbzSession.trRev)
        description: QbzSession.tr("The bundle will contain your tokens. Anyone who opens the file can sign in as you.", QbzSession.trRev)
        QbzToggle { kioskHost: root.kioskHost;
            checked: root.includeAuth
            onToggled: function (v) { root.includeAuth = v }
        }
    }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Export settings…", QbzSession.trRev)
        description: QbzSession.tr("Save a portable bundle of your settings to move to another machine or the qbzd daemon.", QbzSession.trRev)
        SettingsButton { kioskHost: root.kioskHost;
            text: QbzSession.tr("Export…", QbzSession.trRev)
            onClicked: QbzBridge.settingsString(
                "export-settings", root.includeAuth ? "with-auth" : "")
        }
    }
    Text {
        visible: (root.ie.settingsStatus || "") !== ""
        width: parent.width
        text: root.ie.settingsStatus || ""
        color: theme.textMuted
        font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        wrapMode: Text.WordWrap
    }

    SettingsSpacer { }

    // ============================ BLACKLIST ==============================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("BLACKLIST", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Export blacklist…", QbzSession.trRev)
        description: QbzSession.tr("Save your blocked artists and albums as a file you can hand to another QBZ user.", QbzSession.trRev)
        SettingsButton { kioskHost: root.kioskHost;
            text: QbzSession.tr("Export…", QbzSession.trRev)
            enabled: ((root.ie.blacklistArtists || 0) + (root.ie.blacklistAlbums || 0)) > 0
            onClicked: QbzBridge.settingsString("export-blacklist", "")
        }
    }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Import blacklist…", QbzSession.trRev)
        description: QbzSession.tr("Add blocked artists and albums from a file. Nothing already on your blacklist is removed.", QbzSession.trRev)
        SettingsButton { kioskHost: root.kioskHost;
            text: QbzSession.tr("Import…", QbzSession.trRev)
            onClicked: QbzBridge.settingsString("import-blacklist", "")
        }
    }
    Text {
        visible: (root.ie.blacklistStatus || "") !== ""
        width: parent.width
        text: root.ie.blacklistStatus || ""
        color: theme.textMuted
        font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        wrapMode: Text.WordWrap
    }

    SettingsSpacer { }

    // ======================== ACCOUNT MIGRATION ==========================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("ACCOUNT MIGRATION", QbzSession.trRev) }
    Text {
        width: parent.width
        text: QbzSession.tr("Moves favorites, playlists and followed playlists from one Qobuz account into another, adding only what is missing and deleting nothing on either side. It does not move subscriptions or purchases: those stay with the Qobuz account that holds them.", QbzSession.trRev)
        color: theme.textMuted
        font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        wrapMode: Text.WordWrap
    }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Create migration snapshot", QbzSession.trRev)
        description: QbzSession.tr("Sign in with the account you are moving FROM, then save its favorites and playlists to a file in its QBZ profile.", QbzSession.trRev)
        SettingsButton { kioskHost: root.kioskHost;
            text: QbzSession.tr("Create…", QbzSession.trRev)
            enabled: !root.ie.migrationBusy
            onClicked: QbzBridge.settingsString("account-snapshot", "")
        }
    }
    Text {
        visible: (root.ie.snapshotStatus || "") !== ""
        width: parent.width
        text: root.ie.snapshotStatus || ""
        color: theme.textMuted
        font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        wrapMode: Text.WordWrap
    }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Migrate into this account", QbzSession.trRev)
        description: (root.ie.snapshots || []).length > 0
            ? QbzSession.tr("Review the saved source profiles, choose what to copy, and confirm the signed-in destination account.", QbzSession.trRev)
            : QbzSession.tr("No snapshots yet. Sign in with the old account and create one first.", QbzSession.trRev)
        SettingsButton { kioskHost: root.kioskHost;
            text: QbzSession.tr("Migrate…", QbzSession.trRev)
            enabled: !root.ie.migrationBusy
                && (root.ie.snapshots || []).length > 0
            onClicked: if (root.migrationSetupModal) root.migrationSetupModal.open()
        }
    }
    Text {
        visible: (root.ie.migrationStatus || "") !== ""
        width: parent.width
        text: root.ie.migrationStatus || ""
        color: theme.textMuted
        font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
        wrapMode: Text.WordWrap
    }
}
