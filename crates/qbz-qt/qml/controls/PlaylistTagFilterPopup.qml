// Qobuz-Playlists category filter — the dropdown of the Home rail's
// "Filter by category" trigger. Port of
// `crates/qbz-ui/ui/discover/PlaylistTagFilter.slint:120-183`.
//
// The filter is CLIENT-side over the 40 cards already in hand: the tags ship in
// the same /discover/index response as the playlists, so opening this costs no
// request. The selection is a UNION (a card passes if it carries ANY selected
// slug) and an empty selection shows everything.
//
// The selection lives in RUST (`home_qt::toggle_playlist_tag`), not in a
// property here or on HomeView: this view is destroyed on every navigation, so
// a QML-side selection would be forgotten the moment the user opened an album
// and came back. Slint keeps it in `TAB_SECTIONS` for the same reason and says
// so: "Client-side; survives a tab switch."
//
// ANCHORING. The trigger sits INSIDE the Home flickable, at a y that depends on
// the scroll position and on the rail order the Discover configurator dictates,
// so the fixed `anchorTop` of GenreFilterPopup is not available here. The
// position is captured by `mapToItem` AT OPEN and stored as NUMBERS — never a
// reference to the trigger — so a rebuilt rail cannot dangle under an open
// popup. Same rule, same reason, as controls/QbzTooltip.qml:127-133.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    // [{ slug, name }] — the offered set, from QbzHome.playlistTagsJson.
    property var tags: []
    // Selected slugs, from the same document.
    property var selected: []

    readonly property bool opened: root._open
    property bool _open: false
    property real _x: 0
    property real _y: 0

    signal toggled(string slug)
    signal cleared()

    QbzTheme { id: theme }

    // 220 = the trigger's width (PlaylistTagFilter.slint:69), so the card lines
    // up under it exactly.
    readonly property int cardW: 220
    readonly property int rowH: 32
    readonly property int footerH: 32
    readonly property int listMax: 320
    readonly property real listH: Math.min(root.tags.length * root.rowH, root.listMax)
    readonly property real cardH: root.listH + root.footerH + 10

    anchors.fill: parent
    visible: root._open
    z: 3000

    function openAt(item) {
        if (!item)
            return
        var p = item.mapToItem(root, 0, 0)
        // 4px of air under the trigger (PlaylistTagFilter.slint:122), clamped
        // so the card never leaves the window.
        root._x = Math.max(8, Math.min(root.width - root.cardW - 8, p.x))
        root._y = Math.max(8, Math.min(root.height - root.cardH - 8,
                                       p.y + item.height + 4))
        root._open = true
    }
    function close() {
        root._open = false
    }
    function toggle(item) {
        if (root._open)
            root.close()
        else
            root.openAt(item)
    }

    // Click-out backdrop. Declared FIRST so the card below covers it.
    MouseArea {
        anchors.fill: parent
        onClicked: root.close()
        // A MouseArea eats presses, NOT wheel events. Without this the page
        // behind an open popup kept scrolling under the pointer.
        onWheel: function (wheel) { wheel.accepted = true }
    }

    Rectangle {
        x: Math.round(root._x)
        y: Math.round(root._y)
        width: root.cardW
        height: root.cardH
        color: theme.surfaceMain
        radius: theme.radiusSm
        border.width: 1
        border.color: theme.borderMuted
        clip: true

        // Swallow clicks so the backdrop does not close the card, and the
        // wheel so a scroll over the footer does not reach the page.
        MouseArea {
            anchors.fill: parent
            onWheel: function (wheel) { wheel.accepted = true }
        }

        Flickable {
            id: listFlick
            x: 0
            y: 5
            width: parent.width
            height: root.listH
            contentWidth: width
            contentHeight: listCol.height
            boundsBehavior: Flickable.StopAtBounds
            clip: true

            Column {
                id: listCol
                width: listFlick.width

                Repeater {
                    model: root.tags
                    delegate: Rectangle {
                        id: tagRow
                        width: listCol.width
                        height: root.rowH
                        color: tagArea.containsMouse ? theme.surfaceHover : "transparent"

                        readonly property string slug: modelData.slug || ""
                        readonly property bool picked:
                            (root.selected || []).indexOf(tagRow.slug) >= 0

                        Row {
                            anchors.left: parent.left
                            anchors.leftMargin: 10
                            anchors.right: parent.right
                            anchors.rightMargin: 10
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 8

                            Rectangle {
                                width: 16
                                height: 16
                                anchors.verticalCenter: parent.verticalCenter
                                radius: 4
                                color: tagRow.picked ? theme.accent : "transparent"
                                border.width: tagRow.picked ? 0 : 1.5
                                border.color: theme.textMuted
                                QbzIcon {
                                    anchors.centerIn: parent
                                    visible: tagRow.picked
                                    name: "check"
                                    width: 11
                                    height: 11
                                    // The reference hardcodes #ffffff
                                    // (PlaylistTagFilter.slint:43). This port
                                    // does not: white on a pale accent measures
                                    // under 2.6:1 on 16 of the 35 palettes, so
                                    // every check in this build takes the
                                    // measured selector instead.
                                    tintName: theme.accentGlyphTint
                                }
                            }
                            Text {
                                width: parent.width - 24 - parent.spacing
                                anchors.verticalCenter: parent.verticalCenter
                                text: modelData.name || ""
                                color: theme.textPrimary
                                font.pixelSize: 12
                                font.weight: tagRow.picked ? theme.weightMedium
                                                           : theme.weightRegular
                                elide: Text.ElideRight
                            }
                        }

                        MouseArea {
                            id: tagArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.toggled(tagRow.slug)
                        }
                    }
                }
            }
        }

        // "All categories" — clears the selection. Dimmed and inert when there
        // is nothing to clear (PlaylistTagFilter.slint:159-180).
        Item {
            width: parent.width
            height: root.footerH
            y: root.listH + 5
            opacity: (root.selected || []).length > 0 ? 1.0 : 0.5

            Text {
                anchors.centerIn: parent
                text: QbzSession.tr("All categories", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: 12
            }
            MouseArea {
                anchors.fill: parent
                enabled: (root.selected || []).length > 0
                cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                onClicked: root.cleared()
            }
        }
    }
}
