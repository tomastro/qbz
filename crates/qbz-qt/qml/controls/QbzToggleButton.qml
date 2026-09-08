// QbzToggleButton — 1:1 port of primitives/ToggleButton.slint: one half of a
// segmented toggle (grid/list) or a standalone toolbar toggle (group-by /
// Hi-Res). Shared by every album-toolbar surface in the Slint (Discover
// Browse, Label Releases, Favorites), so it is shared here too.
//
// Numbers, read off ToggleButton.slint:
//   :22 34px square (30px with `sm`)   :35 glyph 17px (15px with `sm`)
//   :25 active   -> surface-hover fill, text-primary glyph
//   :30 hover    -> surface-elevated fill
//   idle         -> transparent fill,   text-muted glyph
//
// NOT QbzToggle.qml — that one is the on/off SWITCH (checked/toggled(bool)).
//
// LIGHT-THEME CORRECTNESS: the active fill is `surface-hover`, a THEME
// surface, so the active glyph is `text-primary` — a runtime-tinted token
// (src/icon_tint_qt.rs), not the fixed #ffffff the "primary" bake is. The
// old lightness ternary that picked between two fixed bakes is gone; the
// tinter reproduces the live token exactly.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root

    property string name: ""
    property bool active: false
    /// Bootstrap-style small variant (ToggleButton `sm`): 30px, not 34px.
    property bool sm: false
    property bool btnEnabled: true
    signal clicked()

    QbzTheme { id: theme }

    readonly property string tintStrong: "textPrimary"

    width: sm ? 30 : 34
    height: sm ? 30 : 34
    radius: 6
    opacity: btnEnabled ? 1.0 : 0.35
    // Glass under the app-wide dynamic background: the ACTIVE fill goes
    // translucent so the moving field shows through (ToggleButton.slint:25-29).
    // Only the active arm changes — surface-hover is already translucent and
    // the resting arm is transparent, so neither has an alpha to set.
    color: root.active
        ? (theme.ambientOn ? theme.surfaceElevatedA50 : theme.surfaceHover)
        : (tbArea.containsMouse && root.btnEnabled ? theme.surfaceElevated : "transparent")

    QbzIcon {
        name: root.name
        width: root.sm ? 15 : 17
        height: root.sm ? 15 : 17
        anchors.centerIn: parent
        tintName: root.active ? root.tintStrong : "muted"
    }

    MouseArea {
        id: tbArea
        anchors.fill: parent
        enabled: root.btnEnabled
        hoverEnabled: true
        cursorShape: root.btnEnabled ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: root.clicked()
    }
}
