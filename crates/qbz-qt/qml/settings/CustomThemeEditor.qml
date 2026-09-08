// CustomThemeEditor — the base-token swatch grid of the custom theme, plus
// the ONE shared inline colour picker that expands under whichever swatch is
// open. The QML port of the grid block at
// crates/qbz-ui/ui/settings/AppearanceSettings.slint:333-425 (the
// TokenGroupLabel and CustomTokenCell sub-components live at :33-38 and
// :44-76).
//
// It is mounted DIRECTLY in the Appearance Column, never inside a SettingRow:
// SettingRow hardcodes its height (52 / 64, controls/SettingRow.qml:18) and
// vertically centres one control, so a ~250px picker inside one would be
// clipped. The reference places the grid outside its own SettingRow for the
// same reason (a bare VerticalLayout at AppearanceSettings.slint:333).
//
// STATE. The eleven swatch colours + the polarity ride
// QbzShell.customThemeJson (src/custom_theme_qt.rs) as
// {"isDark":bool,"tokens":{"<kebab-key>":"#aarrggbb"}} — never local truth.
// Which swatch is OPEN is the exception: that is pure view state (the Slint
// keeps it in AppearanceState.custom-open-token only because Slint has no
// other place to put it) and stays here.
//
// The token keys are the SAME kebab strings the Rust controller matches on
// (custom_theme_qt::TOKEN_KEYS) and the same ones the Slint rows carry, so the
// vocabulary is single-sourced across both frontends.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Column {
    property bool kioskHost: false

    id: root

    QbzTheme { id: theme }

    /// The token key whose picker is expanded; "" = none open.
    property string openToken: ""

    /// Collapse the picker — the seed action re-seeds every swatch, so leaving
    /// a picker open on a token that just changed underneath is the same
    /// stale-editor state the reference clears (custom_theme.rs:187).
    function closePicker() { root.openToken = "" }

    /// Leaving the Custom theme collapses the picker too. The reference clears
    /// `custom-open-token` inside `push_base_to_state`, which runs on EVERY
    /// entry into Custom (custom_theme.rs:187, main.rs:11415) — without this,
    /// Custom -> OLED -> Custom comes back with a picker still expanded under
    /// whatever swatch was last open.
    onVisibleChanged: if (!visible) root.openToken = ""

    readonly property var doc: {
        try {
            return JSON.parse(QbzShell.customThemeJson)
        } catch (e) {
            return ({})
        }
    }
    readonly property var tokens: doc.tokens || ({})
    readonly property bool isDark: doc.isDark === true

    /// One swatch colour by key. Guarded: an empty document during the very
    /// first frames must not bind `color` to undefined.
    function tokenColor(key) {
        const v = tokens[key]
        return v === undefined ? "transparent" : v
    }

    // Width comes DOWN from the settings column, never up from the children:
    // the picker binds its own width to this, so letting a Column derive its
    // implicitWidth from that child would be a binding loop.
    width: parent ? parent.width : 0
    spacing: 14
    topPadding: 8
    bottomPadding: 8

    // A small muted sub-group label (AppearanceSettings.slint:33-38).
    component TokenGroupLabel: Text {
        color: theme.textMuted
        font.pixelSize: root.kioskHost ? (10) * 1.2 : (10)
        font.letterSpacing: 1
        font.weight: theme.weightSemibold
    }

    // One editable base-token CELL: a compact swatch with its label
    // underneath, laid inline with its group siblings. Clicking TOGGLES the
    // shared picker (clicking the open one again closes it).
    component TokenCell: Column {
        id: cell
        property string tokenKey: ""
        property string label: ""
        readonly property bool open: root.openToken === cell.tokenKey

        spacing: 6

        Rectangle {
            id: swatch
            width: 56
            height: 32
            radius: theme.radiusSm
            border.width: cell.open || swatch.activeFocus ? 2 : 1
            border.color: cell.open || swatch.activeFocus
                ? theme.accent : theme.borderSubtle
            color: root.tokenColor(cell.tokenKey)
            activeFocusOnTab: visible && enabled
            Accessible.role: Accessible.Button
            Accessible.name: cell.label
            Accessible.onPressAction: swatch.activate()
            function activate() {
                root.openToken = cell.open ? "" : cell.tokenKey
            }
            Keys.onPressed: function (event) {
                if (!event.isAutoRepeat
                        && (event.key === Qt.Key_Space
                            || event.key === Qt.Key_Return
                            || event.key === Qt.Key_Enter)) {
                    swatch.activate()
                    event.accepted = true
                }
            }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onPressed: swatch.forceActiveFocus()
                onClicked: swatch.activate()
            }
        }
        Text {
            width: 56
            text: cell.label
            color: cell.open ? theme.textPrimary : theme.textMuted
            font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            horizontalAlignment: Text.AlignHCenter
            elide: Text.ElideRight
        }
    }

    // --- Row 1: SURFACES / TEXT / ACCENT ---------------------------------
    Row {
        spacing: 28
        Column {
            spacing: 8
            TokenGroupLabel { text: QbzSession.tr("SURFACES", QbzSession.trRev) }
            Row {
                spacing: 10
                TokenCell { tokenKey: "surface-main"; label: QbzSession.tr("Background", QbzSession.trRev) }
                TokenCell { tokenKey: "surface-card"; label: QbzSession.tr("Surface", QbzSession.trRev) }
                TokenCell { tokenKey: "surface-elevated"; label: QbzSession.tr("Elevated", QbzSession.trRev) }
            }
        }
        Column {
            spacing: 8
            TokenGroupLabel { text: QbzSession.tr("TEXT", QbzSession.trRev) }
            Row {
                spacing: 10
                TokenCell { tokenKey: "text-primary"; label: QbzSession.tr("Text", QbzSession.trRev) }
                TokenCell { tokenKey: "text-secondary"; label: QbzSession.tr("Secondary text", QbzSession.trRev) }
            }
        }
        Column {
            spacing: 8
            TokenGroupLabel { text: QbzSession.tr("ACCENT", QbzSession.trRev) }
            Row {
                spacing: 10
                TokenCell { tokenKey: "accent"; label: QbzSession.tr("Accent", QbzSession.trRev) }
            }
        }
    }

    // --- Row 2: STATUS / OTHER -------------------------------------------
    Row {
        spacing: 28
        Column {
            spacing: 8
            TokenGroupLabel { text: QbzSession.tr("STATUS", QbzSession.trRev) }
            Row {
                spacing: 10
                TokenCell { tokenKey: "danger"; label: QbzSession.tr("Danger", QbzSession.trRev) }
                TokenCell { tokenKey: "warning"; label: QbzSession.tr("Warning", QbzSession.trRev) }
                TokenCell { tokenKey: "success"; label: QbzSession.tr("Success", QbzSession.trRev) }
            }
        }
        Column {
            spacing: 8
            TokenGroupLabel { text: QbzSession.tr("OTHER", QbzSession.trRev) }
            Row {
                spacing: 10
                TokenCell { tokenKey: "border"; label: QbzSession.tr("Border", QbzSession.trRev) }
                TokenCell { tokenKey: "favorite"; label: QbzSession.tr("Favorite", QbzSession.trRev) }
            }
        }
    }

    // --- The ONE shared picker, for whichever token is open ---------------
    //
    // `visible` and not a Loader: the picker is ~250px of static QML with no
    // model behind it, and keeping the instance alive across opens preserves
    // nothing (it re-seeds from `value` on every external change anyway) while
    // a Loader would cost a rebuild per swatch click. An invisible child of a
    // Column is skipped by the positioner, so the collapsed state leaves no
    // gap — the same idiom AppearanceSettings.qml uses for its platform rows.
    QbzColorPicker {
        width: root.width
        visible: root.openToken !== ""
        value: root.tokenColor(root.openToken)
        // Live drag: re-derive + repaint now, write debounced in Rust.
        onPicked: function (hex) { QbzShell.customSetToken(root.openToken, hex) }
        // Drag end: the settled value lands on disk without waiting the debounce.
        onReleased: QbzShell.customFlush()
        // A committed HEX is a discrete edit — set it and persist it.
        onHexCommitted: function (hex) {
            QbzShell.customSetToken(root.openToken, hex)
            QbzShell.customFlush()
        }
    }
}
