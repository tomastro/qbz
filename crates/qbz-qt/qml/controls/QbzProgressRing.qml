// QbzProgressRing — a determinate ring (0..1) for a compact "something is
// downloading" glyph: the sidebar's mini rail and the header's compact nav,
// where a text row has no room.
//
// It repaints ONLY when `value` changes (one Canvas.requestPaint per publish),
// never on a timer — the repaint-pulse rule (CLAUDE.md, qbz-qt) forbids a
// continuous animation here, and a download publishes at most once per track.

import QtQuick
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    /// 0.0 .. 1.0
    property real value: 0
    /// Ring diameter in px.
    property int size: 18
    /// Stroke width in px.
    property real stroke: 2.5
    property color trackColor: theme.borderSubtle
    property color barColor: theme.accent

    width: size
    height: size

    onValueChanged: ring.requestPaint()
    onBarColorChanged: ring.requestPaint()
    onTrackColorChanged: ring.requestPaint()

    Canvas {
        id: ring
        anchors.fill: parent
        antialiasing: true
        onPaint: {
            var ctx = getContext("2d")
            ctx.reset()
            var cx = width / 2, cy = height / 2
            var r = Math.max(1, Math.min(cx, cy) - root.stroke / 2)
            ctx.lineWidth = root.stroke
            ctx.lineCap = "round"
            ctx.strokeStyle = root.trackColor
            ctx.beginPath()
            ctx.arc(cx, cy, r, 0, Math.PI * 2)
            ctx.stroke()
            var v = Math.max(0, Math.min(1, root.value))
            if (v > 0) {
                ctx.strokeStyle = root.barColor
                ctx.beginPath()
                ctx.arc(cx, cy, r, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * v)
                ctx.stroke()
            }
        }
    }
}
