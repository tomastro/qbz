// CommandBlock — a copy-paste shell command / config snippet. QML port of
// crates/qbz-ui/ui/primitives/CommandBlock.slint.
//
// Surface-elevated card (radiusSm, no pills — ADR-008) with an optional
// caption, a monospace body that WRAPS, and a copy button that flips to a
// green check for 2 s. READ-ONLY: nothing here is ever executed. The height is
// content-sized, so a multi-line command grows the block; callers place it in
// a Column and never set a fixed height.
//
// THE CLIPBOARD. QML has no clipboard API of its own and this port has no
// clipboard invokable, so the copy rides an off-screen TextEdit's own
// `copy()` — the same carrier SandboxSettings.qml already uses. It is
// duplicated there rather than shared because that file's block is a
// different shape (a one-line row with a labelled "Copy" button); this one is
// the wizard's, with a caption above and an icon button pinned to the top.
//
// WHY THE ICON IS `accent` AND NOT `success`. The check glyph after a copy is
// green in the reference (a literal #3fae6a). `tintName: "success"` has NO qrc
// bake directory, so whenever the runtime tint set is unreachable the glyph
// would render as NOTHING at all rather than in a fallback colour
// (theme/QbzIcon.qml, "the one way a bake still silently lies"). The port
// already made this exact call once — WarningBanner.qml maps its `success`
// variant's glyph to the `accent` tint for the same reason — so this follows
// that precedent instead of inventing a one-glyph `success/` directory. The
// TEXT colours in this modal are unaffected: `theme.success` is #3fae6a
// verbatim, which is what the stepper dots and the read-back tick use.

import QtQuick
import "../theme"

Rectangle {
    id: root

    property string command: ""
    // Optional caption rendered above the command (e.g. what it does).
    property string caption: ""
    property bool justCopied: false

    QbzTheme { id: theme }

    width: parent ? parent.width : 0
    height: col.implicitHeight + 24
    radius: theme.radiusSm
    color: theme.surfaceElevated
    border.width: 1
    border.color: theme.borderSubtle

    Column {
        id: col
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 12
        spacing: 8

        Text {
            visible: root.caption !== ""
            width: parent.width
            text: root.caption
            color: theme.textSecondary
            font.pixelSize: theme.fontLegal
            font.weight: theme.weightMedium
            wrapMode: Text.WordWrap
        }

        Item {
            width: parent.width
            height: Math.max(cmdText.implicitHeight, 30)

            // Monospace body — wraps, takes the width left by the button.
            // WrapAnywhere, not WordWrap: these are shell lines and long paths
            // with no spaces to break on, and WordWrap would push them out of
            // the card instead of folding them.
            Text {
                id: cmdText
                anchors.left: parent.left
                anchors.right: copyBtn.left
                anchors.rightMargin: 10
                anchors.top: parent.top
                text: root.command
                color: theme.textPrimary
                font.family: "monospace"
                font.pixelSize: theme.fontLegal
                wrapMode: Text.WrapAnywhere
            }

            // Off-screen carrier: TextEdit.copy() is the only clipboard write
            // QML offers without a C++ seam.
            TextEdit {
                id: carrier
                visible: false
                text: root.command
            }

            // Copy button — fixed 30x30, pinned to the TOP of the row so a
            // tall command does not centre it halfway down the block.
            Rectangle {
                id: copyBtn
                anchors.right: parent.right
                anchors.top: parent.top
                width: 30
                height: 30
                radius: theme.radiusSm
                border.width: 1
                border.color: theme.borderSubtle
                color: copyTa.containsMouse ? theme.surfaceHover : theme.surfaceCard

                QbzIcon {
                    anchors.centerIn: parent
                    width: 15
                    height: 15
                    name: root.justCopied ? "check" : "copy"
                    tintName: root.justCopied
                        ? "accent"
                        : (copyTa.containsMouse ? "textPrimary" : "muted")
                }

                MouseArea {
                    id: copyTa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        carrier.selectAll()
                        carrier.copy()
                        carrier.deselect()
                        root.justCopied = true
                        resetTimer.restart()
                    }
                }
            }
        }
    }

    // Resets the "copied" check after 2s (reference: `reset-timer`, 2s).
    Timer {
        id: resetTimer
        interval: 2000
        repeat: false
        onTriggered: root.justCopied = false
    }
}
