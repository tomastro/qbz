// PmMenuRow — ONE row of the Playlist Manager's three menus: the toolbar's
// filter menu, its sort menu (with the #657 direction caret) and the
// move-to-folder menu. The port of `PmMenuItem`
// (PlaylistManagerView.slint:113-158).
//
// 30 px, NOT the port's 33 px context-menu convention: rule 3 says take the
// reference's number. `QbzContextMenu`'s own header states its rows are
// per-site, so there is no shared row component to reuse
// (QbzContextMenu.qml:16-18); controls/CardMenu.qml was rejected as the base —
// it is a flat entries model with a leading icon, no `selected` state, no
// trailing caret and no collapse arm.
//
// The `shown` arm is the reference's collapse (a hidden row is height 0, not
// removed) — Slint 1.16 disallows `for x: if c:`, so the move-to-folder menu
// hides the playlist's CURRENT folder that way (`:857`). Kept 1:1 rather than
// filtering the model, so the two menus stay one component.
//
// The trailing glyph renders ONLY when `selected`, in the reference: an
// unselected row has no check and no caret (`:140-150`).

import QtQuick
import com.blitzfc.qbz
import "../../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    property string label: ""
    property bool selected: false
    /// false => height 0 (the collapse arm). The row keeps its slot in the
    /// Column but takes no space and no input.
    property bool shown: true
    /// `"check"` | `"caret"` | `"none"` — what the SELECTED row trails with.
    property string trailing: "check"
    /// Only read when `trailing === "caret"`: up = ascending, down = descending.
    property bool sortAsc: true
    /// The handler MUST end with the owning menu's `close()` — Slint's
    /// PopupWindow closes on any inside click and Qt's CloseOnPressOutside
    /// does not (contract D15).
    signal activated()

    QbzTheme { id: theme }

    height: root.shown ? (root.kioskHost ? 44 : 30) : 0
    visible: root.shown
    radius: 5
    color: rowArea.containsMouse ? theme.surfaceHover : "transparent"

    Row {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        spacing: 8

        Text {
            anchors.verticalCenter: parent.verticalCenter
            width: Math.max(0, parent.width - (trailingGlyph.visible ? 13 + 8 : 0))
            text: root.label
            color: root.selected ? theme.accent : theme.textSecondary
            font.pixelSize: 13
            font.weight: root.selected ? theme.weightSemibold : theme.weightRegular
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }
        QbzIcon {
            id: trailingGlyph
            anchors.verticalCenter: parent.verticalCenter
            visible: root.selected && root.trailing !== "none"
            width: 13
            height: 13
            name: root.trailing === "caret"
                ? (root.sortAsc ? "chevron-up" : "chevron-down")
                : "check"
            tintName: "accent"
        }
    }

    MouseArea {
        id: rowArea
        anchors.fill: parent
        enabled: root.shown
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.activated()
    }
}
