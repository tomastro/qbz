// CardOverlayButton — the shared card hover-overlay button (the Slint
// per-card OverlayButton copies, homologated in the POC at
// LibraryView.LibOverlayBtn and promoted to shared in phase 21):
// primary = 44px white disc (black icon), ghost = 36px translucent disc
// (1.5px border); `active` tints the icon accent (follow/heart state).

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    property string name: ""
    property bool primary: false
    property bool active: false
    // Forwards the MouseArea event. Three cards write
    // `onClicked: function (mouse) { menu.openAtCursor(btn, mouse.x, mouse.y) }`
    // against this signal; while it took no argument, `mouse` was undefined and
    // those context menus threw a TypeError instead of opening.
    signal clicked(var mouse)

    width: primary ? 44 : 36
    height: primary ? 44 : 36
    radius: width / 2
    color: primary ? (obArea.containsMouse ? "#d6ffffff" : "#ffffff")
         : (obArea.containsMouse ? "#3dffffff" : "#24ffffff")
    border.width: primary ? 0 : 1.5
    border.color: "#ccffffff"
    QbzIcon {
        name: parent.name
        width: primary ? 18 : 16
        height: primary ? 18 : 16
        anchors.centerIn: parent
        // Both non-accent arms are theme-INDEPENDENT by design: this button
        // only ever sits on artwork. `primary` is a solid #ffffff disc (black
        // glyph), the ghost arm is #24ffffff over the artwork itself (white
        // glyph). "white" is the explicit spelling of what "primary" baked.
        tintName: parent.active ? "accent" : (parent.primary ? "black" : "white")
    }
    property alias hovered: obArea.containsMouse
    MouseArea {
        id: obArea
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: function (m) { parent.clicked(m) }
    }
}
