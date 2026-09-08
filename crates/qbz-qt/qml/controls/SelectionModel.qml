// SelectionModel — THE selection rules, in one place.
//
// Five views (Album, Artist, Playlist, Label, LocalAlbum) each carried their
// own hand-copied `selected` map and a `toggleSelected(id)`, and none of them
// knew what a modifier key was: every click was a single toggle, so picking
// 40 tracks meant 40 clicks. Slint and Tauri have had Excel-style selection
// since 2026-06 (crates/qbz/src/selection.rs, src/lib/utils/multiSelect.ts);
// this is that port, with the rule shared instead of copied a sixth time.
//
// ── THE RULES, and where they come from ───────────────────────────────────
// Straight off Tauri's `applyShiftRange`, which is the behaviour that has
// been in the owner's hands the longest:
//
//   plain click   toggle just this row. It does NOT clear the others — in
//                 select mode the whole point is accumulating a set.
//   Ctrl / Cmd    the SAME as a plain click, and that is parity, not a
//                 shortcut: because a plain click already accumulates,
//                 "add this one without disturbing the rest" is what both
//                 already do. Tauri reads only `shiftKey` at click time and
//                 Slint matched it.
//   Shift         select the range between the anchor and this row,
//                 ADDITIVELY — a shift-click never deselects, so re-dragging
//                 a range cannot eat the set. The anchor does not move, which
//                 is what lets a range be adjusted by shift-clicking again.
//
// ── THE ANCHOR IS AN ID, NOT AN INDEX ─────────────────────────────────────
// Rows get re-sorted and filtered under a live selection — every one of these
// views has a sort control and Local Library has a filter box. An index
// anchor silently points at a different track the moment the order changes;
// an id is re-resolved against the CURRENT rows on every shift-click, and is
// simply dropped if it is no longer among them. Slint's selection.rs made the
// same call for the same reason.
//
// ── WHY IT RETURNS A MAP INSTEAD OF OWNING ONE ────────────────────────────
// Every host already owns its `selected` map and a dozen call sites read it.
// Taking that ownership away would mean rewriting all of them to find out
// whether the rule is right; handing back the NEXT map keeps each host's
// plumbing untouched and puts only the rule in here. The anchor is the one
// piece of state worth holding, because it is the one nobody had.
//
// A NEW object is always returned: mutating a `var` map in place notifies
// nothing, and every binding on it would go stale.

import QtQuick

QtObject {
    id: root

    /// The row a shift-range measures from. "" = no anchor yet. Hosts clear
    /// it when they leave select mode.
    property string anchorId: ""

    function _indexOf(rows, id) {
        for (var i = 0; i < rows.length; i++)
            if (rows[i] && rows[i].id === id)
                return i
        return -1
    }

    /// The selection map after clicking `id`, given the ordered `rows` the
    /// user is looking at and the `modifiers` off the mouse event.
    ///
    /// Pass the FILTERED rows when a view is filtered: a range over rows the
    /// user cannot see is not a range they asked for.
    function next(current, id, rows, modifiers) {
        var shift = (modifiers & Qt.ShiftModifier) !== 0
        var anchorAt = root.anchorId !== "" ? root._indexOf(rows, root.anchorId) : -1

        if (shift && anchorAt >= 0) {
            var here = root._indexOf(rows, id)
            if (here >= 0) {
                var lo = Math.min(anchorAt, here)
                var hi = Math.max(anchorAt, here)
                var m = Object.assign({}, current)
                for (var i = lo; i <= hi; i++)
                    m[rows[i].id] = true
                // The anchor STAYS, so the same range can be re-dragged.
                return m
            }
        }

        var out = Object.assign({}, current)
        if (out[id] === true) delete out[id]
        else out[id] = true
        root.anchorId = id
        return out
    }
}
