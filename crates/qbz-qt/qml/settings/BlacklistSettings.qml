// Settings > Blacklist — the QML port of crates/qbz-ui/ui/settings/
// ContentFilteringSettings.slint: ONE 64px row (blind-eye glyph + "Blacklist" +
// "{n} artists, {m} albums blocked", with a "(disabled)" suffix when the
// feature is off) and a right-aligned "Manage" button that opens the full-page
// Blacklist Manager. There is no enable toggle here in the Slint either
// (:13-16) — the switch lives inside the manager.
//
// Layout, verbatim from the reference (:66-115): outer spacing 4; a 64px row
// with spacing 24; the left group horizontal-stretch 1 / spacing 12 with the
// blind-eye 18x18 tint secondary, then a text column at spacing 3 holding
// "Blacklist" (text-primary 15 medium) over the status line (text-muted, a
// LITERAL 12px — not a Typography token); the ManageButton right-aligned and
// vertically centred.
//
// ManageButton (:26-64) is `width: max(content + 32, 160)`, height 34, Radius.sm,
// 1px border-subtle, surfaceHover on hover else surfaceElevated, content centred
// with spacing 6: label "Manage" text-secondary 15 medium, then chevron-right
// 16x16 tint secondary. controls/SettingsButton.qml already matches every one of
// those metrics, so it is reused with a trailing-icon slot rather than forked.
// Accepted deltas (the port's settings-button voice): content padding 24 vs 32,
// icon 15 vs 16, label textPrimary vs textSecondary.
//
// The counters come from the per-user blacklist singleton
// (crate::artist_blacklist, published into the settings document by
// settings_qt/devtools.rs::blacklist_counts), so they tell the truth
// immediately after a mutation — every blacklist mutation republishes the
// settings document as well as its own, which is what makes
// Manage -> unblock -> Back show fresh numbers.

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

    Item {
        width: parent.width
        height: root.kioskHost ? 112 : 64

        Row {
            anchors.fill: parent
            // The Slint's 24px gap between the left group and the button.
            spacing: 24

            Row {
                id: leftGroup
                width: Math.max(0, parent.width - 24 - manageBtn.width)
                anchors.verticalCenter: parent.verticalCenter
                spacing: 12

                QbzIcon {
                    anchors.verticalCenter: parent.verticalCenter
                    name: "blind-eye"
                    width: 18
                    height: 18
                    tintName: "secondary"
                }
                Column {
                    // 30 = the 18px glyph + the 12px row spacing.
                    width: Math.max(0, parent.width - 30)
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 3
                    Text {
                        width: parent.width
                        text: QbzSession.tr("Blacklist", QbzSession.trRev)
                        color: theme.textPrimary
                        font.pixelSize: root.kioskHost ? (theme.fontBody) * 1.2 : (theme.fontBody)
                        font.weight: theme.weightMedium
                    }
                    Text {
                        width: parent.width
                        // Two placeholders, replaced left to right:
                        // artists first, then albums.
                        text: (root.dev.blacklistEnabled === false
                                ? QbzSession.tr("{} artists, {} albums blocked (disabled)", QbzSession.trRev)
                                : QbzSession.tr("{} artists, {} albums blocked", QbzSession.trRev))
                            .replace("{}", root.dev.blacklistArtists || 0)
                            .replace("{}", root.dev.blacklistAlbums || 0)
                        color: theme.textMuted
                        font.pixelSize: root.kioskHost ? (12) * 1.2 : (12)
                        wrapMode: Text.WordWrap
                    }
                }
            }

            SettingsButton { kioskHost: root.kioskHost;
                id: manageBtn
                anchors.verticalCenter: parent.verticalCenter
                text: QbzSession.tr("Manage", QbzSession.trRev)
                trailingIconName: "chevron-right"
                onClicked: QbzBlacklist.openManager()
            }
        }
    }
}
