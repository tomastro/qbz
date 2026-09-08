// Segmented icon toggle — an N-way "pick one" rendered as icon-only segments
// inside a small rounded well.
//
// PROMOTED from views/local/LocalSegToggle.qml. Its Local geometry survives
// UNCHANGED as the defaults; the cell metrics and the active/hover look are
// now properties so the MyQBZ well can be the reference's other set (see TWO
// METRIC SETS below).
// It was never local-library-specific: Slint has the same widget
// three times over (ViewToggle grid/list at LocalLibraryView.slint:358,
// ModeToggle flat/tree at :384, and MyQbzShared.slint:57-70's list/grid), as
// three separate components only because its ToggleButton cannot take a model.
// Here one `segments` model covers all of them.
//
// Call sites: views/myqbz/MyQbzGridView.qml (2 segments, list/grid),
// views/myqbz/MyQbzDetailView.qml (3 segments, list/grid/expanded, `width: 90`
// — see the sizing note below), views/local/LocalToolbar.qml (3 instances:
// albums grid/list, folders grid/list, folders flat/tree) and
// views/local/LocalFolderDetail.qml (grid/list). The old
// views/local/LocalSegToggle.qml is GONE — this file is the only copy.
//
// SIZING. The well derives its width from the real segment row. With the
// DEFAULT metrics that is 26px cells, 2px gaps and 3px of well on each side:
// 2 segments -> 60px, 3 -> 88px. A caller may still override `width` for a
// roomier reference metric, but adding a segment can no longer leave its
// button visibly outside a stale 60px background.
//
// ── TWO METRIC SETS, AND WHY THE CELL LOOK IS PARAMETERISED ────────────────
// The DEFAULTS below are the Local Library set, unchanged: 26x24 r4 cells with
// 2px gaps (so 3px of dead well on each side), a 14px glyph, active fill
// surface-main and active tint `accent`, hover surface-hover. All four Local
// call sites (LocalToolbar.qml x3, LocalFolderDetail.qml) set NOTHING but
// `segments` / `mode` / `onSetMode`, so they keep exactly that.
//
// MyQBZ's toggle is a DIFFERENT widget in the reference and always was —
// `MyQbzShared.slint:49-71` (2 segments, 60x30) and
// `MixtapeDetailView.slint:1058-1084` (3 segments, 90x30) fill the well
// EDGE TO EDGE with `ToggleButton { sm: true }`, i.e. 30x30 cells, no gaps, a
// 15px glyph, and — the load-bearing part, spelled out at MyQbzShared.slint:47-48
// — active = `surface-hover`, NEVER accent (`ToggleButton.slint:19-20,24-28,32-33,38`:
// 30x30 at `sm`, active background surface-hover, hover surface-elevated, idle
// transparent, tint text-primary active / text-muted idle). So the two MyQBZ
// call sites pass that set explicitly. Nine properties at two call sites, in
// preference to a second near-identical component (TRACK-RULES §5).
//
// `segRadius` is the one number that is NOT the reference's: Slint's
// ToggleButton has border-radius 0 and relies on the WELL's `clip: true` to
// round the outer corners. A QML clip is a rectangular scissor and does not
// follow `radius`, so an edge-to-edge square cell would paint its own square
// corner over the well's curve. The curve therefore has to come from the cell,
// which also rounds the two INNER corners Slint leaves square — invisible at
// this contrast, since the active fill is `surfaceHover` = #10ffffff, a 6%
// white wash over the well's own surface-elevated.
//
// ADR-008: small-radius rectangle, never a pill.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root

    /// [{ id, icon }] — `id` is the value published through `setMode`, `icon`
    /// is a QbzIcon name that must exist in BOTH the `activeTint` and the
    /// `idleTint` directories. A missing tint variant renders nothing at all,
    /// with nothing logged.
    property var segments: []
    property string mode: ""
    signal setMode(string value)

    // --- cell metrics. DEFAULTS = the Local Library set (see the header). ---
    property int segWidth: 26
    property int segHeight: 24
    property int segRadius: 4
    property int segSpacing: 2
    property int glyphSize: 14
    /// Fill of the SELECTED cell. Local: surface-main. MyQBZ/ToggleButton:
    /// surface-hover — "active = surface-hover, NEVER accent"
    /// (MyQbzShared.slint:47-48).
    property color activeFill: theme.surfaceMain
    /// Fill of a hovered, unselected cell. `ToggleButton.slint:28` uses
    /// surface-elevated, which equals the WELL's own fill, i.e. the reference's
    /// hover is deliberately a no-op inside a well — pass `theme.surfaceElevated`
    /// to reproduce that, not a visible highlight.
    property color hoverFill: theme.surfaceHover
    /// QbzIcon tint names (theme/QbzIcon.qml's vocabulary). Local: accent /
    /// secondary. ToggleButton.slint:38: text-primary / text-muted.
    property string activeTint: "accent"
    property string idleTint: "secondary"

    QbzTheme { id: theme }

    readonly property int contentWidth: root.segments.length > 0
        ? root.segments.length * root.segWidth
            + (root.segments.length - 1) * root.segSpacing
        : 0
    implicitWidth: Math.max(60, root.contentWidth + 6)
    width: implicitWidth
    height: 30
    radius: 6
    // The well itself: surface-elevated @ 0.5 under the dynamic background
    // (SegmentedTabBar.slint:108).
    color: theme.ambientOn ? theme.surfaceElevatedA50 : theme.surfaceElevated
    // NO CLIP, and that turns a silent crop into a visible one. The invariant
    // a caller now owns: `width >= N*segWidth + (N-1)*segSpacing`. All six
    // call sites satisfy it today with integer geometry (54/60, 60/60, 90/90);
    // a future one that does not will OVERFLOW instead of being cropped
    // without a trace, which is the failure you want. The rounded corner comes
    // from `segRadius` on the cells — a QML clip is a rectangular scissor and
    // never shaped this well.

    Row {
        anchors.centerIn: parent
        spacing: root.segSpacing
        Repeater {
            model: root.segments
            delegate: Rectangle {
                id: seg
                required property var modelData
                readonly property bool active: modelData.id === root.mode
                width: root.segWidth
                height: root.segHeight
                radius: root.segRadius
                color: active ? root.activeFill
                     : segArea.containsMouse ? root.hoverFill : "transparent"
                QbzIcon {
                    name: seg.modelData.icon
                    width: root.glyphSize
                    height: root.glyphSize
                    anchors.centerIn: parent
                    tintName: seg.active ? root.activeTint : root.idleTint
                }
                MouseArea {
                    id: segArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.setMode(seg.modelData.id)
                }
            }
        }
    }
}
