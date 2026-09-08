// A checkbox for the rip wizard's track list.
//
// Its own file rather than a copy per call site, and NOT the app's other
// tick-shaped controls: the multi-select bar's is a row-hover affordance and
// the settings toggles are switches. This one is a plain, always-visible
// checkbox with a THIRD state — partially selected — which the select-all row
// needs and neither of the others has.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root
    property bool checked: false
    /// Some but not all — drawn as a dash, the convention every file manager
    /// uses, because a half-ticked box that looks empty invites a click that
    /// does the opposite of what the user expects.
    property bool partial: false
    signal toggled()

    QbzTheme { id: theme }

    width: 18
    height: 18
    radius: 4
    color: (root.checked || root.partial) ? theme.accent : "transparent"
    border.width: 1
    border.color: (root.checked || root.partial)
        ? theme.accent
        : (area.containsMouse ? theme.textMuted : theme.borderStrong)

    QbzIcon {
        anchors.centerIn: parent
        visible: root.checked && !root.partial
        name: "check"
        width: 12
        height: 12
        // The glyph sits on `accent`, so it takes the measured on-accent
        // selector rather than a hardcoded white — 16 of the 35 shipped
        // palettes make white unreadable there (QbzCircleAction's rationale).
        tintName: theme.accentGlyphTint
    }
    Rectangle {
        anchors.centerIn: parent
        visible: root.partial
        width: 9
        height: 2
        radius: 1
        // The COLOUR twin of the tint above, so the dash and a check on
        // the same palette can never be different colours.
        color: theme.accentGlyphColor
    }

    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.toggled()
    }
}
