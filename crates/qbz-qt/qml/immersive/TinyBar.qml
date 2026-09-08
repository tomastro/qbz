// TinyBar — the immersive seek micro-slider (ImmersiveView.slint:110-145),
// immersive-local per divergence D5: NOT QbzSlider (integer-step; seek needs
// a fraction + the npSeekableMax clamp, which the HOST applies).
//
// Fraction in/out, fill width = parent.width * clamp(value, 0, 1); the
// changed() callback fires on press AND pressed-move with
// clamp(mouseX / width, 0, 1).

import QtQuick

Item {
    id: root

    property real value: 0.0
    property color fill: "#ffffff"
    signal changed(real v)

    function clamp01(v) {
        return Math.max(0.0, Math.min(1.0, v))
    }

    height: 10

    // Track: 2px #ffffff2e (Slint RRGGBBAA -> Qt #AARRGGBB) at y=4.
    Rectangle {
        x: 0
        y: 4
        width: root.width
        height: 2
        radius: 1
        color: "#2effffff"
    }
    // Fill.
    Rectangle {
        x: 0
        y: 4
        width: root.width * root.clamp01(root.value)
        height: 2
        radius: 1
        color: root.fill
    }
    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onPressed: function (mouse) { root.changed(root.clamp01(mouse.x / width)) }
        onPositionChanged: function (mouse) {
            if (pressed)
                root.changed(root.clamp01(mouse.x / width))
        }
    }
}
