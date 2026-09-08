// QbzSplitButton — a toolbar button that LOOKS like a select: the body runs
// the primary action, the chevron cell opens a menu of alternatives.
//
//   +--------------+-+
//   | Play all     |v|
//   +--------------+-+
//
// Sized and painted like `QbzSelect { sm: true }` so it sits flush beside one
// in a section header (30 tall, r6, elevated fill, no outline, 12px label,
// 14px chevron): the album-section "Play all / Play random / Play selected"
// control on the artist page is the first host. The menu is the shared
// controls/QbzContextMenu opened below-right of the whole control, the
// QbzSelect / sort-menu convention.
//
// `menuItems` is `[{ label, action, icon? }]`; a pick closes the menu and
// emits `picked(action)`. The body emits `clicked()` and never opens the
// menu — the two halves are separate hit zones with their own hover fill,
// and the 1px hairline between them is the split.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root

    property string label: ""
    property var menuItems: []
    property int menuWidth: 180
    property bool btnEnabled: true
    signal clicked()
    signal picked(string action)

    QbzTheme { id: theme }

    readonly property int chevronWidth: 24

    height: 30
    radius: 6
    implicitWidth: bodyLabel.implicitWidth + 24 + root.chevronWidth
    width: implicitWidth
    // Same resting fill as QbzSelect sm (translucent under the dynamic
    // background); hover is painted per zone below.
    color: theme.ambientOn ? theme.surfaceElevatedA50 : theme.surfaceElevated
    opacity: root.btnEnabled ? 1.0 : 0.4

    // --- Body: the primary action ------------------------------------------
    Rectangle {
        id: body
        x: 0
        y: 0
        width: parent.width - root.chevronWidth
        height: parent.height
        radius: 6
        color: bodyArea.containsMouse && root.btnEnabled ? theme.surfaceHover : "transparent"
        // Square the inner corners so the hover fill meets the hairline.
        Rectangle {
            x: parent.width - 6
            y: 0
            width: 6
            height: parent.height
            color: parent.color
        }
        Text {
            id: bodyLabel
            anchors.centerIn: parent
            text: root.label
            color: theme.textPrimary
            font.pixelSize: 12
            verticalAlignment: Text.AlignVCenter
        }
        MouseArea {
            id: bodyArea
            anchors.fill: parent
            enabled: root.btnEnabled
            hoverEnabled: true
            cursorShape: root.btnEnabled ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: root.clicked()
        }
    }

    // --- The split ----------------------------------------------------------
    Rectangle {
        x: parent.width - root.chevronWidth
        y: 7
        width: 1
        height: parent.height - 14
        color: theme.borderSubtle
    }

    // --- Chevron: the menu --------------------------------------------------
    Rectangle {
        id: chevron
        x: parent.width - root.chevronWidth
        y: 0
        width: root.chevronWidth
        height: parent.height
        radius: 6
        color: chevronArea.containsMouse && root.btnEnabled ? theme.surfaceHover : "transparent"
        Rectangle {
            x: 0
            y: 0
            width: 6
            height: parent.height
            color: parent.color
        }
        QbzIcon {
            name: "chevron-down"
            width: 14
            height: 14
            anchors.centerIn: parent
            tintName: chevronArea.containsMouse ? "textPrimary" : "secondary"
        }
        MouseArea {
            id: chevronArea
            anchors.fill: parent
            enabled: root.btnEnabled
            hoverEnabled: true
            cursorShape: root.btnEnabled ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: menu.openBelowRight(root)
        }
    }

    QbzContextMenu {
        id: menu
        menuWidth: root.menuWidth
        Repeater {
            model: root.menuItems
            delegate: Rectangle {
                required property var modelData
                width: parent ? parent.width : 0
                height: 33
                radius: 5
                color: itemArea.containsMouse ? theme.surfaceHover : "transparent"
                Row {
                    anchors.fill: parent
                    anchors.leftMargin: 8
                    spacing: 8
                    QbzIcon {
                        visible: (modelData.icon || "") !== ""
                        name: modelData.icon || ""
                        width: visible ? 15 : 0
                        height: 15
                        anchors.verticalCenter: parent.verticalCenter
                        tintName: "secondary"
                    }
                    Text {
                        height: parent.height
                        width: parent.width - 23
                        text: modelData.label
                        color: theme.textSecondary
                        font.pixelSize: 13
                        verticalAlignment: Text.AlignVCenter
                        elide: Text.ElideRight
                    }
                }
                MouseArea {
                    id: itemArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        menu.close()
                        root.picked(modelData.action)
                    }
                }
            }
        }
    }
}
