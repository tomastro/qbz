// ImmersiveHeader — the immersive chrome band (ImmersiveView.slint:470-1184,
// 1618-1685), mounted by ImmersiveView at x=24 / y=16 / height=36 /
// width=parent-48 (§5.2 of the 2026-08-02 immersive-port contract).
//
// Three zones:
//   LEFT    — a 122x36 previous | picker | next capsule + a wide three-column
//             popup (two FOCUS columns, one SPLIT). Every row
//             fires QbzImmersive.setView(...); the data-panel rows also fire
//             their §5.5 entry load (Queue / Track Info / Suggestions —
//             Lyrics needs nothing: Qt fetches lyrics automatically per
//             track, §5.5).
//   CENTER  — the QbzLineEdit-based search field, absolutely centered,
//             min(band.width, 420) wide. The cortinilla itself ships in B5;
//             the Rust search invokables are the inert B1 surface until then
//             (that is CORRECT — do not implement them here).
//   RIGHT   — the window-controls capsule: a 44px circle (sliders glyph) that
//             expands LEFTWARD to 112px on hover (150ms ease-in-out), revealing
//             the fullscreen toggle (the FIRST FullScreen write in the Qt
//             frontend, §5.2) and the X exit.
//
// The whole band fades with the auto-hide chrome (ImmersiveView drives the
// opacity); nothing here knows about the hide timer.

import QtQuick
import QtQuick.Controls
import QtQuick.Window
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    id: header

    // §5.4 text-input gate: the view root reads this to inert the seek
    // arrows while the search field has focus (D16 — the QbzLineEdit
    // `fieldActive` seam).
    readonly property bool searchActive: searchField.fieldActive

    /// macOS with the NATIVE traffic lights floating over the client area —
    /// the only configuration where the band's left edge is already occupied.
    /// Same predicate the view's old `chromeLeftInset` used.
    readonly property bool trailingTrigger:
        QbzShell.isMacos && !QbzShell.systemTitleBar

    // Persistent custom-chrome drag strip. Tauri kept a top-level
    // data-tauri-drag-region over Immersive, but the Qt port never brought
    // that surface across; the full-coverage click blocker therefore left no
    // route to start a system move. Declared FIRST so every real control below
    // wins hit-testing, matching HeaderBar.qml's proven drag pattern.
    MouseArea {
        anchors.fill: parent
        // Immersive must remain draggable whenever native chrome is absent,
        // including the explicit hide-titlebar preference: there is no second
        // title strip behind this overlay to rescue that configuration.
        enabled: !QbzShell.systemTitleBar
            && header.Window.window !== null
            && header.Window.window.visibility !== Window.FullScreen
        acceptedButtons: Qt.LeftButton
        property bool dragStarted: false
        onPressed: dragStarted = false
        onReleased: dragStarted = false
        onPositionChanged: {
            if (pressed && !dragStarted && header.Window.window) {
                dragStarted = true
                header.Window.window.startSystemMove()
            }
        }
        onDoubleClicked: {
            var w = header.Window.window
            if (w)
                w.visibility = w.visibility === Window.Maximized
                    ? Window.Windowed : Window.Maximized
        }
    }


    // --- LEFT: previous | view menu | next -------------------------------
    Rectangle {
        id: viewTrigger
        // MACOS ONLY: over on the right, beside the capsule. Everywhere else it
        // stays at the band's left edge, where it has always been.
        //
        // The move exists for ONE reason — on macOS the band's left edge is the
        // native traffic lights' corner (x ~ 7-85, this pill's exact spot), so
        // something had to give. Nothing else has traffic lights, so nothing
        // else has the problem, and moving the control there was a change with
        // no cause. (It shipped unconditional first; this is that fix.)
        //
        // `x` rather than swapped anchors: toggling between `anchors.left` and
        // `anchors.right` means clearing one with `undefined` on every flip,
        // and the position is one expression either way.
        //
        // The trailing position RESERVES THE CAPSULE'S EXPANDED WIDTH (112)
        // plus a 12px gap instead of tracking its live left edge. The capsule
        // grows leftward on hover, so tracking it would slide this pill
        // sideways every time the pointer crossed the other one — motion with
        // no meaning. Reserving the maximum keeps both still.
        x: header.trailingTrigger ? header.width - 112 - 12 - width : 0
        anchors.verticalCenter: parent.verticalCenter
        width: 122
        height: 36
        // Half the height — same silhouette as the capsule it now sits beside.
        radius: height / 2
        // Slint glass #00000080 / border #ffffff2e (RRGGBBAA), converted.
        color: "#80000000"
        border.width: 1
        border.color: "#2effffff"
        clip: true

        Rectangle {
            width: 38
            height: parent.height
            color: previousArea.containsMouse ? "#18ffffff" : "transparent"
            QbzIcon {
                name: "chevron-left"
                width: 15
                height: 15
                anchors.centerIn: parent
                tintName: "white"
            }
            MouseArea {
                id: previousArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: viewMenu.cycle(-1)
            }
        }

        Rectangle {
            x: 38
            width: 46
            height: parent.height
            color: menuArea.containsMouse ? "#18ffffff" : "transparent"
            QbzIcon {
                name: "layout-grid"
                width: 16
                height: 16
                anchors.centerIn: parent
                tintName: "white"
            }
            MouseArea {
                id: menuArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: viewMenu.openBelowLeft(viewTrigger)
            }
        }

        Rectangle {
            x: 84
            width: 38
            height: parent.height
            color: nextArea.containsMouse ? "#18ffffff" : "transparent"
            QbzIcon {
                name: "chevron-right"
                width: 15
                height: 15
                anchors.centerIn: parent
                tintName: "white"
            }
            MouseArea {
                id: nextArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: viewMenu.cycle(1)
            }
        }

        Rectangle { x: 38; width: 1; height: parent.height; color: "#24ffffff" }
        Rectangle { x: 83; width: 1; height: parent.height; color: "#24ffffff" }
    }

    QbzContextMenu {
        id: viewMenu
        // The old single column could run past a short display. The owner
        // explicitly chose a wider three-column picker: two balanced FOCUS
        // columns and one SPLIT column.
        menuWidth: 660
        contentSpacing: 0
        // §5.2 popup chrome: #0b0b0ef7, radius 16 (Slint RRGGBBAA converted).
        background: Rectangle {
            color: "#f70b0b0e"
            radius: 16
            border.width: 1
            border.color: "#2effffff"
        }

        // One row model entry: vm 0 = FOCUS (mode m), vm 1 = SPLIT (panel sp).
        // Block A1 adds `sc`: >= 0 = a SHADER SCENE row (the Slint shader
        // rows at ImmersiveView.slint:654-711), -1/absent = a panel row.
        // `entry` names the §5.5 entry load fired alongside setView:
        //   "queue"       -> QbzQueue.queuePanelOpened()
        //   "trackinfo"   -> QbzAlbum.openTrackInfo(QbzPlayer.npTrackId)
        //   "suggestions" -> QbzSuggestions.load(QbzPlayer.npTrackId) (B4)
        //   ""            -> none (Lyrics rows: Qt fetches lyrics
        //                    automatically per track, §5.5).
        readonly property var focusRows: {
            var rows = [
                { "vm": 0, "m": 0, "sp": -1, "entry": "",
                  "label": QbzSession.tr("Album Reactive", QbzSession.trRev) },
                { "vm": 0, "m": 1, "sp": -1, "entry": "",
                  "label": QbzSession.tr("Static", QbzSession.trRev) },
                { "vm": 0, "m": 2, "sp": -1, "entry": "",
                  "label": QbzSession.tr("Coverflow", QbzSession.trRev) },
                { "vm": 0, "m": 4, "sp": -1, "entry": "",
                  "label": QbzSession.tr("Lyrics", QbzSession.trRev) },
                { "vm": 0, "m": 5, "sp": -1, "entry": "queue",
                  "label": QbzSession.tr("Queue", QbzSession.trRev) },
                { "vm": 0, "m": 3, "sp": -1, "entry": "",
                  "label": QbzSession.tr("Spectrum", QbzSession.trRev) },
                { "vm": 0, "m": 6, "sp": -1, "entry": "",
                  "label": QbzSession.tr("Wave Bed", QbzSession.trRev) },
                { "vm": 0, "m": 9, "sp": -1, "entry": "",
                  "label": QbzSession.tr("Reactive Rings", QbzSession.trRev) },
            ]
            if (QbzShell.gpuTier) {
                rows.push({ "vm": 0, "m": 7, "sp": -1, "entry": "",
                            "label": QbzSession.tr("Goniometer", QbzSession.trRev) })
                rows.push({ "vm": 0, "m": 8, "sp": -1, "entry": "",
                            "label": QbzSession.tr("Oscilloscope", QbzSession.trRev) })
            }
            if (QbzShell.shaderScenesAvailable) {
                rows.push(
                    { "vm": 0, "m": -1, "sp": -1, "entry": "", "sc": 1,
                      "label": QbzSession.tr("Plasma", QbzSession.trRev) },
                    { "vm": 0, "m": -1, "sp": -1, "entry": "", "sc": 2,
                      "label": QbzSession.tr("Tunnel", QbzSession.trRev) },
                    { "vm": 0, "m": -1, "sp": -1, "entry": "", "sc": 3,
                      "label": QbzSession.tr("Aurora", QbzSession.trRev) },
                    { "vm": 0, "m": -1, "sp": -1, "entry": "", "sc": 7,
                      "label": QbzSession.tr("Ambient", QbzSession.trRev) },
                    { "vm": 0, "m": -1, "sp": -1, "entry": "", "sc": 4,
                      "label": QbzSession.tr("Spectral Ribbon", QbzSession.trRev) },
                    { "vm": 0, "m": -1, "sp": -1, "entry": "", "sc": 5,
                      "label": QbzSession.tr("Line Bed", QbzSession.trRev) },
                    { "vm": 0, "m": -1, "sp": -1, "entry": "", "sc": 8,
                      "label": QbzSession.tr("Tunnel Flow", QbzSession.trRev) })
            }
            return rows
        }

        readonly property var splitRows: [
            { "vm": 1, "m": -1, "sp": 0, "entry": "",
              "label": QbzSession.tr("Lyrics", QbzSession.trRev) },
            { "vm": 1, "m": -1, "sp": 1, "entry": "trackinfo",
              "label": QbzSession.tr("Track Info", QbzSession.trRev) },
            { "vm": 1, "m": -1, "sp": 2, "entry": "suggestions",
              "label": QbzSession.tr("Suggestions", QbzSession.trRev) },
            { "vm": 1, "m": -1, "sp": 3, "entry": "queue",
              "label": QbzSession.tr("Queue", QbzSession.trRev) },
            { "vm": 1, "m": -1, "sp": 4, "entry": "",
              "label": QbzSession.tr("Now Playing", QbzSession.trRev) },
            { "vm": 1, "m": -1, "sp": 5, "entry": "",
              "label": QbzSession.tr("Now Playing", QbzSession.trRev)
                  + " · " + QbzSession.tr("Lyrics", QbzSession.trRev) },
        ]

        readonly property int focusCut: Math.ceil(focusRows.length / 2)

        function rowIsActive(row) {
            if (row.sc !== undefined && row.sc >= 0)
                return QbzImmersive.viewMode === 0
                    && QbzShaderScene.scene === row.sc
            if (row.vm === 0)
                return QbzImmersive.viewMode === 0
                    && QbzImmersive.mode === row.m
                    && QbzShaderScene.scene === 0
            return QbzImmersive.viewMode === 1
                && QbzImmersive.splitPanel === row.sp
                && QbzShaderScene.scene === 0
        }

        function fireRow(row) {
            if (row.sc !== undefined && row.sc >= 0) {
                // Shader scene row: FOCUS view + scene N (Slint sets
                // view-mode=0 and shader-mode=N and leaves `mode` alone;
                // setView is the ONLY mode mutator here, §3.2).
                if (row.sc > 0 && QbzImmersive.viewMode !== 0)
                    QbzImmersive.setView(0, QbzImmersive.mode, QbzImmersive.splitPanel)
                QbzShaderScene.scene = row.sc
                viewMenu.close()
                return
            }
            // Panel rows turn the scene OFF — every Slint panel row sets
            // shader-mode = 0 (:586-650) — then setView as before.
            QbzShaderScene.scene = 0
            QbzImmersive.setView(
                row.vm === 0 ? 0 : 1,
                row.vm === 0 ? row.m : QbzImmersive.mode,
                row.vm === 0 ? QbzImmersive.splitPanel : row.sp)
            if (row.entry === "queue")
                QbzQueue.queuePanelOpened()
            else if (row.entry === "trackinfo")
                QbzAlbum.openTrackInfo(QbzPlayer.npTrackId)
            else if (row.entry === "suggestions")
                QbzSuggestions.load(QbzPlayer.npTrackId)
            viewMenu.close()
        }

        // The columns are presentation only. Arrow navigation is ONE circular
        // sequence in menu order: every FOCUS panel/scene, then every SPLIT
        // panel, crossing the family boundary in both directions.
        function cycle(direction) {
            var rows = focusRows.concat(splitRows)
            if (rows.length === 0)
                return
            var current = -1
            for (var i = 0; i < rows.length; ++i) {
                if (rowIsActive(rows[i])) {
                    current = i
                    break
                }
            }
            var next = current < 0 ? (direction < 0 ? rows.length - 1 : 0)
                : (current + direction + rows.length) % rows.length
            fireRow(rows[next])
        }

        Component {
            id: menuRow
            Rectangle {
                required property var modelData
                // A1: shader rows (sc >= 0) are active when their scene is
                // on; FOCUS panel rows additionally require the scene OFF
                // (Slint: `active: is-focus && sh == 0 && md == N`).
                readonly property bool active: viewMenu.rowIsActive(modelData)
                width: parent ? parent.width : 0
                height: 33
                radius: 8
                // Active row #ffffff26 (Slint RRGGBBAA converted).
                color: active ? "#26ffffff"
                    : (rowArea.containsMouse ? "#14ffffff" : "transparent")
                Text {
                    anchors.left: parent.left
                    anchors.leftMargin: 10
                    anchors.right: dot.left
                    anchors.rightMargin: 8
                    height: parent.height
                    text: modelData.label
                    color: "#f2ffffff"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                }
                // Trailing 7px active dot (:529-537).
                Rectangle {
                    id: dot
                    visible: parent.active
                    anchors.right: parent.right
                    anchors.rightMargin: 12
                    anchors.verticalCenter: parent.verticalCenter
                    width: 7
                    height: 7
                    radius: 3.5
                    color: "#ffffff"
                }
                MouseArea {
                    id: rowArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: viewMenu.fireRow(modelData)
                }
            }
        }

        // Group headers: FOCUS spans its two balanced columns; SPLIT owns the
        // third. The row below determines the popup's height from its tallest
        // column, so the picker never needs its own scroll area.
        Row {
            width: viewMenu.availableWidth
            height: 26
            spacing: 8
            Text {
                width: (parent.width - 16) / 3 * 2 + 8
                height: parent.height
                leftPadding: 10
                verticalAlignment: Text.AlignVCenter
                text: QbzSession.tr("FOCUS", QbzSession.trRev)
                color: "#80ffffff"
                font.pixelSize: 11
                font.weight: Font.Bold
                font.letterSpacing: 1.5
            }
            Text {
                width: (parent.width - 16) / 3
                height: parent.height
                leftPadding: 10
                verticalAlignment: Text.AlignVCenter
                text: QbzSession.tr("SPLIT", QbzSession.trRev)
                color: "#80ffffff"
                font.pixelSize: 11
                font.weight: Font.Bold
                font.letterSpacing: 1.5
            }
        }

        Row {
            id: menuColumns
            width: viewMenu.availableWidth
            spacing: 8
            readonly property real columnWidth: (width - spacing * 2) / 3

            Column {
                width: menuColumns.columnWidth
                spacing: 1
                Repeater {
                    model: viewMenu.focusRows.slice(0, viewMenu.focusCut)
                    delegate: menuRow
                }
            }
            Column {
                width: menuColumns.columnWidth
                spacing: 1
                Repeater {
                    model: viewMenu.focusRows.slice(viewMenu.focusCut)
                    delegate: menuRow
                }
            }
            Column {
                width: menuColumns.columnWidth
                spacing: 1
                Repeater {
                    model: viewMenu.splitRows
                    delegate: menuRow
                }
            }
        }
    }

    // --- CENTER: the search field -----------------------------------------
    // QbzLineEdit-based (§5.2). 36px tall, min(band.width, 420) wide,
    // absolutely centered in the band. The 220ms debounce + >=2-char gate
    // live in the Rust immersive controller (§3.4, B5) — the field reports
    // every keystroke.
    QbzLineEdit {
        id: searchField
        searchMode: true
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        width: Math.min(header.width, 420)
        height: 36
        placeholder: QbzSession.tr("Search", QbzSession.trRev)
        // Two-way with the bridge (the B5 controller clears it on
        // activation); the B1 surface is inert until then — correct.
        text: QbzImmersive.searchInputText
        onEdited: function (value) { QbzImmersive.searchLive(value) }
        onAccepted: function (value) {
            QbzImmersive.searchRowActivated(QbzImmersive.immSearchSelectedIndex)
        }
        // Up/Down on the field ROOT (Qt handles them here; Slint rejects
        // them in the field because winit owns them — D15, same observable
        // behavior). Escape is NOT handled here: the inner input declines it
        // (QbzLineEdit non-expandable arm) and it propagates to the view
        // root, whose handler dismisses search first / exits otherwise
        // (§3.4).
        Keys.onPressed: function (event) {
            if (event.key === Qt.Key_Down) {
                QbzImmersive.searchMoveSelection(1)
                event.accepted = true
            } else if (event.key === Qt.Key_Up) {
                QbzImmersive.searchMoveSelection(-1)
                event.accepted = true
            }
        }
    }
    // "↵ Enter" hint, visible only while the cortinilla is open (:985-993) —
    // inert in B2 because immSearchOpen cannot flip true before B5.
    Text {
        visible: QbzImmersive.immSearchOpen
        anchors.left: searchField.right
        anchors.leftMargin: 10
        anchors.verticalCenter: searchField.verticalCenter
        text: "↵  " + QbzSession.tr("Enter", QbzSession.trRev)
        color: "#bfffffff"
        font.pixelSize: 12
    }

    // --- RIGHT: the window-controls capsule --------------------------------
    // Collapsed: 44px circle, sliders glyph. Expands LEFTWARD (150ms
    // ease-in-out, anchored right) revealing the window controls and the X
    // exit. On macOS the native traffic lights already carry
    // minimize/maximize, so the capsule keeps only fullscreen + X (112px);
    // everywhere else it also reveals minimize and maximize/restore (184px —
    // 2026-08-31 visual cleanup; the Slint had them parked, :1173-1183).
    //
    // The expansion is the Slint state machine ported 1:1
    // (ImmersiveView.slint:1057-1184): `expanded = pinned || anyHover` is a
    // BOOL and the buttons gate on that bool — NEVER on the animated width.
    // (2026-08-15 flicker root cause: gating on `capsule.width > 100`
    // mid-animation let the binding flip while the capsule was traveling,
    // and the expanding left edge could cross a resting cursor frame by
    // frame — a visible 44↔112 bounce. Clicking the circle toggles `pinned`
    // (Slint circle-ta, :1159-1162), which is what "stays 100% open" means.)
    Rectangle {
        id: capsule
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        height: 36
        // 40px lead-in + one 36px slot per button (2 on macOS, 4 elsewhere).
        readonly property int buttonSlots: QbzShell.isMacos ? 2 : 4
        width: expanded ? 40 + 36 * buttonSlots : 44
        radius: 18
        color: "#80000000"
        border.width: 1
        border.color: "#2effffff"
        clip: true
        Behavior on width {
            NumberAnimation { duration: 150; easing.type: Easing.InOutQuad }
        }

        // Click-pin state; survives the auto-hide (Slint keeps it too).
        property bool pinned: false
        // Hover over ANY part of the capsule: the background or either
        // button (their hover areas extend 9px past the glyphs).
        readonly property bool anyHover: capHover.containsMouse
            || minArea.containsMouse || maxArea.containsMouse
            || fsArea.containsMouse || xArea.containsMouse
        readonly property bool expanded: pinned || anyHover

        MouseArea {
            id: capHover
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            // Collapsed-circle click = pin open (Slint circle-ta). Clicking
            // the expanded body un-pins; hover still holds it open, so this
            // can never collapse anything out from under the cursor.
            onClicked: capsule.pinned = !capsule.pinned
        }

        // Collapsed-state glyph (fades out as the capsule opens).
        QbzIcon {
            name: "sliders-horizontal"
            width: 16
            height: 16
            anchors.centerIn: parent
            tintName: "white"
            opacity: capsule.expanded ? 0 : 1
            Behavior on opacity { NumberAnimation { duration: 100 } }
        }

        // Minimize + maximize/restore — non-macOS only (see the header note);
        // both gate on the expansion BOOL exactly like the pair below.
        readonly property bool platformButtons: !QbzShell.isMacos
        QbzIcon {
            name: "minus"
            width: 15
            height: 15
            visible: capsule.platformButtons
            x: capsule.width - 144
            anchors.verticalCenter: parent.verticalCenter
            tintName: minArea.containsMouse ? "white" : "muted"
            opacity: capsule.expanded && capsule.platformButtons ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: 100 } }
            MouseArea {
                id: minArea
                anchors.fill: parent
                anchors.margins: -9
                enabled: capsule.expanded && capsule.platformButtons
                hoverEnabled: enabled
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                    var w = header.Window.window
                    if (w)
                        w.showMinimized()
                }
            }
        }
        QbzIcon {
            name: (header.Window.window
                   && header.Window.window.visibility === Window.Maximized)
                  ? "copy" : "square"
            width: 14
            height: 14
            visible: capsule.platformButtons
            x: capsule.width - 108
            anchors.verticalCenter: parent.verticalCenter
            tintName: maxArea.containsMouse ? "white" : "muted"
            opacity: capsule.expanded && capsule.platformButtons ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: 100 } }
            MouseArea {
                id: maxArea
                anchors.fill: parent
                anchors.margins: -9
                enabled: capsule.expanded && capsule.platformButtons
                hoverEnabled: enabled
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                    var w = header.Window.window
                    if (!w)
                        return
                    w.visibility = w.visibility === Window.Maximized
                        ? Window.Windowed : Window.Maximized
                }
            }
        }

        // Fullscreen toggle (glyph swaps on state) + X exit — revealed by the
        // expansion, pinned to the capsule's right half.
        QbzIcon {
            id: fsGlyph
            name: (header.Window.window
                   && header.Window.window.visibility === Window.FullScreen)
                  ? "minimize-2" : "maximize-2"
            width: 15
            height: 15
            x: capsule.width - 72
            anchors.verticalCenter: parent.verticalCenter
            tintName: fsArea.containsMouse ? "white" : "muted"
            opacity: capsule.expanded ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: 100 } }
            MouseArea {
                id: fsArea
                anchors.fill: parent
                anchors.margins: -9
                // Gated on the expansion BOOL: the two button MouseAreas are
                // INVISIBLE while collapsed but would still cover the 44px
                // circle (the -9 margins make the X's hit area span almost
                // all of it), so a click on the collapsed capsule exited
                // immersive instead of pinning it.
                enabled: capsule.expanded
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                    var w = header.Window.window
                    if (!w)
                        return
                    w.visibility = w.visibility === Window.FullScreen
                        ? Window.Windowed : Window.FullScreen
                }
            }
        }
        QbzIcon {
            name: "x"
            width: 14
            height: 14
            x: capsule.width - 36
            anchors.verticalCenter: parent.verticalCenter
            tintName: xArea.containsMouse ? "white" : "muted"
            opacity: capsule.expanded ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: 100 } }
            MouseArea {
                id: xArea
                anchors.fill: parent
                anchors.margins: -9
                enabled: capsule.expanded
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                // Exit path 2 (§5.4) — every exit funnels through the bridge's
                // open setter.
                onClicked: QbzImmersive.open = false
            }
        }
    }
}
