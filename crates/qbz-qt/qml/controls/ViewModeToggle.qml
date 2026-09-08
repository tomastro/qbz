// ViewModeToggle — discover/BrowseHeaderTools.slint:149-173, verbatim: the
// 68x34 grid/list pair, radius 6, surface-elevated, CLIPPED so the segments'
// own corners never poke out.
//
// NOT views/local/LocalSegToggle.qml — that one is 60x30 with 26x24 segments
// (LocalLibraryView.slint's own ViewToggle) and reusing it here would ship a
// browse toolbar 8px short of every other control on the row
// (ui-control-alignment-standard: 34px).
//
// `mode` is the caller's state ("grid" | "list"); the toggle only reports the
// change — the host owns persistence, exactly like the Slint two-way binding.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root

    property string mode: "grid"
    signal setMode(string value)

    QbzTheme { id: theme }

    width: 68
    height: 34
    radius: 6
    // The well: surface-elevated @ 0.5 under the dynamic background
    // (BrowseHeaderTools.slint:155).
    color: theme.ambientOn ? theme.surfaceElevatedA50 : theme.surfaceElevated
    clip: true

    Row {
        anchors.centerIn: parent
        spacing: 0
        QbzToggleButton {
            name: "layout-grid"
            active: root.mode !== "list"
            onClicked: root.setMode("grid")
        }
        QbzToggleButton {
            name: "list"
            active: root.mode === "list"
            onClicked: root.setMode("list")
        }
    }
}
