// Local Library search input — a NAMING wrapper over the shared
// controls/QbzLineEdit.qml search arm, kept only so the six local call sites
// read as one thing and tune in one place.
//
// Slint has three near-identical inputs on this view (LocalLibraryView.slint:
// the toolbar `ExpandableSearch { sm: true }` at :782, the tree-rail header
// input at :1724 and the folder-detail input at :1984). All three are the
// EXPANDABLE form here: closed they are a 30px magnifier slot; clicking opens
// a `boxWidth` field that grows LEFT over its neighbours, and the X clears and
// closes it again (primitives/ExpandableSearch.slint).

import QtQuick
import "../../controls"

QbzLineEdit {
    id: root

    /// Width of the OPEN field (the closed footprint is always 30px).
    property int boxWidth: 200

    searchMode: true
    expandable: true
    sm: true
    // surface-card — ExpandableSearch.slint's own overlay fill (the port's
    // toolbar copy used surface-elevated; the reference does not).
    elevated: false
    openWidth: boxWidth
}
