// MiniQueueSurface — surface 3 (2026-08-03 miniplayer/tray contract §4.4),
// port of `crates/qbz-ui/ui/miniplayer/MiniQueueSurface.slint` (139 L, read
// whole).
//
// The current track plus the FULL upcoming list — not capped at 20, not
// paginated, not de-duplicated. Rows mirror the main sidebar's UP NEXT rows
// and stop there: no hover actions, no heart, no context menu, no
// drag-reorder, no remove, no stop-after marker, no section headers. The
// header of the reference states the rule this surface exists under — "the
// mini must not have MORE than the main" (:6).
//
// TWO THINGS THAT LOOK LIKE OMISSIONS AND ARE NOT:
//
//  - NO ARTWORK. The reference builds these rows with an EMPTY prior-image map
//    (`crates/qbz/src/queue.rs:1032-1033`) and renders a POSITION NUMBER in
//    its place (§15 trap 11). Do not "improve" it.
//  - HOVER BEATS ACTIVE on the row background. The reference's ternary tests
//    hover FIRST (`:45-47`), so hovering the playing row paints surfaceHover,
//    not the accent tint (§15 trap 15).
//
// THE DATA is `QbzMini.miniQueueJson`, built by `mini_qt::mini_queue_doc` and
// published from the ONE queue funnel (`queue_qt::publish` -> its own
// identical-document guard, §7-M9), never from a fetch of this surface's own
// (§7-M1). Neither existing document serves: `queue_json` is PAGED at 40 and
// `coverflow_json` carries no duration and no explicit flag (§4.4.1).
// `currentId` — not a per-row flag — is the highlight key (`:42`).
//
// DIVERGENCE §13-D7: a recycling ListView with `reuseItems: true`, where the
// reference is a Flickable + VerticalLayout that materialises every row. An
// unbounded queue in a 380 px window is exactly the case TRACK-RULES §6
// forbids (§7-M5). The structural consequence is deliberate: the delegate
// reads `index`, never a captured loop variable, and it carries no popup of
// its own — with reuseItems every recycled delegate would get its own
// instance (`PmListRow.qml:14`, `PmFolderMenu.qml:3`).
//
// NO COLOUR LITERALS LIVE HERE. §4.11's only entry for this file is
// `accent.with-alpha(0.10)`, which is a token expression rather than a hex
// string, so the #RRGGBBAA -> #AARRGGBB conversion the port owes every Slint
// literal has nothing to convert on this surface.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    // Parsed ONCE per publish, not per row (§7). The bridge's default is the
    // full-shape `{"currentId":"","rows":[]}` (mini_qt.rs's MINI_QUEUE_EMPTY),
    // so `.rows` exists in the pre-publish frame too; the try/catch is the
    // same belt MiniShell wears over its own queue read.
    // APPLIED IMPERATIVELY, not bound — and that is §7-M9, not style.
    //
    // A bound `model:` hands the ListView a NEW JS array on every publish whose
    // content differs, which is a full model reset: every delegate destroyed
    // and `contentY` clamped to 0. The upstream identical-document guard
    // (`queue_qt.rs`'s LAST_MINI_QUEUE_JSON) only suppresses IDENTICAL
    // republishes; a real change is exactly what it lets through. So a user
    // scrolled to row 120 of a long queue is yanked back to the top every time
    // the track advances — once per track, forever. The reference does not
    // have this problem because it keeps the offset on the Flickable and only
    // rebuilds the `for` inside it (MiniQueueSurface.slint:30-36).
    //
    // The cure is the port's own, and its comment describes this exact failure:
    // Cortinilla.qml:34-70 (latch contentY, apply, restore under Qt.callLater
    // so a burst of republishes costs ONE restore).
    property var doc: ({ "currentId": "", "rows": [] })
    property var rows: []
    property string currentId: ""

    function parseDoc() {
        try {
            return JSON.parse(QbzMini.miniQueueJson)
        } catch (e) {
            return ({ "currentId": "", "rows": [] })
        }
    }

    property real _restoreY: 0
    function applyDoc() {
        root._restoreY = list.contentY
        root.doc = root.parseDoc()
        root.rows = root.doc.rows || []
        root.currentId = root.doc.currentId !== undefined ? root.doc.currentId : ""
        Qt.callLater(root._restoreScroll)
    }
    function _restoreScroll() {
        var maxY = Math.max(0, list.contentHeight - list.height)
        list.contentY = Math.max(0, Math.min(root._restoreY, maxY))
    }

    Component.onCompleted: root.applyDoc()
    Connections {
        target: QbzMini
        function onMiniQueueJsonChanged() { root.applyDoc() }
    }

    // --- Empty state (MiniQueueSurface.slint:15-23) -------------------------
    // A single Text at 100% x 100%, centred on BOTH axes inside itself — not a
    // centred label inside a wrapper.
    Text {
        anchors.fill: parent
        visible: root.rows.length === 0
        text: QbzSession.tr("Queue is empty", QbzSession.trRev)
        color: theme.textMuted
        font.pixelSize: 13
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }

    // --- The list (:26-40, :129-137) ---------------------------------------
    // padding-top 6 / padding-bottom 8 (asymmetric, and that IS the
    // reference's) and NO spacing — the rows are flush, so the pitch equals
    // the 44 px row height. Side padding lives INSIDE the row, because a
    // vertical ListView writes its delegates' `x` and a delegate-side `x`
    // would be overwritten (`AddToMixtapeModal.qml:352-361`).
    ListView {
        id: list
        anchors.fill: parent
        visible: root.rows.length > 0
        topMargin: 6
        bottomMargin: 8
        spacing: 0
        cacheBuffer: 200
        reuseItems: true
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: root.rows

        delegate: Rectangle {
            id: row

            required property var modelData
            required property int index

            width: list.width
            height: 44
            radius: 4
            antialiasing: true

            // `item.id != "" && item.id == current-track-id` (:42) — BOTH
            // halves, so an empty id is never active.
            readonly property bool active: row.modelData.id !== ""
                                           && row.modelData.id === root.currentId

            // :45-47 — hover is tested FIRST. See the header.
            color: rowArea.containsMouse
                   ? theme.surfaceHover
                   : (row.active
                      ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.10)
                      : "transparent")

            // Declared FIRST so every visual below paints on top of it, which
            // is the order the reference declares its TouchArea in and for the
            // same stated reason (:49-56). It is the row's ONLY hit area.
            MouseArea {
                id: rowArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                // `index`, never a captured variable — the delegate is
                // RECYCLED (§13-D7). The bridge owns the rest: remote-first,
                // the row-0 test against `currentId`, and the flat upcoming
                // index (§4.4.3).
                onClicked: QbzMini.queuePlay(row.index)
            }

            // The active bar (:58-65): 3 x 36 at x = 0, i.e. UNDER the 12 px
            // left padding the number column starts at.
            Rectangle {
                visible: row.active
                x: 0
                y: 4
                width: 3
                height: row.height - 8
                radius: 3
                color: theme.accent
            }

            // Number column (:77-86): a FIXED 22 px, centred both ways, the
            // 1-based DISPLAY position over the whole rendered list — which is
            // the unfiltered list, because this surface has no filter (§8.1).
            Text {
                id: num
                x: 12
                width: 22
                anchors.verticalCenter: parent.verticalCenter
                text: String(row.index + 1)
                color: row.active ? theme.accent : theme.textMuted
                font.pixelSize: 12
                horizontalAlignment: Text.AlignHCenter
            }

            // Duration (:116-123): NO fixed width — it sizes to its content,
            // so the middle column's width moves with the string.
            Text {
                id: dur
                anchors.right: parent.right
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                text: row.modelData.duration !== undefined ? row.modelData.duration : ""
                color: theme.textMuted
                font.pixelSize: 11
            }

            // The middle column (:88-113). The reference's
            // `VerticalLayout { horizontal-stretch: 1; alignment: center }`:
            // the stretch is the anchor pair below, and `alignment: center` —
            // which makes the two texts a vertically-centred block of their
            // natural heights rather than a stretch across the 36 px content
            // box — is anchors.verticalCenter on a Column whose height is its
            // implicit one. It is also what keeps a row with no artist
            // optically centred instead of top-aligned.
            //
            // The 9 px gaps on both sides are the content layout's `spacing`.
            Column {
                id: mid
                anchors.left: num.right
                anchors.leftMargin: 9
                anchors.right: dur.left
                anchors.rightMargin: 9
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2

                // Title + badge row (:92-106), gap 5. Same shape as the two
                // display surfaces: the title's box shrinks by the badge, and
                // the badge is CENTRED on the title line (the reference wraps
                // it in a `VerticalLayout { alignment: center }`), not
                // baseline-aligned.
                Item {
                    width: mid.width
                    height: title.implicitHeight

                    Text {
                        id: title
                        anchors.left: parent.left
                        // badge.width is the constant 13, so this is not a loop.
                        width: parent.width - (badge.visible ? badge.width + 5 : 0)
                        text: row.modelData.title !== undefined ? row.modelData.title : ""
                        color: row.active ? theme.textPrimary : theme.textSecondary
                        font.pixelSize: 12
                        font.weight: Font.DemiBold
                        elide: Text.ElideRight
                    }

                    MiniExplicitBadge {
                        id: badge
                        visible: row.modelData.explicit === true
                        anchors.left: title.right
                        anchors.leftMargin: 5
                        anchors.verticalCenter: title.verticalCenter
                    }
                }

                // CONDITIONAL (:107-112) — an empty artist makes this a
                // ONE-line row, and in a Column an invisible child takes its
                // spacing with it, which is exactly Slint's `if artist != ""`.
                Text {
                    width: mid.width
                    visible: row.modelData.artist !== undefined
                             && row.modelData.artist !== ""
                    text: row.modelData.artist !== undefined ? row.modelData.artist : ""
                    color: theme.textMuted
                    font.pixelSize: 10
                    elide: Text.ElideRight
                }
            }
        }
    }

    // The scrollbar (:129-137): a SIBLING of the list overlaid on the rows'
    // right edge, not a gutter carved out of them. The Slint header claims its
    // bar "sits in its own gutter" while this surface overlays it on
    // full-width rows; the overlay is reproduced 1:1 (§12-P3) — it auto-hides,
    // and reserving 14 px would move every row's duration column.
    //
    // `target` is REQUIRED (§15 trap 36) and `visible` is NOT assigned here:
    // the component owns that binding, and overriding it gives a short list a
    // full-height thumb on gutter hover (`PlaylistManagerView.qml:221-224`).
    QbzScrollBar {
        target: list
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
    }
}
