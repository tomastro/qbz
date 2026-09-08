// MiniExplicitBadge — the mini's parental-advisory chip (2026-08-03
// miniplayer/tray contract A-28, §4.2.3), port of `ExplicitBadge` at
// `crates/qbz-ui/ui/miniplayer/MiniBits.slint:9-23`.
//
// `MiniBits.slint` exports TWO components (ExplicitBadge :9, CoverArt :28) and
// a QML file declares exactly one root type, so the pair is split — this file
// and MiniCoverArt.qml (§15 trap 32).
//
// The glyph is a STYLED LETTER, not the official explicit.svg mask the Tauri
// build rendered at 13x13 / opacity 0.45. That is a deliberate divergence the
// reference records at MiniBits.slint:5-8 for app consistency, and it is
// REPRODUCED here rather than "fixed" back to the SVG.
//
// Takes NO properties — every call site mounts it bare.

import QtQuick
import "../theme"

Rectangle {
    id: root

    QbzTheme { id: theme }

    width: 13
    height: 13
    radius: 3
    antialiasing: true
    color: theme.alphaTier(12)

    Text {
        anchors.fill: parent
        // Literal, never translated (MiniBits.slint:16).
        text: "E"
        color: theme.alphaTier(55)
        font.pixelSize: 8
        font.weight: Font.DemiBold
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }
}
