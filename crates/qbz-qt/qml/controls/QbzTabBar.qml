// QbzTabBar — the segmented tab pill bar (primitives/SegmentedTabBar
// .slint), consolidated in phase 22 from HomeView's discover tab pill and
// LibraryView's SegTab row (+ the playlists sub-tab via `compact`).
// Arms: `tabs` ([{id, label, count?}]), `activeId`, `counts` (11px count
// chip), `underline` (2px accent active cue — the Slint redundant-shape
// cue), `compact` (24px mini variant). Emits selected(id); the host owns
// side effects (scroll resets, refetches).
// Deliberately NOT absorbed (Slint keeps them per-surface too): the
// SearchView underline tab strip, QueuePanel's text tabs, ArtistView's
// scroll-spy jump links.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root

    property var tabs: []
    property string activeId: ""
    property bool counts: false
    property bool underline: false
    property bool compact: false
    /// Explicit density opt-in for a Kiosk host; the DEFAULT is desktop and
    /// every metric below falls back to the exact number it had before this
    /// property existed. A tab is a primary destination, so the kiosk arm
    /// takes the contract's 64px primary target (§2.6) rather than the 29px
    /// the desktop padding produces.
    property bool kioskHost: false
    signal selected(string id)

    QbzTheme { id: theme }

    width: segBarRow.width
    height: segBarRow.height
    // The well: surface-elevated @ 0.5 under the dynamic background
    // (SegmentedTabBar.slint:108).
    color: theme.ambientOn ? theme.surfaceElevatedA50 : theme.surfaceElevated
    radius: 6

    Row {
        id: segBarRow
        padding: root.compact ? 2 : 3
        spacing: 4
        Repeater {
            model: root.tabs
            delegate: Rectangle {
                id: segRoot
                required property var modelData
                readonly property bool active: modelData.id === root.activeId
                readonly property int count: modelData.count || 0

                width: root.kioskHost
                    ? Math.max(96, segRow.implicitWidth) : segRow.implicitWidth
                height: root.kioskHost ? 64 : segRow.implicitHeight
                radius: 4
                // The ACTIVE cell takes surface-main @ 0.5 — surface-MAIN, not
                // elevated, and at the chrome alpha (SegmentedTabBar.slint:29).
                color: active
                     ? (theme.ambientOn ? theme.surfaceMainA50 : theme.surfaceMain)
                     : segArea.containsMouse ? theme.surfaceHover : "transparent"

                Row {
                    id: segRow
                    // Desktop keeps `x/y = 0` and sizes the cell from its own
                    // padding; the kiosk cell has an EXPLICIT 64px box, so the
                    // content is centred inside it instead.
                    anchors.horizontalCenter: root.kioskHost
                        ? segRoot.horizontalCenter : undefined
                    anchors.verticalCenter: root.kioskHost
                        ? segRoot.verticalCenter : undefined
                    leftPadding: root.kioskHost ? 18 : (root.compact ? 8 : 12)
                    rightPadding: (root.kioskHost ? 18 : (root.compact ? 8 : 12))
                        - (root.counts && segRoot.count > 0 ? 4 : 0)
                    topPadding: root.compact ? 4 : 6
                    bottomPadding: root.compact ? 4 : 6
                    spacing: 7
                    Text {
                        text: segRoot.modelData.label
                        color: segRoot.active ? theme.textPrimary : theme.textMuted
                        font.pixelSize: root.kioskHost ? theme.fontLegal * 1.2
                            : (root.compact ? 12 : theme.fontLegal)
                        font.weight: theme.weightMedium
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    Rectangle {
                        visible: root.counts && segRoot.count > 0
                        width: Math.max(18, countText.implicitWidth + 10)
                        height: 16
                        radius: 8
                        anchors.verticalCenter: parent.verticalCenter
                        color: segRoot.active ? "#26ffffff" : "#14ffffff"
                        Text {
                            id: countText
                            anchors.centerIn: parent
                            text: segRoot.count
                            color: segRoot.active ? theme.textPrimary : theme.textSecondary
                            font.pixelSize: 11
                            font.weight: theme.weightMedium
                        }
                    }
                }
                // Active underline (redundant shape cue, Slint Segment).
                Rectangle {
                    visible: root.underline && segRoot.active
                    x: 4
                    width: parent.width - 8
                    height: 2
                    y: parent.height - 2
                    radius: 1
                    color: theme.accent
                }
                MouseArea {
                    id: segArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.selected(segRoot.modelData.id)
                }
            }
        }
    }
}
