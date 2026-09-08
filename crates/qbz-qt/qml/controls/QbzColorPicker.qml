// QbzColorPicker — the QML port of primitives/ColorPicker.slint: an inline,
// expandable HSV colour picker (SV square + hue strip + preview + editable
// HEX field). Used by the custom-theme editor; one instance is mounted at a
// time, behind the open swatch.
//
// Design decisions carried over from the reference, and one that is NOT:
//
//   - INLINE, never a Popup. The parent mounts exactly one picker at a time,
//     so there is no z-order or positioning fight (ColorPicker.slint:4-6).
//   - The picker OWNS its hue/sat/val state, seeded from `value` on mount and
//     on EXTERNAL change only. A fully derived model would lose the hue at
//     s == 0 or v == 0 (ColorPicker.slint:7-9). Two guards keep it: `dragging`
//     skips mid-drag feedback, and a hex compare skips the parent's own
//     round-tripped colour.
//   - NO Timer / NumberAnimation / Behavior. The reference has none, and the
//     shell's repaint-pulse rule forbids adding any: the picker repaints on
//     user input and costs nothing at rest. (The reference's stated reason is
//     a Slint compiler panic on Timer-in-`if`; ours is the pulse contract.
//     Same conclusion, different rationale — do not "restore" an animation.)
//
// HUE DOMAIN. The Slint stores hue in 0..360 (`Colors.hsv`). `Qt.hsva()` takes
// ALL FOUR arguments in 0..1, so `hue` here is NORMALISED 0..1 and every
// boundary — the SV base colour, the strip marker position, the pointer maths
// — works in that domain. Feeding 217 into Qt.hsva clamps to 1.0 and paints
// every swatch red.
//
// ENCODING. `picked(hex)` and the HEX field speak 6-digit `#rrggbb` — the
// EDIT/PERSIST encoding of the custom-theme base tokens (qbz_theme
// Rgba::to_hex). `value` comes in as a `color`, so it accepts the
// `#aarrggbb` render encoding the bridge publishes; the alpha is ignored,
// base tokens being opaque by contract.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root

    /// The current value, driven by the parent. Read on mount + external change.
    property color value: "#ff4285f4"
    /// Emitted live (drag included) with a 6-digit `#rrggbb` string. Named
    /// `picked`, not the reference's `changed`: in QML `changed` collides
    /// visually with the auto-generated `<property>Changed` handlers and reads
    /// as one at every call site. Same semantics.
    signal picked(string hex)
    /// Emitted when a colour drag ENDS — the owner persists here, so a settled
    /// drag does not wait out the write debounce.
    signal released()
    /// Emitted when the HEX field is committed (the parent parses + feeds back).
    signal hexCommitted(string hex)

    QbzTheme { id: theme }

    // --- owned HSV state -------------------------------------------------
    property real hue: 0.6028   // 0..1 (Qt.hsva domain — see the header)
    property real sat: 0.74     // 0..1
    property real val: 0.96     // 0..1
    // True only while an SV/hue drag is in flight, so the round-tripped colour
    // the parent feeds back does not fight the crosshair mid-drag.
    property bool dragging: false

    readonly property color current: Qt.hsva(root.hue, root.sat, root.val, 1)
    readonly property string currentHex: _hex(root.current)

    /// A QML `color` to 6-digit `#rrggbb`. `.r/.g/.b` are 0..1 floats that came
    /// from 8-bit channels, so this round-trips exactly.
    function _hex(c) {
        function b(x) {
            var v = Math.max(0, Math.min(255, Math.round(x * 255)))
            return (v < 16 ? "0" : "") + v.toString(16)
        }
        return "#" + b(c.r) + b(c.g) + b(c.b)
    }

    /// Seed the owned HSV from `value` (mount + external change). Mirrors
    /// ColorPicker.slint:51-69, with hue normalised to 0..1.
    function decompose() {
        var r = root.value.r, g = root.value.g, b = root.value.b
        var mx = Math.max(r, Math.max(g, b))
        var mn = Math.min(r, Math.min(g, b))
        var d = mx - mn
        root.val = mx
        root.sat = mx <= 0 ? 0 : d / mx
        if (d <= 0)
            root.hue = 0                                   // gray: hue undefined -> 0
        else if (mx === r)
            root.hue = ((((g - b) / d) % 6) + 6) % 6 / 6   // JS % keeps the sign
        else if (mx === g)
            root.hue = (((b - r) / d) + 2) / 6
        else
            root.hue = (((r - g) / d) + 4) / 6
    }

    Component.onCompleted: root.decompose()

    // Reseed on EXTERNAL changes only. The comparison is on the 8-bit HEX and
    // not on the colours themselves: `value` is our own emitted hex parsed
    // back, so it is byte-quantised, while `current` is float HSV output —
    // comparing those directly would report a difference for every edit and
    // re-decompose (resetting the hue whenever s or v hits 0), which is the
    // exact failure ColorPicker.slint:74-79 documents.
    onValueChanged: {
        if (!root.dragging && root._hex(root.value) !== root.currentHex)
            root.decompose()
    }

    // --- layout ----------------------------------------------------------
    color: theme.surfaceCard
    radius: theme.radiusSm
    border.width: 1
    border.color: theme.borderSubtle
    implicitHeight: col.implicitHeight + 24
    height: implicitHeight

    Column {
        id: col
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 12
        spacing: 10

        // Saturation / Value square.
        Rectangle {
            id: sv
            width: parent.width
            height: 140
            radius: theme.radiusSm
            clip: true
            // Base: the pure hue.
            color: Qt.hsva(root.hue, 1, 1, 1)
            // Both overlays carry the parent's radius: Qt's `clip` scissors to
            // the bounding RECT, unlike Slint's, which honours border-radius —
            // so a square-cornered child paints over the rounded corners the
            // reference renders (ColorPicker.slint:118-124).
            //
            // White -> transparent (left to right) = saturation.
            Rectangle {
                anchors.fill: parent
                radius: sv.radius
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0.0; color: "#ffffffff" }
                    GradientStop { position: 1.0; color: "#00ffffff" }
                }
            }
            // Transparent -> black (top to bottom) = value.
            Rectangle {
                anchors.fill: parent
                radius: sv.radius
                gradient: Gradient {
                    GradientStop { position: 0.0; color: "#00000000" }
                    GradientStop { position: 1.0; color: "#ff000000" }
                }
            }
            // Crosshair: x = saturation, y = (1 - value).
            Rectangle {
                width: 14
                height: 14
                radius: 7
                x: Math.max(0, Math.min(sv.width - width, root.sat * sv.width))
                y: Math.max(0, Math.min(sv.height - height, (1 - root.val) * sv.height))
                color: "transparent"
                border.width: 2
                border.color: "#ffffffff"
                Rectangle {
                    x: 2
                    y: 2
                    width: parent.width - 4
                    height: parent.height - 4
                    radius: width / 2
                    color: "transparent"
                    border.width: 1
                    border.color: "#80000000"
                }
            }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.CrossCursor
                onPressed: function (m) { root.dragging = true; root._pickSv(m.x, m.y) }
                onPositionChanged: function (m) { if (root.dragging) root._pickSv(m.x, m.y) }
                onReleased: { root.dragging = false; root.released() }
                onCanceled: { root.dragging = false; root.released() }
            }
        }

        // Hue strip.
        Rectangle {
            id: hueStrip
            width: parent.width
            height: 14
            radius: 7
            clip: true
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0000; color: "#ffff0000" }
                GradientStop { position: 0.1666; color: "#ffffff00" }
                GradientStop { position: 0.3333; color: "#ff00ff00" }
                GradientStop { position: 0.5000; color: "#ff00ffff" }
                GradientStop { position: 0.6666; color: "#ff0000ff" }
                GradientStop { position: 0.8333; color: "#ffff00ff" }
                GradientStop { position: 1.0000; color: "#ffff0000" }
            }
            // Marker.
            Rectangle {
                width: 4
                height: parent.height + 4
                y: -2
                x: Math.max(0, Math.min(hueStrip.width - width, root.hue * hueStrip.width))
                radius: 2
                color: "#ffffffff"
                border.width: 1
                border.color: "#80000000"
            }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onPressed: function (m) { root.dragging = true; root._pickHue(m.x) }
                onPositionChanged: function (m) { if (root.dragging) root._pickHue(m.x) }
                onReleased: { root.dragging = false; root.released() }
                onCanceled: { root.dragging = false; root.released() }
            }
        }

        // Preview swatch + editable HEX field.
        Row {
            width: parent.width
            spacing: 10
            Rectangle {
                width: 40
                height: 34
                radius: theme.radiusSm
                border.width: 1
                border.color: theme.borderSubtle
                color: root.current
            }
            QbzLineEdit {
                // The plain arm is a fixed 240 wide; the picker spans the
                // settings column, so the field takes what the swatch leaves.
                width: Math.max(120, parent.width - 50)
                anchors.verticalCenter: parent.verticalCenter
                text: root.currentHex
                placeholder: "#rrggbb"
                // Enter commits. No hotkey-guard wiring is needed: the shell
                // derives "a text input has focus" structurally from the
                // active focus item (AppShell.qml, the `afi instanceof
                // TextInput` gate inside Keys.onPressed), so the Slint
                // UiFocusState dance (ColorPicker.slint:229-234, a workaround
                // for #619) has no counterpart here.
                onAccepted: function (v) { root.hexCommitted(v) }
                // Commit only well-formed #rrggbb (7 chars); a shorter string
                // is an in-progress edit and is ignored until it is valid.
                onEdited: function (v) { if (v.length === 7) root.hexCommitted(v) }
            }
        }
    }

    // Both pointer handlers build the hex from the values they just assigned
    // rather than reading `currentHex`, so the emission cannot depend on
    // binding evaluation order.
    function _pickSv(mx, my) {
        root.sat = Math.max(0, Math.min(1, mx / sv.width))
        root.val = Math.max(0, Math.min(1, 1 - my / sv.height))
        root.picked(root._hex(Qt.hsva(root.hue, root.sat, root.val, 1)))
    }
    function _pickHue(mx) {
        root.hue = Math.max(0, Math.min(1, mx / hueStrip.width))
        root.picked(root._hex(Qt.hsva(root.hue, root.sat, root.val, 1)))
    }
}
