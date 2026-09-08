// QbzLoadingDots — the three-dot "still resolving" indicator
// (primitives/LoadingDots.slint). Mounted in the MyQBZ detail row's 160px
// quality cell while the badge has not resolved yet.
//
// A CHEAPER APPROXIMATION, on purpose. Slint runs a per-dot triangle wave off
// `animation-tick()` — phase offsets 0 / 0.18 / 0.36 over 900ms,
// `opacity: 0.25 + 0.55 * tri` (so 0.25 … 0.80, LoadingDots.slint:15-20).
// That is a per-DELEGATE animation, and in a windowed list of hundreds of rows
// it is exactly what the port's perf rule forbids. So the phase comes IN as an
// int 0..2 and the HOST owns one `Timer { interval: 300; repeat: true }` for
// the whole view, gated `<any row resolving> && visible && windowShowing`
// (PlaylistView.qml:156-159 + :173-179 — freeze on not-visible or minimized,
// NEVER on lost focus: a tiling desktop keeps windows visible and unfocused).
//
// The dot row CENTRES itself in whatever host it is given, because the .slint
// component inherits Rectangle and therefore fills its cell and centres its
// layout (:26-35) — it is not left-aligned at x=0.
//
// Slint's optional `label` arm (:36-41) is unused by MyQBZ and not ported.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    /// 0..2 — which dot is lit. Cycled by ONE Timer in the host view.
    property int phase: 0

    QbzTheme { id: theme }

    implicitWidth: 3 * 8 + 2 * 8
    implicitHeight: 8

    Row {
        anchors.centerIn: parent
        spacing: 8
        Repeater {
            // A STATIC model (3), so the delegates are built once and only
            // their opacity binding re-evaluates.
            model: 3
            delegate: Rectangle {
                required property int index
                width: 8
                height: 8
                radius: 4
                color: theme.textSecondary
                opacity: root.phase === index ? 0.8 : 0.25
            }
        }
    }
}
