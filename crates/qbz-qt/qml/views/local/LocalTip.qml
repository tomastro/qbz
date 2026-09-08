// Hover tooltip for the Local Library's icon-only toolbar controls.
//
// Slint routes these through a shell-level overlay (TooltipState, set by
// LocalLibraryView.slint's CircleIconBtn / RailBulkBtn). This port has no
// tooltip overlay singleton, so the pattern is the one the quality badge
// already uses: a QtQuick.Controls ToolTip (a POPUP, so it survives the
// `clip: true` containers these toolbars live in) styled to the Slint
// numbers — 24px tall, radius sm, surface-elevated on a 1px border-subtle,
// 11px medium secondary text, 9px side padding, 6px above its anchor.
//
// `delay` is 0 and the caller drives `visible` from its own hover area (the
// quality-badge convention), so the tip tracks hover exactly.

import QtQuick
import QtQuick.Controls
import "../../theme"

ToolTip {
    id: tip

    delay: 0
    timeout: -1
    padding: 0
    implicitHeight: 24
    x: parent ? Math.round((parent.width - width) / 2) : 0
    y: -height - 6

    contentItem: Text {
        text: tip.text
        color: theme.textSecondary
        font.pixelSize: 11
        font.weight: theme.weightMedium
        verticalAlignment: Text.AlignVCenter
        leftPadding: 9
        rightPadding: 9
    }
    background: Rectangle {
        color: theme.surfaceElevated
        radius: theme.radiusSm
        border.width: 1
        border.color: theme.borderSubtle
        // The theme instance lives here because a ToolTip's default property
        // is its content; ids are component-scoped, so contentItem sees it.
        QbzTheme { id: theme }
    }
}
