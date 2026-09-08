// Loading spinner — replica of primitives/LoadingSpinner.slint: the
// Lucide loader-circle arc in accent, rotating 360deg/1000ms linear.

import QtQuick

Item {
    id: root
    property int size: 32
    property bool spinning: true

    width: size
    height: size

    QbzIcon {
        name: "loader-circle"
        width: root.size
        height: root.size
        tintName: "accent"
        RotationAnimation on rotation {
            running: root.spinning && root.visible
            from: 0
            to: 360
            duration: 1000
            loops: Animation.Infinite
        }
    }
}
