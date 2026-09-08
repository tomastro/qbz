// Shared hover-tooltip overlay — the QML port of Slint's tooltip MECHANISM,
// not of one bubble: `shell/TooltipOverlay.slint` (bubble ABOVE the control,
// downward caret, clamped to the window) and `shell/SidebarTooltip.slint`
// (bubble to the RIGHT of a collapsed-rail row) are the same machine in Slint —
// a global state channel (`TooltipState` / `SidebarTooltipState`, state.slint:
// 4791 / 1893) that every hoverable control writes its text + anchor into, and
// ONE overlay mounted last in AppShell that renders it on top of everything.
//
// WHY AN OVERLAY AND NOT A `ToolTip`/`Popup` PER CALL SITE
//
// Two independent reasons, both measured:
//
//  1. QtQuick.Controls' `ToolTip` DOES NOT SIZE TO A CUSTOM contentItem.
//     Measured on Qt 6.11.1 with a probe (four variants, same contentItem):
//       Popup   { contentItem: Text {...} }  ->  implicitContentWidth = 157.8
//       ToolTip { contentItem: Text {...} }  ->  implicitContentWidth = 1
//     The Basic style's ToolTip binds
//       implicitWidth: max(implicitBackgroundWidth + insets,
//                          implicitContentWidth + padding)
//     so with implicitContentWidth stuck at 1 the popup collapses to a ~13x25
//     box while the Text — which a Popup never clips — keeps painting its full
//     178px. That is EXACTLY the reported defect: the label floats with no
//     background and a small detached background box sits beside it. Setting
//     `contentWidth`/`contentHeight` by hand papers over it; not being a
//     ToolTip at all removes it.
//  2. The rows that need a tooltip live inside a `clip: true` sidebar and
//     inside recycled delegates. A per-row popup would be one QQuickPopup per
//     playlist (the tree runs to hundreds) and would have to be torn down with
//     its delegate. The overlay is one Item at the shell root, and it stores
//     only NUMBERS + strings — never a reference to the anchoring item — so a
//     delegate can be destroyed under an open bubble without dangling.
//
// The overlay never takes the pointer (no MouseArea, `enabled: false`), so it
// cannot steal the hover that is keeping it open — the same rule
// ArtPreviewOverlay.qml documents.
//
// PLACEMENTS (both flavours keep their own Slint numbers, they are not one
// style with two anchors):
//   showRight()  SidebarTooltip.slint — surface-elevated, Radius.sm, 1px
//                border-muted, 10/10/5/5 padding, 12px/w500 text-primary,
//                x = row.right + 6, vertically centred on the row.
//   showAbove()  TooltipOverlay.slint — surface-elevated, radius 4, no border,
//                9/9/5/5 padding, 11px/medium text-primary, downward caret,
//                y = anchor.top - height - 9, centre clamped 8px from the
//                window edges with the caret still pointing at the control.
//   showSummary() APPLIED-FILTERS bubble — this port's own, no Slint twin.
//                Same machine, structured content: a list of {group, values}
//                instead of one string, laid out UNDER the control (filter
//                funnels live in toolbars, so the space is below them) with an
//                upward caret, flipping above when it would leave the window.
//                10/10/8/8 padding, 6px between groups — one step up from the
//                one-line bubble and one step below a flyout's 14/12, which is
//                a DELIBERATE departure from the control-alignment standard
//                (that document sizes flyouts, and this is not one).
//                Group headers are 11px semibold text-muted rather than the
//                standard GroupHeader (ALL-CAPS 11px, letter-spacing 1.5):
//                inside an 11px bubble the tracked caps are unreadable.
//                Values are comma-separated TEXT, never chips — ADR-008.
//
// DELIBERATE DEVIATIONS FROM SLINT, both stated because they are visible:
//   - MAX WIDTH + ELISION. Neither Slint bubble caps its width; a 200-character
//     playlist name would run off the window. `maxWidth` (320 default) caps it
//     and the label elides. Set maxWidth: 0 for the uncapped Slint behaviour.
//   - SHADOW. Slint draws `drop-shadow-blur: 16px` / `10px`. The port's usual
//     approximation is used instead: one offset rectangle behind the bubble,
//     exactly like SidebarNowPlayingDock.qml does for the cover.
//     SUPERSEDED (2026-07-29): the justification here used to be "QtQuick.
//     Effects renders NOTHING". Effects need shaders, and this port runs on the
//     GPU (OpenGL RHI, measured); that note came from an offscreen session,
//     which forces the software renderer by definition — see
//     theme/RoundedImage.qml, which now detects the software path with
//     `GraphicsInfo.api` instead of assuming it. A real MultiEffect shadow is
//     therefore possible; it is a VISUAL change needing its own parity pass
//     against Slint's `drop-shadow-blur`, so it is deliberately NOT done here.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    // Purely decorative — never take a click, never take the hover that is
    // keeping the bubble open.
    enabled: false

    // ---- State (the QML stand-in for Slint's TooltipState globals) --------
    // `ownerKey` is the race-safe owner id Slint carries in
    // SidebarTooltipState.id: a row that loses the pointer only clears the
    // bubble if it still owns it, so sliding straight from one row to the next
    // never blanks it (Sidebar.slint:199-203).
    property string ownerKey: ""
    property string label: ""
    // 0 = right of the anchor (sidebar rail), 1 = above it (caret bubble),
    // 2 = below it (applied-filters summary, upward caret; flips to 1 when it
    // would fall off the bottom).
    property int placement: 0
    // Anchor rectangle, in THIS overlay's coordinates. Captured as plain
    // numbers on show — never a reference to the anchoring Item, so a recycled
    // or destroyed delegate cannot dangle here.
    property real anchorX: 0
    property real anchorY: 0
    property real anchorW: 0
    property real anchorH: 0
    // 0 disables the cap (Slint's own behaviour).
    property int maxWidth: 320

    // ---- Summary mode -----------------------------------------------------
    // [{ group: "Genre", values: ["Rock", "Jazz"] }, …]. Empty = the classic
    // one-line bubble, so nothing about the sidebar changes.
    property var groups: []
    property int summaryMinWidth: 160

    readonly property bool isSummary: root.placement === 2 && root._rows.length > 0
    readonly property bool shown: ownerKey !== ""
        && (label !== "" || root.isSummary)

    // OVERFLOW, and why there is no scrolling.
    //
    // A scrollable tooltip cannot be scrolled: this overlay is `enabled: false`
    // by construction (it must never take the pointer, or it steals the hover
    // that is keeping it open — see the header). So the rule for "too much
    // content" has to be SUMMARY, not scroll, and it is staged:
    //   1. at most 3 values per group, then "+N more";
    //   2. at most 5 groups, then one final "+N more filters" row;
    //   3. every row elides inside the fixed width;
    //   4. a hard width cap of min(maxWidth, 40% of the window), floor 160.
    // With those, the height cap is never the thing that truncates.
    readonly property int _maxValues: 3
    readonly property int _maxGroups: 5
    readonly property var _rows: {
        var gs = root.groups || []
        var out = []
        var n = Math.min(gs.length, root._maxGroups)
        for (var i = 0; i < n; i++) {
            var g = gs[i] || {}
            var vals = g.values || []
            if (vals.length === 0)
                continue
            var line = vals.slice(0, root._maxValues).join(", ")
            if (vals.length > root._maxValues)
                line += "  " + QbzSession.tr("+{} more", QbzSession.trRev)
                        .replace("{}", vals.length - root._maxValues)
            out.push({ head: g.group || "", body: line })
        }
        if (gs.length > root._maxGroups)
            out.push({
                head: "",
                body: QbzSession.tr("+{} more filters", QbzSession.trRev)
                      .replace("{}", gs.length - root._maxGroups)
            })
        return out
    }

    // ---- API --------------------------------------------------------------
    // `item` is any Item; its bounds are mapped into this overlay. `key` is the
    // owner id used by hide().
    function showRight(item, key, text) {
        if (!item || !text)
            return
        root._capture(item)
        root.placement = 0
        root.label = text
        root.ownerKey = key
    }
    function showAbove(item, key, text) {
        if (!item || !text)
            return
        root._capture(item)
        root.placement = 1
        root.label = text
        root.ownerKey = key
    }
    // The applied-filters bubble. `groups` is built by the HOST, not read from
    // a global: of the app's filter surfaces roughly a third keep their state
    // in QML and the rest in Rust documents, so there is no single source this
    // could read. Passing an empty (or all-empty) array is a no-op — a surface
    // with nothing applied shows its ordinary one-line tooltip instead.
    function showSummary(item, key, groups) {
        if (!item)
            return
        const p = item.mapToItem(root, 0, 0)
        root.showSummaryAt(key, p.x, p.y, item.width, item.height, groups)
    }
    // The same, from plain NUMBERS — the form the bridge channel uses, and the
    // honest one: this overlay has never wanted an Item, only a rectangle.
    function showSummaryAt(key, x, y, w, h, groups) {
        if (!groups || groups.length === 0)
            return
        root.groups = groups
        if (root._rows.length === 0) {
            root.groups = []
            return
        }
        root.anchorX = x
        root.anchorY = y
        root.anchorW = w
        root.anchorH = h
        root.label = ""
        root.placement = 2
        root.ownerKey = key
    }
    // Race-safe close: only the owner may clear the bubble.
    /// The whole boilerplate of a hover tooltip, in one call.
    ///
    /// Every call site was writing the same four lines — an
    /// `onContainsMouseChanged` that branches on the flag and remembers to
    /// pass the same key to both `showAbove` and `hide`. Two of them got the
    /// key wrong, which does not fail: the bubble simply never goes away,
    /// because `hide` only clears a tooltip it owns. One function, one key,
    /// no way to mismatch them.
    ///
    /// `on` is the hover flag (a MouseArea's `containsMouse`, a HoverHandler's
    /// `hovered`); `item` is what the bubble points at.
    function hover(on, item, key, text) {
        if (on)
            root.showAbove(item, key, text)
        else
            root.hide(key)
    }

    function hide(key) {
        if (root.ownerKey === key) {
            root.ownerKey = ""
            root.groups = []
        }
    }
    // Unconditional close — for the events that invalidate the anchor itself
    // (the sidebar leaving the mini state, a flyout taking the same slot, a
    // view change).
    function hideAll() {
        root.ownerKey = ""
        root.groups = []
    }

    function _capture(item) {
        const p = item.mapToItem(root, 0, 0)
        root.anchorX = p.x
        root.anchorY = p.y
        root.anchorW = item.width
        root.anchorH = item.height
    }

    // ---- Geometry ---------------------------------------------------------
    // The bubble is sized by its LABEL, which is the whole point: `bubbleW`
    // reads the Text's natural implicitWidth (NoWrap + ElideRight leaves
    // implicitWidth at the unelided width, so assigning the Text a narrower
    // width below is not a binding loop) and the cap only ever shrinks it.
    readonly property real padH: root.isSummary ? 10 : (placement === 0 ? 10 : 9)
    readonly property real padV: root.isSummary ? 8 : 5
    // The width cap for the summary is tighter than the plain bubble's: 40% of
    // the window, so a wide screen does not get a 320px slab floating over a
    // toolbar, with a 160px floor so a one-word filter still reads as a bubble.
    readonly property real summaryCap: Math.max(
        root.summaryMinWidth,
        Math.min(maxWidth > 0 ? maxWidth : 320, root.width * 0.4))
    readonly property real bubbleW: root.isSummary
        ? Math.max(root.summaryMinWidth,
                   Math.min(sumMeasure.implicitWidth + 2 * padH, root.summaryCap))
        : Math.min(tipText.implicitWidth + 2 * padH,
                   maxWidth > 0 ? maxWidth : Number.MAX_VALUE)
    readonly property real bubbleH: root.isSummary
        ? sumCol.implicitHeight + 2 * padV
        : tipText.implicitHeight + 2 * padV

    // Right placement (SidebarTooltip.slint:17-19): +6px off the row's right
    // edge, vertically centred on the row. Flips to the row's LEFT when the
    // bubble would leave the window — Slint never clamps this one because its
    // rail is always at x=0 with the whole window to its right; the flip costs
    // nothing and keeps a narrow window honest.
    readonly property real rightX: (anchorX + anchorW + 6 + bubbleW <= root.width - 8)
        ? anchorX + anchorW + 6
        : Math.max(8, anchorX - bubbleW - 6)
    readonly property real rightY: Math.max(8, Math.min(
        root.height - bubbleH - 8,
        anchorY + Math.round((anchorH - bubbleH) / 2)))

    // Above placement (TooltipOverlay.slint:17-26): centred on the control,
    // clamped 8px from both window edges, 9px of air over the control.
    readonly property real aboveCx: anchorX + anchorW / 2
    readonly property real aboveX: Math.max(8, Math.min(
        Math.max(8, root.width - bubbleW - 8), aboveCx - bubbleW / 2))
    readonly property real aboveY: anchorY - bubbleH - 9

    // Below placement (summary): 9px of air under the control, same horizontal
    // clamp as `above`. Flips ABOVE when the bubble would leave the bottom of
    // the window — a filter funnel near the foot of a short window is exactly
    // where a toolbar ends up on a laptop.
    readonly property real belowY: anchorY + anchorH + 9
    readonly property bool flipUp: root.isSummary
        && (root.belowY + root.bubbleH > root.height - 8)
        && (root.aboveY >= 8)
    // ...and the mirror of it for the ABOVE placement, which needed the same
    // escape and did not have it: a control near the TOP of its host — the
    // disc pane's close button sits 16px under the header — put the bubble at
    // a negative y, where the header simply painted over it. Flips BELOW when
    // there is no room above and there is room under.
    readonly property bool flipDown: root.placement === 1
        && (root.aboveY < 8)
        && (root.belowY + root.bubbleH <= root.height - 8)
    // True when the caret points UP at a control above the bubble — i.e.
    // whenever the bubble ended up UNDER what it describes.
    readonly property bool caretUp: (root.isSummary && !root.flipUp) || root.flipDown

    // ---- The bubble -------------------------------------------------------
    Item {
        id: bubble
        visible: root.shown
        x: Math.round(root.placement === 0 ? root.rightX : root.aboveX)
        y: Math.round(root.placement === 0 ? root.rightY
                      : root.isSummary ? (root.flipUp ? root.aboveY : root.belowY)
                      : (root.flipDown ? root.belowY : root.aboveY))
        width: Math.ceil(root.bubbleW)
        height: Math.ceil(root.bubbleH)

        // Shadow approximation (see the header): one offset rect, same radius.
        Rectangle {
            x: 0
            y: root.placement === 0 ? 4 : (root.isSummary ? 3 : 2)
            width: parent.width
            height: parent.height
            radius: root.placement === 0 ? theme.radiusSm : 4
            color: "#66000000"
        }

        Rectangle {
            anchors.fill: parent
            radius: root.placement === 0 ? theme.radiusSm : 4
            color: theme.surfaceElevated
            // Only the sidebar bubble is bordered (SidebarTooltip.slint:24-25);
            // TooltipOverlay.slint has none.
            border.width: root.placement === 0 ? 1 : 0
            border.color: theme.borderMuted

            Text {
                id: tipText
                visible: !root.isSummary
                x: root.padH
                y: root.padV
                width: Math.max(0, parent.width - 2 * root.padH)
                text: root.label
                color: theme.textPrimary
                font.pixelSize: root.placement === 0 ? 12 : 11
                font.weight: theme.weightMedium
                verticalAlignment: Text.AlignVCenter
                wrapMode: Text.NoWrap
                elide: Text.ElideRight
            }

            // Summary body. Every row is NoWrap + elide, deliberately: with
            // `Text.Wrap` a Text's implicitWidth depends on the width assigned
            // to it, and `bubbleW` is computed FROM those implicit widths — a
            // binding loop. NoWrap keeps implicitWidth at the unelided width,
            // which is the same reason the one-line bubble above uses it. The
            // values are already capped to three plus a "+N more", so a row is
            // short by construction and the cap rarely bites.
            Column {
                id: sumCol
                visible: root.isSummary
                x: root.padH
                y: root.padV
                width: Math.max(0, parent.width - 2 * root.padH)
                spacing: 6

                Repeater {
                    model: root.isSummary ? root._rows : []
                    delegate: Column {
                        width: sumCol.width
                        spacing: 1
                        Text {
                            visible: (modelData.head || "") !== ""
                            width: parent.width
                            text: modelData.head || ""
                            color: theme.textMuted
                            font.pixelSize: 11
                            font.weight: theme.weightSemibold
                            wrapMode: Text.NoWrap
                            elide: Text.ElideRight
                        }
                        Text {
                            width: parent.width
                            text: modelData.body || ""
                            color: theme.textSecondary
                            font.pixelSize: 11
                            wrapMode: Text.NoWrap
                            elide: Text.ElideRight
                        }
                    }
                }
            }
        }

        // Off-screen measuring pass. A Column's implicitWidth is the widest of
        // its children, and an UNASSIGNED Text's width is its implicitWidth —
        // so this reports the natural width of the widest row without any of
        // the visible rows influencing it. It only ever holds delegates while
        // the summary is up (`_rows` is empty otherwise), so at rest it costs
        // nothing.
        Column {
            id: sumMeasure
            visible: false
            spacing: 6
            Repeater {
                model: root.isSummary ? root._rows : []
                delegate: Column {
                    spacing: 1
                    Text {
                        visible: (modelData.head || "") !== ""
                        text: modelData.head || ""
                        font.pixelSize: 11
                        font.weight: theme.weightSemibold
                    }
                    Text {
                        text: modelData.body || ""
                        font.pixelSize: 11
                    }
                }
            }
        }

        // Downward caret, above-placement only. Slint draws a Path
        // ("M 0 0 L 5 6 L 10 0 Z", TooltipOverlay.slint:56-64) and keeps it
        // pointing at the REAL control centre even when the bubble is clamped.
        // Canvas is this port's polygon primitive (Canvas.Immediate — the
        // Cooperative/Threaded render strategies segfault against list
        // recycling; see theme/RoundedImage.qml).
        Canvas {
            id: caret
            visible: root.placement === 1 || root.isSummary
            width: 10
            height: 6
            x: Math.round(Math.max(4, Math.min(parent.width - width - 4,
                                               root.aboveCx - bubble.x - width / 2)))
            y: root.caretUp ? -(height - 1) : parent.height - 1
            renderTarget: Canvas.Image
            renderStrategy: Canvas.Immediate
            // The fill has to follow the theme, and a Canvas does not repaint
            // on a colour binding by itself.
            property color fill: theme.surfaceElevated
            // The triangle points DOWN at a control below the bubble, and UP at
            // one above it. A Canvas repaints on neither of these by itself.
            property bool up: root.caretUp
            onFillChanged: caret.requestPaint()
            onUpChanged: caret.requestPaint()
            onVisibleChanged: if (visible) caret.requestPaint()
            onPaint: {
                var ctx = caret.getContext("2d")
                if (!ctx)
                    return
                ctx.reset()
                ctx.clearRect(0, 0, caret.width, caret.height)
                ctx.fillStyle = caret.fill
                ctx.beginPath()
                if (caret.up) {
                    ctx.moveTo(0, caret.height)
                    ctx.lineTo(caret.width / 2, 0)
                    ctx.lineTo(caret.width, caret.height)
                } else {
                    ctx.moveTo(0, 0)
                    ctx.lineTo(caret.width / 2, caret.height)
                    ctx.lineTo(caret.width, 0)
                }
                ctx.closePath()
                ctx.fill()
            }
        }
    }
}
