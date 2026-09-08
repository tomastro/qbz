// Audio settings flyout — the port of the "Audio settings" PopupWindow that
// BOTH now-playing bars mount (`shell/PlayerBar.slint:686-705` and
// `shell/PlayerBarSmall.slint:635-712`). Two rows, Normalization and Gapless,
// and nothing else: the Slint popup is 252x118 with exactly these two
// ToggleRows (`PlayerBar.slint:80-115` is the file-private ToggleRow this
// reproduces — 44px tall, 12px side padding, 12px gap to the switch, label
// text-primary 13px over hint text-muted 10px, spacing 1).
//
// It replaces a direct-toggle button that both bars had shipped with LOCAL
// state (`normLocal` / `property bool normOn: false`), which is what made
// normalization impossible to turn off: the bars never learned the persisted
// value, so the button drew OFF on a fresh launch and the first click SET IT
// ON. The state now comes from the published document, which is seeded at
// shell entry and republished after every apply.
//
// Mounted as a QbzContextMenu (the port's popup primitive, same as
// ViewModeMenu next to it) rather than a raw Popup, so the panel chrome,
// placement clamping and close policy are the ones every other flyout in the
// bar already uses.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../controls"
import "../theme"

QbzContextMenu {
    id: root
    // PlayerBar.slint:689 — `width: 252px`.
    menuWidth: 252

    /// The live document both rows read. The host passes its already-parsed
    /// `settingsDoc` so this file never parses the JSON a second time.
    property var doc: ({})

    QbzTheme { id: theme }

    Repeater {
        model: [
            {
                "key": "normalization",
                "label": QbzSession.tr("Normalization", QbzSession.trRev),
                "hint": QbzSession.tr("Level loudness across tracks", QbzSession.trRev),
                "on": root.doc.normalization === true,
            },
            {
                "key": "gapless",
                "label": QbzSession.tr("Gapless", QbzSession.trRev),
                "hint": QbzSession.tr("Seamless track transitions", QbzSession.trRev),
                "on": root.doc.gapless === true,
            },
        ]
        delegate: Item {
            required property var modelData
            width: parent ? parent.width : 0
            // ToggleRow.slint:86 — `height: 44px`.
            height: 44

            Row {
                anchors.fill: parent
                // ToggleRow.slint:88-90 — padding 12/12, spacing 12.
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                spacing: 12

                Column {
                    width: parent.width - 12 - rowToggle.width
                    height: parent.height
                    // The label/hint pair is vertically centred as a block
                    // (ToggleRow's inner VerticalLayout `alignment: center`),
                    // so the Column is centred rather than filling.
                    anchors.verticalCenter: parent.verticalCenter
                    // ToggleRow.slint:93 — `spacing: 1px`.
                    spacing: 1

                    Text {
                        width: parent.width
                        text: modelData.label
                        color: theme.textPrimary
                        // ToggleRow.slint:98 — literal 13px, not a token.
                        font.pixelSize: 13
                        elide: Text.ElideRight
                    }
                    Text {
                        width: parent.width
                        text: modelData.hint
                        color: theme.textMuted
                        // ToggleRow.slint:103 — literal 10px.
                        font.pixelSize: 10
                        elide: Text.ElideRight
                    }
                }
                QbzToggle {
                    id: rowToggle
                    anchors.verticalCenter: parent.verticalCenter
                    checked: modelData.on
                    onToggled: function (v) {
                        QbzBridge.settingsBool(modelData.key, v)
                    }
                }
            }
        }
    }
}
