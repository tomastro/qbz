// Settings > Integrations — the QML port of
// crates/qbz-ui/ui/settings/IntegrationsSettings.slint. Row order, group
// headers and auth-gating mirror the Slint 1:1; all state rides the single
// settingsJson document (integrations_qt.rs over the SAME scrobbler /
// discover / ui_prefs / offline-queue stores the Slint app uses).
//
// Every integration here is STRICTLY OPT-IN: nothing connects, fires or
// queues until the user signs the service in and its toggles are on.
//
// NOTE: the Last.fm connect flow opens the system browser (open::that) — the
// offscreen smoke never clicks it.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Column {
    property bool kioskHost: false

    id: root

    property var doc: ({})
    // Password drafts never enter settingsJson. Keep the live token inside
    // this component until the user explicitly submits it.
    property string listenbrainzTokenDraft: ""
    /// The view-level SettingsConfirmHost (SettingsView.qml), for the
    /// destructive "Clear listening history" row. Null in previews.
    property var confirmHost: null

    QbzTheme { id: theme }

    spacing: 4

    function submitListenBrainzToken() {
        if (root.doc.listenbrainzBusy === true)
            return
        QbzBridge.settingsString("listenbrainz-token", root.listenbrainzTokenDraft)
    }

    // ======================= RECOMMENDATIONS =============================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("RECOMMENDATIONS", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Show Recommendations in Discover", QbzSession.trRev)
        description: QbzSession.tr("A personalized Discover tab built from your listening. For the best results connect Last.fm and ListenBrainz below — MusicBrainz is used automatically to match releases.", QbzSession.trRev)
        QbzToggle { kioskHost: root.kioskHost;
            checked: root.doc.showRecommendations === true
            onToggled: function (v) { QbzBridge.settingsBool("show-recommendations", v) }
        }
    }

    SettingsSpacer { }

    // =========================== METADATA ================================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("METADATA", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("MusicBrainz", QbzSession.trRev)
        description: QbzSession.tr("Enable artist relationships and enhanced metadata from MusicBrainz. No telemetry — it only matches releases to enrich artist pages and playlist song suggestions. Turn off to disable those sections.", QbzSession.trRev)
        QbzToggle { kioskHost: root.kioskHost;
            checked: root.doc.musicbrainzEnabled === true
            onToggled: function (v) { QbzBridge.settingsBool("musicbrainz", v) }
        }
    }

    SettingsSpacer { }

    // ============================ PRIVACY ================================
    // The local listen log (qbz_app::listen_log): one row per play, on this
    // disk only, per account. Default ON. "Clear" is DELETE + VACUUM. The
    // toggle's state comes from the per-user store (`listen_meta.paused`),
    // not from a pref, so it follows the account.
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("PRIVACY", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Listening history", QbzSession.trRev)
        description: QbzSession.tr("Keep a local log of what you play: track, source, how much of it you heard and when. It stays on this machine, per account, and never leaves it unless you enable a scrobbler.", QbzSession.trRev)
        QbzToggle { kioskHost: root.kioskHost;
            checked: root.doc.listenHistoryEnabled !== false
            onToggled: function (v) { QbzBridge.settingsBool("listen-history", v) }
        }
    }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Clear listening history", QbzSession.trRev)
        description: QbzSession.tr("Deletes every play recorded on this machine for this account. Scrobbles already sent are not affected.", QbzSession.trRev)
        SettingsButton { kioskHost: root.kioskHost;
            danger: true
            text: QbzSession.tr("Clear", QbzSession.trRev)
            onClicked: {
                if (!root.confirmHost) {
                    QbzBridge.integrationsAction("listen-history-clear")
                    return
                }
                root.confirmHost.ask(
                    QbzSession.tr("Clear listening history", QbzSession.trRev),
                    QbzSession.tr("Deletes every play recorded on this machine for this account. Scrobbles already sent are not affected.", QbzSession.trRev),
                    QbzSession.tr("Clear", QbzSession.trRev),
                    function () { QbzBridge.integrationsAction("listen-history-clear") })
            }
        }
    }

    SettingsSpacer { }

    // ========================== SCROBBLERS ===============================
    // Master header row (toggle + collapse chevron), then the body gated
    // on enabled && !collapsed (IntegrationsSettings.slint:113-169).
    Item {
        width: parent.width
        height: root.kioskHost ? 112 : 64
        Column {
            anchors.left: parent.left
            anchors.right: scrobbleControl.left
            anchors.rightMargin: 24
            anchors.verticalCenter: parent.verticalCenter
            spacing: 3
            Text {
                width: parent.width
                text: QbzSession.tr("SCROBBLERS", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: root.kioskHost ? (11) * 1.2 : (11)
                font.letterSpacing: 1.5
                font.weight: theme.weightSemibold
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: QbzSession.tr("Send your plays to Last.fm and ListenBrainz. Works for Qobuz, local, and Plex tracks. This switch also decides whether Recommendations may read your listening history from those services.", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: root.kioskHost ? (12) * 1.2 : (12)
                wrapMode: Text.WordWrap
            }
        }
        Row {
            id: scrobbleControl
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8
            // Collapse chevron (only when the master toggle is on).
            Rectangle {
                id: collapseButton
                visible: root.doc.scrobbleEnabled === true
                width: root.kioskHost ? 44 : 28
                height: root.kioskHost ? 44 : 28
                radius: theme.radiusSm
                color: colArea.containsMouse ? theme.surfaceHover : "transparent"
                activeFocusOnTab: visible && enabled
                border.width: activeFocus ? 2 : 0
                border.color: theme.accent
                Accessible.role: Accessible.Button
                Accessible.name: QbzSession.tr("Scrobbling", QbzSession.trRev)
                Accessible.onPressAction: collapseButton.activate()
                function activate() {
                    QbzBridge.settingsBool(
                        "scrobble-collapse", root.doc.scrobbleUiCollapsed !== true)
                }
                Keys.onPressed: function (event) {
                    if (!event.isAutoRepeat
                            && (event.key === Qt.Key_Space
                                || event.key === Qt.Key_Return
                                || event.key === Qt.Key_Enter)) {
                        collapseButton.activate()
                        event.accepted = true
                    }
                }
                QbzIcon {
                    anchors.centerIn: parent
                    // 28px hit-box / 18px glyph (IntegrationsSettings.slint:129).
                    name: root.doc.scrobbleUiCollapsed === true ? "chevron-right" : "chevron-down"
                    width: 18
                    height: 18
                    tintName: "secondary"
                }
                MouseArea {
                    id: colArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onPressed: collapseButton.forceActiveFocus()
                    onClicked: collapseButton.activate()
                }
            }
            QbzToggle { kioskHost: root.kioskHost;
                anchors.verticalCenter: parent.verticalCenter
                checked: root.doc.scrobbleEnabled === true
                onToggled: function (v) { QbzBridge.settingsBool("scrobble-enable", v) }
            }
        }
    }

    Column {
        visible: root.doc.scrobbleEnabled === true && root.doc.scrobbleUiCollapsed !== true
        width: parent.width
        spacing: 4

        SettingRow { kioskHost: root.kioskHost;
            label: QbzSession.tr("Scrobble while logged out", QbzSession.trRev)
            description: QbzSession.tr("Continue sending plays to connected scrobblers without a Qobuz session.", QbzSession.trRev)
            QbzToggle { kioskHost: root.kioskHost;
                checked: root.doc.allowLoggedOutScrobbling !== false
                onToggled: function (v) {
                    QbzBridge.settingsBool("scrobble-logged-out", v)
                }
            }
        }

        SettingsSpacer { }

        // ------------------------- LAST.FM ------------------------------
        GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("LAST.FM", QbzSession.trRev) }
        SettingRow { kioskHost: root.kioskHost;
            label: QbzSession.tr("Scrobble to Last.fm", QbzSession.trRev)
            description: root.doc.lastfmAuthed === true
                ? QbzSession.tr("Signed in as {}.", QbzSession.trRev).replace("{}", root.doc.lastfmUsername || "")
                : QbzSession.tr("Connect your Last.fm account to enable scrobbling.", QbzSession.trRev)
            QbzToggle { kioskHost: root.kioskHost;
                checked: root.doc.lastfmEnabled === true
                onToggled: function (v) { QbzBridge.settingsBool("lastfm-enable", v) }
            }
        }
        SettingRow { kioskHost: root.kioskHost;
            visible: root.doc.lastfmAuthed !== true
            label: QbzSession.tr("Connect", QbzSession.trRev)
            description: QbzSession.tr("Authorize QBZ in your browser, then click Finish.", QbzSession.trRev)
            Row {
                spacing: 8
                SettingsButton { kioskHost: root.kioskHost;
                    text: root.doc.lastfmBusy === true ? QbzSession.tr("Working...", QbzSession.trRev) : QbzSession.tr("Connect Last.fm", QbzSession.trRev)
                    enabled: root.doc.lastfmBusy !== true
                    onClicked: QbzBridge.integrationsAction("lastfm-connect")
                }
                SettingsButton { kioskHost: root.kioskHost;
                    visible: (root.doc.lastfmAuthUrl || "") !== ""
                    text: QbzSession.tr("Open authorize page", QbzSession.trRev)
                    onClicked: QbzBridge.integrationsAction("lastfm-open-auth-url")
                }
                SettingsButton { kioskHost: root.kioskHost;
                    visible: (root.doc.lastfmAuthUrl || "") !== ""
                    text: root.doc.lastfmBusy === true
                        ? QbzSession.tr("Working...", QbzSession.trRev)
                        : QbzSession.tr("Finish", QbzSession.trRev)
                    enabled: root.doc.lastfmBusy !== true
                    onClicked: QbzBridge.integrationsAction("lastfm-finish")
                }
            }
        }
        SettingRow { kioskHost: root.kioskHost;
            visible: root.doc.lastfmAuthed === true
            label: QbzSession.tr("Disconnect Last.fm", QbzSession.trRev)
            description: QbzSession.tr("Sign out of Last.fm.", QbzSession.trRev)
            SettingsButton { kioskHost: root.kioskHost;
                danger: true
                text: QbzSession.tr("Disconnect", QbzSession.trRev)
                onClicked: QbzBridge.integrationsAction("lastfm-disconnect")
            }
        }

        SettingsSpacer { }

        // ----------------------- LISTENBRAINZ ----------------------------
        GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("LISTENBRAINZ", QbzSession.trRev) }
        SettingRow { kioskHost: root.kioskHost;
            label: QbzSession.tr("Scrobble to ListenBrainz", QbzSession.trRev)
            description: root.doc.listenbrainzBusy === true
                ? QbzSession.tr("Validating...", QbzSession.trRev)
                : root.doc.listenbrainzAuthed === true
                ? QbzSession.tr("Signed in as {}.", QbzSession.trRev).replace("{}", root.doc.listenbrainzUsername || "")
                : QbzSession.tr("Paste your ListenBrainz user token to enable scrobbling.", QbzSession.trRev)
            QbzToggle { kioskHost: root.kioskHost;
                checked: root.doc.listenbrainzEnabled === true
                onToggled: function (v) { QbzBridge.settingsBool("listenbrainz-enable", v) }
            }
        }
        SettingRow { kioskHost: root.kioskHost;
            visible: root.doc.listenbrainzAuthed !== true
            label: QbzSession.tr("User token", QbzSession.trRev)
            description: QbzSession.tr("From listenbrainz.org/settings.", QbzSession.trRev)
            QbzLineEdit { kioskHost: root.kioskHost;
                id: lbTokenField
                isPassword: true
                enabled: root.doc.listenbrainzBusy !== true
                placeholder: QbzSession.tr("ListenBrainz token", QbzSession.trRev)
                // Typing only updates the local draft. Enter submits once;
                // focus loss merely settles the draft, so a button click can
                // never race an implicit blur validation.
                onEdited: function (v) { root.listenbrainzTokenDraft = v }
                onCommitted: function (v) { root.listenbrainzTokenDraft = v }
                onAccepted: function (v) {
                    root.listenbrainzTokenDraft = v
                    root.submitListenBrainzToken()
                }
                // The row hides once the sign-in lands; drop the typed token
                // with it (Slint resets listenbrainz-token-input on success).
                onVisibleChanged: if (!visible) {
                    text = ""
                    root.listenbrainzTokenDraft = ""
                }
            }
        }
        SettingRow { kioskHost: root.kioskHost;
            visible: root.doc.listenbrainzAuthed !== true
            label: QbzSession.tr("Save token", QbzSession.trRev)
            description: QbzSession.tr("Validates the token against ListenBrainz.", QbzSession.trRev)
            SettingsButton { kioskHost: root.kioskHost;
                text: root.doc.listenbrainzBusy === true
                    ? QbzSession.tr("Validating...", QbzSession.trRev)
                    : QbzSession.tr("Connect ListenBrainz", QbzSession.trRev)
                enabled: root.doc.listenbrainzBusy !== true
                    && root.listenbrainzTokenDraft.trim().length > 0
                onClicked: root.submitListenBrainzToken()
            }
        }
        SettingRow { kioskHost: root.kioskHost;
            visible: root.doc.listenbrainzAuthed === true
            label: QbzSession.tr("Disconnect ListenBrainz", QbzSession.trRev)
            description: QbzSession.tr("Sign out of ListenBrainz.", QbzSession.trRev)
            SettingsButton { kioskHost: root.kioskHost;
                danger: true
                text: QbzSession.tr("Disconnect", QbzSession.trRev)
                onClicked: QbzBridge.integrationsAction("listenbrainz-disconnect")
            }
        }

        // Status line (scrobble.rs set_status; kind 1 info / 2 ok / 3 error).
        Text {
            visible: (root.doc.integrationsStatusText || "") !== ""
            width: parent.width
            text: root.doc.integrationsStatusText || ""
            color: root.doc.integrationsStatusKind === 2 ? theme.success
                : root.doc.integrationsStatusKind === 3 ? theme.danger
                : theme.textMuted
            font.pixelSize: root.kioskHost ? (12) * 1.2 : (12)
            wrapMode: Text.WordWrap
        }
    }

    SettingsSpacer { }

    // =========================== DISCORD =================================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("DISCORD", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Discord Rich Presence", QbzSession.trRev)
        description: QbzSession.tr("Show what you're listening to as your Discord status. Opt-in — Discord must be running.", QbzSession.trRev)
        QbzToggle { kioskHost: root.kioskHost;
            checked: root.doc.discordEnabled === true
            onToggled: function (v) { QbzBridge.settingsBool("discord-rpc", v) }
        }
    }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Flatpak socket access", QbzSession.trRev)
        description: QbzSession.tr("Flatpak install and the presence isn't showing? Grant access to Discord's IPC socket, then restart QBZ:\nflatpak override --user --filesystem=xdg-run/discord-ipc-0 com.blitzfc.qbz", QbzSession.trRev)
    }

    // ========================= QOBUZ LINKS ===============================
    // The OS-level claim on the official client's `qobuzapp://` scheme, as a
    // choice (link_handler_qt.rs): on Windows the official client is where
    // most users still are, so it is opt-IN there; elsewhere it defaults on.
    // `qbz://` (our own scheme) is always registered and is not this switch.
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("QOBUZ LINKS", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Open Qobuz links in QBZ", QbzSession.trRev)
        description: QbzSession.tr("Make QBZ the handler for qobuzapp:// links from your browser. Off, the official Qobuz app keeps them; QBZ's own qbz:// links always open here.", QbzSession.trRev)
        QbzToggle { kioskHost: root.kioskHost;
            checked: root.doc.qobuzLinksEnabled === true
            onToggled: function (v) { QbzBridge.settingsBool("qobuz-links", v) }
        }
    }
}
