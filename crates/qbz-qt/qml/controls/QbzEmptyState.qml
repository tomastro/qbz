// QbzEmptyState — the plain centered empty state (title + optional icon /
// body / action), consolidated in phase 22 from HomeView.RecentPlaceholder
// and QueuePanel's empty queue.
//
// NOT used by SearchView or Cortinilla, contrary to what this header used to
// claim: both draw their own no-results line inline, because the cortinilla's
// is a single 28px row inside a dropdown rather than a centred block. Slint
// has no shared EmptyState either (per-surface inline).
//
// Arms: `iconName` ("" = none), `title`,
// `body` ("" = hidden), `actionLabel` + `actionClicked()` ("" = no action).

import QtQuick
import com.blitzfc.qbz
import "../theme"

Column {
    id: emptyRoot
    property string iconName: ""
    property string title: ""
    property string body: ""
    property string actionLabel: ""
    // Per-surface metrics. The consolidated default is the phase-22 one (40px
    // glyph, full opacity, primary semibold title); the Blacklist manager's
    // three empty branches are a 48px glyph at 0.3 with a MUTED regular title
    // (BlacklistManagerView.slint:702-732, :801-831, :898-928), which is a
    // call-site number, not a second component.
    property int iconSize: 40
    property real iconOpacity: 1.0
    property bool titleMuted: false
    property int titleWeight: theme.weightSemibold
    signal actionClicked()

    QbzTheme { id: theme }

    spacing: 0
    QbzIcon {
        visible: emptyRoot.iconName !== ""
        name: emptyRoot.iconName
        width: emptyRoot.iconSize
        height: emptyRoot.iconSize
        opacity: emptyRoot.iconOpacity
        anchors.horizontalCenter: parent.horizontalCenter
        tintName: "muted"
    }
    Item { visible: iconName !== ""; width: 1; height: 14 }
    Text {
        anchors.horizontalCenter: parent.horizontalCenter
        text: emptyRoot.title
        color: emptyRoot.titleMuted ? theme.textMuted : theme.textPrimary
        font.pixelSize: theme.fontSection
        font.weight: emptyRoot.titleWeight
    }
    Item { visible: body !== ""; width: 1; height: 6 }
    Text {
        visible: body !== ""
        anchors.horizontalCenter: parent.horizontalCenter
        text: body
        color: theme.textMuted
        font.pixelSize: 13
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.WordWrap
    }
    Item { visible: actionLabel !== ""; width: 1; height: 14 }
    SettingsButton {
        visible: actionLabel !== ""
        anchors.horizontalCenter: parent.horizontalCenter
        text: actionLabel
        onClicked: parent.actionClicked()
    }
}
