// Applied-filters tooltip trigger — the two-line wiring a filter control mounts
// beside itself so hovering it says WHAT is currently filtered, not just that
// something is.
//
// Non-visual. It writes the shell's `filterTipJson` channel
// (`shell_bridge.rs`), which `AppShell.qml` feeds to the one `QbzTooltip`
// overlay. That indirection is not ceremony: a funnel lives five levels inside
// a view's toolbar and the overlay is a sibling of the whole shell, so a QML
// `id` cannot reach it, and the art preview solved the identical problem the
// identical way.
//
// THE HOST BUILDS `groups`, and there is no way around it: of the app's filter
// surfaces roughly a third keep their state in QML properties and the rest in
// Rust documents, so nothing here could read one source and be right. What this
// does own is the CONTRACT — `[{ group: "<label>", values: ["…"] }]`, groups
// with no values dropped, and an empty result meaning "show nothing", which
// lets a control keep its ordinary one-line tooltip for the unfiltered case.
//
// Usage:
//
//     QbzFilterTip {
//         id: filterTip
//         ownerKey: "local-albums-filter"
//         anchor: funnelButton
//         groups: root.filterSummaryGroups
//     }
//     MouseArea {
//         hoverEnabled: true
//         onEntered: filterTip.enter()
//         onExited: filterTip.exit()
//     }

import QtQuick
import com.blitzfc.qbz

Item {
    id: root

    // Unique per surface. It is the race-safe owner id the overlay closes on,
    // so two funnels on one screen never blank each other's bubble.
    property string ownerKey: ""
    // The control the bubble points at.
    property Item anchor: null
    // [{ group: "Genre", values: ["Rock", "Jazz"] }, …]
    property var groups: []

    width: 0
    height: 0
    visible: false

    // True when there is actually something to say. Hosts read it to decide
    // whether to show their plain tooltip instead.
    readonly property bool hasSummary: {
        var gs = root.groups || []
        for (var i = 0; i < gs.length; i++) {
            var v = (gs[i] || {}).values || []
            if (v.length > 0)
                return true
        }
        return false
    }

    function enter() {
        if (!root.anchor || !root.hasSummary)
            return
        // Scene coordinates: the overlay fills the window, so its coordinate
        // space and the scene's are the same. Captured as NUMBERS — the anchor
        // may be destroyed (a rebuilt view, a recycled delegate) while the
        // bubble is still up.
        var p = root.anchor.mapToItem(null, 0, 0)
        QbzShell.filterTipJson = JSON.stringify({
            key: root.ownerKey,
            x: p.x,
            y: p.y,
            w: root.anchor.width,
            h: root.anchor.height,
            groups: root.groups
        })
    }

    function exit() {
        // Only clear if this control still owns the channel, so sliding from
        // one funnel straight onto another does not blank the new bubble.
        var d = {}
        try {
            d = JSON.parse(QbzShell.filterTipJson)
        } catch (e) {
            d = {}
        }
        if (d && d.key === root.ownerKey)
            QbzShell.filterTipJson = "{}"
    }

    // A view being torn down under an open bubble must not leave it floating.
    Component.onDestruction: root.exit()
}
