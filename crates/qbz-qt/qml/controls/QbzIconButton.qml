// QbzIconButton — the square icon button (Slint BarControls IconButton /
// HeaderIconButton), consolidated in phase 22 from PlayerBar.BarIconBtn +
// NowPlayingBarSmall.BarIconBtn (verbatim twins), QueuePanel.PanelIconBtn
// and HeaderBar.HeaderIconBtn.
// Arms: `btnSize` (32 default; 30 compact panels, 36 header), `iconSize`,
// `active` (accent glyph), `activeBackground` (HeaderBar: elevated fill
// when active), `btnEnabled` (dims 0.3 + muted glyph, disarms).
//
// LIGHT-THEME CORRECTNESS: this button's fills are THEME surfaces
// (surface-hover / surface-elevated / transparent), so its glyphs must be
// theme tokens, never fixed colours.
//
// This used to pick between two FIXED bakes by reading the live token's
// lightness (`textPrimary.hslLightness > 0.5 ? "primary" : "black"`) — a
// 2-value approximation of a token, from the era when assets/icons/<tint>/
// was all there was and "primary" was literally #ffffff. That era is over:
// QbzIcon now resolves theme-following tints through src/icon_tint_qt.rs,
// which bakes the REAL hex of the live theme's token, so "textPrimary" and
// "secondary" are exact under all 36 themes and the ternary is dead weight.
// The old TODO asking for a QbzTheme.tintFor(token) helper is closed by the
// same change — there is nothing left to hoist.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: btn
    property string name: ""
    property int btnSize: 32
    property int iconSize: 16
    property bool active: false
    property bool activeBackground: false
    property bool btnEnabled: true
    /// Optional fixed tint for theme-independent hosts such as the dark album
    /// atmosphere. Empty keeps the normal themed idle/hover selector.
    property string tintOverride: ""
    /// Optional host for the shared QbzTooltip mechanism. Kept opt-in so the
    /// many icon-only call sites that intentionally have no bubble stay inert.
    property var tooltip: null
    property string tooltipKey: ""
    property string tooltipText: ""
    readonly property string resolvedTooltipKey: tooltipKey !== ""
        ? tooltipKey : "icon-" + name
    signal clicked()

    QbzTheme { id: theme }

    // BarControls.slint IconButton: hover -> text-primary, idle ->
    // text-secondary. Both are runtime-tinted, so they ARE those tokens.
    readonly property string tintStrong: "textPrimary"
    readonly property string tintWeak: "secondary"

    width: btnSize
    height: btnSize
    radius: theme.radiusSm
    opacity: btnEnabled ? 1.0 : 0.3
    color: (biArea.containsMouse && btnEnabled) ? theme.surfaceHover
        : (active && activeBackground) ? theme.surfaceElevated : "transparent"
    QbzIcon {
        name: parent.name
        width: parent.iconSize
        height: parent.iconSize
        anchors.centerIn: parent
        tintName: btn.tintOverride !== "" ? btn.tintOverride
            : !parent.btnEnabled ? "muted"
            : parent.active ? "accent"
            : biArea.containsMouse ? btn.tintStrong : btn.tintWeak
    }
    MouseArea {
        id: biArea
        anchors.fill: parent
        enabled: parent.btnEnabled
        hoverEnabled: true
        cursorShape: parent.btnEnabled ? Qt.PointingHandCursor : Qt.ArrowCursor
        onContainsMouseChanged: {
            if (btn.tooltip && btn.tooltipText !== "")
                btn.tooltip.hover(containsMouse, btn, btn.resolvedTooltipKey,
                                  btn.tooltipText)
        }
        onClicked: {
            if (!parent.btnEnabled)
                return
            if (btn.tooltip)
                btn.tooltip.hide(btn.resolvedTooltipKey)
            parent.clicked()
        }
    }
}
