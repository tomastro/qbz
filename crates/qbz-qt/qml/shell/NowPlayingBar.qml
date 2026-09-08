// NowPlayingBar SHELL — the mode seam (NowPlayingBar.slint, phase 18):
// mounts NowPlayingBarSmall for mode 2 (Small) and the full PlayerBar for
// modes 0 (New) / 1 (Classic) / 3 (Large). AppShell pins the height
// mode-aware (42px Small / 112px otherwise).

import QtQuick
import com.blitzfc.qbz

Item {
    id: root

    // The shell's shared hover-tooltip overlay (controls/QbzTooltip.qml),
    // fed in by AppShell (same pattern as Sidebar.tooltip). Forwarded into
    // whichever mode is loaded below: all four modes use it for dynamic
    // Shuffle/Repeat state, and the full bar also uses it for Qobuz Connect.
    property Item tooltip: null

    Loader {
        id: barLoader
        anchors.fill: parent
        source: QbzShell.npbMode === 2 ? "NowPlayingBarSmall.qml" : "PlayerBar.qml"
    }

    // Same shape as AppShell's viewLoader "kind" Binding: applies the moment
    // the item exists, re-applies on a mode switch, RestoreNone because the
    // target is DESTROYED on unload. Both bar implementations declare the
    // property, so a mode switch keeps the same overlay host.
    Binding {
        target: barLoader.item
        property: "tooltip"
        value: root.tooltip
        when: barLoader.item !== null
        restoreMode: Binding.RestoreNone
    }
}
