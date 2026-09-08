// PmActionButton — the Playlist Manager's small hover action button
// (PlaylistManagerView.slint:234-258): a transparent r5 square that fills
// `surface-hover` under the pointer, holding one tinted glyph.
//
// Used by every playlist surface in the manager: the grid card's reorder
// chevrons (22/14) and its footer four (24/14), the list row's reorder pair
// (20/13) and its action strip (26/15), and the tree rows (26/15 default).
//
// INTERFACE NOTE (contract §2.5). The contract declares `name`, `filled`,
// `tintName` and `clicked()`. `btnSize` / `glyph` are ADDED here because the
// reference component takes exactly those two `in property <length>`s
// (`:237-238`) and the call sites use FOUR distinct pairs (03 §4.2) — without
// them the grid's 22 px reorder chevrons and its 24 px footer would render at
// 26 px and the card's own height arithmetic (pad 10 + 22 + 8 + 140 + …) would
// stop adding up. Both keep the reference's defaults, so a call site that omits
// them is the 26/15 form the contract describes. Reported as an interface
// addition rather than assumed silently.
//
// Rejected: controls/QbzIconButton.qml (radiusSm 8 not 5, its own 34 px
// footprint and a `secondary` idle tint) and controls/QbzToolButton.qml (a
// bordered 30/34 px elevated chip — the opposite look). This one is
// transparent, r5, and exists at four sizes.

import QtQuick
import com.blitzfc.qbz
import "../../theme"

Rectangle {
    id: root

    /// Icon name (QbzIcon `name`, ".svg" appended by the control).
    property string name: ""
    /// The "on" state (a favourited heart): the glyph stays `accent` even at
    /// rest and does NOT brighten on hover — `:250`.
    property bool filled: false
    /// Idle tint for the glyph when `filled` is false.
    property string tintName: "muted"
    /// Button footprint / glyph size (see the interface note above).
    property int btnSize: 26
    property int glyph: 15
    signal clicked()

    QbzTheme { id: theme }

    width: root.btnSize
    height: root.btnSize
    radius: 5
    color: btnArea.containsMouse ? theme.surfaceHover : "transparent"

    QbzIcon {
        anchors.centerIn: parent
        name: root.name
        width: root.glyph
        height: root.glyph
        // `filled ? accent : (hover ? text-primary : text-muted)` — the filled
        // arm wins in BOTH hover states (:250).
        tintName: root.filled ? "accent"
            : (btnArea.containsMouse ? "textPrimary" : root.tintName)
    }

    MouseArea {
        id: btnArea
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.clicked()
    }
}
