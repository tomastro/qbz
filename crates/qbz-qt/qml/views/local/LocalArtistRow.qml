// One row of the Artists master rail (LocalLibraryView.slint:609
// LocalArtistRow). 60px tall, radius 8, 48px ROUND avatar (cached photo or a
// user glyph), display name + "N albums · M tracks". Selected = accent fill
// with accent-text labels (that is the Slint, :615-660 — the accent fill is
// the master-list selection, not a pill).

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Rectangle {
    id: root

    property var item: ({})
    property bool selected: false
    property string artSource: ""
    /// The LocalLibraryView root (shared pulse + per-item artwork gate).
    property var view: null
    signal picked(string name)

    /// Does this artist expect an avatar at all? (Whether one has ARRIVED is
    /// the placeholder's business — it hands over on the paint, not the path.)
    readonly property bool avatarWanted: root.view
        ? root.view.artWanted(root.item.artKey) : false

    QbzTheme { id: theme }

    height: 60
    radius: 8
    color: selected ? theme.accent
         : rowArea.containsMouse ? theme.surfaceHover : "transparent"

    Row {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        spacing: 12
        Rectangle {
            width: 48
            height: 48
            radius: 24
            anchors.verticalCenter: parent.verticalCenter
            color: theme.surfaceElevated
            clip: true
            QbzIcon {
                // The designed empty state — shown exactly when there is no
                // art AND the placeholder has retired, so the user is never
                // left with a blank disc and never sees the glyph flash
                // underneath a cover that is about to paint.
                visible: root.artSource === "" && avatarSkel.retired
                name: "user"
                width: 22
                height: 22
                anchors.centerIn: parent
                tintName: "muted"
            }
            RoundedImage {
                id: avatarArt
                anchors.fill: parent
                source: root.artSource
                radius: 24
            }
            // Round per-item placeholder — the avatar shape, not a square.
            QbzSkeleton {
                id: avatarSkel
                variant: "circle"
                anchors.fill: parent
                pending: root.avatarWanted
                coverReady: avatarArt.ready
                phase: root.view ? root.view.skelPhase : false
                settleMs: root.view ? root.view.artSettleMs : 0
                settleHold: root.view ? root.view.artPulse : false
            }
        }
        Column {
            width: parent.width - 60
            anchors.verticalCenter: parent.verticalCenter
            spacing: 3
            Text {
                width: parent.width
                text: root.item.displayName || root.item.name || ""
                // Both labels sit on the selected row's `theme.accent` fill.
                // Same story as views/local/FilterChip.qml: NOT a departure
                // from locallibrary/LocalLibraryView.slint:653 + :660
                // (`Theme.accent-text`), which is what the twin returns on 34
                // of the 35 palettes — only a floor for rose-pine-dawn, where
                // accent-text measures 2.56:1 on this accent.
                color: root.selected ? theme.accentGlyphColor : theme.textPrimary
                font.pixelSize: theme.fontBody
                font.weight: root.selected ? theme.weightSemibold : theme.weightMedium
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: (root.item.albumCount || 0) + " "
                    + QbzSession.tr("albums", QbzSession.trRev) + " · "
                    + (root.item.trackCount || 0) + " "
                    + QbzSession.tr("tracks", QbzSession.trRev)
                color: root.selected ? theme.accentGlyphColor : theme.textMuted
                font.pixelSize: theme.fontLegal
                elide: Text.ElideRight
            }
        }
    }
    MouseArea {
        id: rowArea
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.picked(root.item.name)
    }
}
