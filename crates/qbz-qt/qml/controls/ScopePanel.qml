import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    // ScopeTraceItem is a native scene-graph line strip. Software/Null can
    // paint the guide rectangles below but not the trace; hide the complete
    // instrument instead of leaving axes that look like a dead visualizer.
    visible: QbzShell.gpuTier

    QbzTheme { id: theme }

    // 0 = goniometer, 1 = pitch-locked oscilloscope.
    property int scopeMode: 0
    property color traceColor: theme.accent
    // Immersive scopes choose the brightest colour extracted from the
    // current cover. Compact Large-NPB scopes leave this false and receive
    // the same reactive primary colour as its other three visualizers.
    property bool albumContrast: false
    property bool compact: false
    readonly property color albumPrimary: QbzShell.ambientPrimary
    readonly property color albumSecondary: QbzShell.ambientSecondary
    readonly property color albumAccent: QbzShell.ambientAccent

    function relativeLuminance(c) {
        const r = c.r <= 0.03928 ? c.r / 12.92 : Math.pow((c.r + 0.055) / 1.055, 2.4)
        const g = c.g <= 0.03928 ? c.g / 12.92 : Math.pow((c.g + 0.055) / 1.055, 2.4)
        const b = c.b <= 0.03928 ? c.b / 12.92 : Math.pow((c.b + 0.055) / 1.055, 2.4)
        return 0.2126 * r + 0.7152 * g + 0.0722 * b
    }
    function brightestAlbumColor() {
        const candidates = [root.albumPrimary, root.albumSecondary,
                            root.albumAccent]
        var best = candidates[0]
        var bestLuminance = relativeLuminance(best)
        for (var i = 1; i < candidates.length; ++i) {
            const luminance = relativeLuminance(candidates[i])
            if (luminance > bestLuminance) {
                best = candidates[i]
                bestLuminance = luminance
            }
        }
        return best
    }
    readonly property color effectiveTraceColor:
        root.albumContrast ? root.brightestAlbumColor() : root.traceColor
    readonly property color guideColor: Qt.rgba(
        root.effectiveTraceColor.r, root.effectiveTraceColor.g,
        root.effectiveTraceColor.b, root.scopeMode === 0 ? 0.19 : 0.12)

    // Instrument guides stay deliberately quieter than the trace. They are
    // static scene-graph nodes; only the native trace item changes per frame.
    Rectangle {
        anchors.horizontalCenter: parent.horizontalCenter
        y: root.compact ? 4 : parent.height * 0.08
        width: 1
        height: root.compact ? parent.height - 8 : parent.height * 0.84
        color: root.guideColor
    }
    Rectangle {
        x: root.compact ? 4 : parent.width * 0.08
        anchors.verticalCenter: parent.verticalCenter
        width: root.compact ? parent.width - 8 : parent.width * 0.84
        height: 1
        color: root.guideColor
    }

    Repeater {
        model: root.scopeMode === 1 && !root.compact ? 3 : 0
        delegate: Rectangle {
            required property int index
            x: parent.width * (index + 1) / 4
            y: parent.height * 0.14
            width: 1
            height: parent.height * 0.72
            color: Qt.rgba(root.effectiveTraceColor.r, root.effectiveTraceColor.g,
                           root.effectiveTraceColor.b, 0.07)
        }
    }

    Rectangle {
        visible: root.scopeMode === 0
        anchors.centerIn: parent
        width: parent.width * (root.compact ? 0.76 : 0.84)
        height: width
        color: "transparent"
        border.width: 1
        border.color: Qt.rgba(root.effectiveTraceColor.r, root.effectiveTraceColor.g,
                              root.effectiveTraceColor.b, 0.18)
        radius: width / 2
    }
    Rectangle {
        visible: root.scopeMode === 0 && !root.compact
        anchors.centerIn: parent
        width: parent.width * 0.46
        height: width
        color: "transparent"
        border.width: 1
        border.color: Qt.rgba(root.effectiveTraceColor.r, root.effectiveTraceColor.g,
                              root.effectiveTraceColor.b, 0.09)
        radius: width / 2
    }
    Repeater {
        model: root.scopeMode === 0 && !root.compact ? 2 : 0
        delegate: Rectangle {
            required property int index
            anchors.centerIn: parent
            width: parent.width * 0.76
            height: 1
            rotation: index === 0 ? 45 : -45
            color: Qt.rgba(root.effectiveTraceColor.r, root.effectiveTraceColor.g,
                           root.effectiveTraceColor.b, 0.08)
        }
    }

    ScopeTraceItem {
        anchors.fill: parent
        anchors.margins: root.compact ? 3 : 8
        mode: root.scopeMode
        points: root.scopeMode === 0 ? QbzViz.goniometer : QbzViz.oscilloscope
        traceColor: root.effectiveTraceColor
        lineWidth: root.compact ? 1.35 : 2.2
        trailDepth: root.scopeMode === 0 ? (root.compact ? 3 : 5) : (root.compact ? 2 : 4)
        fillOpacity: root.compact ? 0.08 : 0.16
    }

    Item {
        visible: root.scopeMode === 0 && !root.compact
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: 12
        width: 132
        height: 18

        Rectangle {
            anchors.left: parent.left
            anchors.right: value.left
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            height: 2
            radius: 1
            color: Qt.rgba(root.effectiveTraceColor.r, root.effectiveTraceColor.g,
                           root.effectiveTraceColor.b, 0.22)
            Rectangle {
                width: 4
                height: 10
                radius: 2
                anchors.verticalCenter: parent.verticalCenter
                x: (parent.width - width) * Math.max(0, Math.min(1,
                    (QbzViz.stereoCorrelation + 1) * 0.5))
                color: root.effectiveTraceColor
            }
        }
        Text {
            id: value
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            text: QbzViz.stereoCorrelation.toFixed(2)
            color: Qt.rgba(root.effectiveTraceColor.r, root.effectiveTraceColor.g,
                           root.effectiveTraceColor.b, 0.72)
            font.pixelSize: 11
        }
    }
}
