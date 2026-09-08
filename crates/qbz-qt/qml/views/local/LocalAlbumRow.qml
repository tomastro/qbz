// Compact album row for the LIST arm of the Albums / Folders-flat / Artists
// surfaces (AlbumCollectionView's list mode, mounted by
// LocalLibraryView.slint:1255 with show-source / show-source-badge on).
//
// 56px: cover, title + artist, year, track count, quality mark, the source
// column and the trailing ⋯ overflow. Multi-select puts the shared checkbox
// in front of the cover, as the collection view does.
//
// The ⋯ menu carries the same Local Library favorite as the grid overlay.
// Genuine local and configured media-server albums are snapshot-backed;
// Qobuz-offline-only rows stay in the catalog-favorite domain.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../rows"
import "../../theme"

Rectangle {
    id: root

    property var item: ({})
    property string artSource: ""
    property bool isFavorite: root.item.isFavorite === true
    /// The LocalLibraryView root — the shared skeleton pulse, the per-item
    /// artwork gate and the settle bound all live there (never duplicated
    /// per row, so the rule cannot drift between surfaces).
    property var view: null
    property bool showSource: true
    property bool selectMode: false
    property bool checked: false
    /// Details surfaces keep albums open by default but must not replace the
    /// album row with bespoke chrome. The optional caret is the one extra
    /// affordance; clicking the rest of the row still opens AlbumView.
    property bool expandable: false
    property bool expanded: true
    /// Genres Details uses the same album semantics/menu but a taller table
    /// header whose trailing columns line up with LocalTrackRow.
    property bool detailsMode: false
    property var versions: []
    property int versionIndex: 0
    signal opened()
    signal playRequested()
    signal shuffleRequested()
    signal enqueueRequested(string mode)
    signal favoriteRequested()
    signal toggleExpanded()
    signal versionPicked(int index)
    /// `modifiers` rides straight off the mouse event: Shift is what turns
    /// a click into a range (controls/SelectionModel.qml).
    signal toggleSelect(int modifiers)

    QbzTheme { id: theme }
    TrackCols { id: cols }

    height: detailsMode ? 72 : 56
    radius: 6
    color: rowArea.containsMouse ? theme.surfaceHover : "transparent"

    function openRowMenuAt(anchor, x, y) {
        rowMenuLoader.active = true
        rowMenuLoader.item.openAtCursor(anchor, x, y)
    }
    function openRowMenuBelow(anchor) {
        rowMenuLoader.active = true
        rowMenuLoader.item.openBelowRight(anchor)
    }

    MouseArea {
        id: rowArea
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: Qt.PointingHandCursor
        onClicked: function (mouse) {
            if (mouse.button === Qt.RightButton) {
                root.openRowMenuAt(rowArea, mouse.x, mouse.y)
                return
            }
            if (root.selectMode) root.toggleSelect(mouse.modifiers)
            else root.opened()
        }
        onDoubleClicked: if (!root.selectMode) root.playRequested()
    }

    // Same cold-menu contract as LocalTrackRow: a list/detail viewport should
    // construct album rows, not five invisible popup delegates per row.
    Loader {
        id: rowMenuLoader
        active: false
        sourceComponent: CardMenu {
            menuWidth: 196
            entries: {
                var rows = [
                    { "label": QbzSession.tr("Open album", QbzSession.trRev), "icon": "library-big", "action": "open" },
                    { "label": QbzSession.tr("Play", QbzSession.trRev), "icon": "play-fill", "action": "play" },
                    { "label": QbzSession.tr("Play next", QbzSession.trRev), "icon": "list-start", "action": "next" },
                    { "label": QbzSession.tr("Play later", QbzSession.trRev), "icon": "list-plus", "action": "later" },
                    { "label": QbzSession.tr("Add to queue", QbzSession.trRev), "icon": "list-end", "action": "queue" },
                    { "label": QbzSession.tr("Add to playlist", QbzSession.trRev), "icon": "list-music", "action": "add-to-playlist" },
                    { "label": QbzSession.tr("Album info", QbzSession.trRev), "icon": "info", "action": "album-info" }
                ]
                if (root.item.favoriteable === true) {
                    rows.push({
                        "label": root.isFavorite
                            ? QbzSession.tr("Remove from Library", QbzSession.trRev)
                            : QbzSession.tr("Add to Library", QbzSession.trRev),
                        "icon": root.isFavorite ? "heart-filled" : "heart",
                        "action": "favorite"
                    })
                }
                return rows
            }
            onPicked: function (a) {
                if (a === "open") root.opened()
                else if (a === "play") root.playRequested()
                else if (a === "favorite") root.favoriteRequested()
                else if (a === "album-info" || a === "add-to-playlist")
                    // Both resolve the group key through the same bulk
                    // "album" scope (local_bulk.rs resolve_blocking).
                    QbzLocal.bulkAction("album", JSON.stringify([String(root.item.id)]), a)
                else root.enqueueRequested(a)
            }
        }
    }

    Row {
        visible: !root.detailsMode
        anchors.fill: parent
        anchors.leftMargin: 10
        anchors.rightMargin: 12
        spacing: 12

        Item {
            visible: root.selectMode
            width: visible ? 16 : 0
            height: parent.height
            SelectCheck {
                anchors.centerIn: parent
                on: root.checked
                onToggled: function (mods) { root.toggleSelect(mods) }
            }
        }
        Rectangle {
            width: 40
            height: 40
            anchors.verticalCenter: parent.verticalCenter
            radius: 4
            color: theme.surfaceElevated
            clip: true
            RoundedImage {
                id: rowArt
                anchors.fill: parent
                source: root.artSource
                radius: 4
            }
            // Per-item: hands over the instant the art is actually ON the
            // canvas (`rowArt.ready`, not "the path landed"); settles out
            // when the album simply has none (local artwork drops such keys).
            QbzSkeleton {
                variant: "art"
                anchors.fill: parent
                blockRadius: 4
                pending: root.view ? root.view.artWanted(root.item.artKey) : false
                coverReady: rowArt.ready
                phase: root.view ? root.view.skelPhase : false
                settleMs: root.view ? root.view.artSettleMs : 0
                settleHold: root.view ? root.view.artPulse : false
            }
        }
        Column {
            width: parent.width - 40 - 70 - 90 - 92 - 32
                - (root.showSource ? 34 : 0)
                - (root.selectMode ? 28 : 0) - 6 * 12
                - (root.expandable ? 28 + 12 : 0)
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            Text {
                width: parent.width
                text: root.item.title || ""
                color: theme.textPrimary
                font.pixelSize: 14
                font.weight: theme.weightMedium
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: root.item.artist || ""
                color: theme.textMuted
                font.pixelSize: 12
                elide: Text.ElideRight
            }
        }
        Text {
            width: 70
            anchors.verticalCenter: parent.verticalCenter
            text: root.item.year || ""
            color: theme.textMuted
            font.pixelSize: 12
        }
        Text {
            width: 90
            anchors.verticalCenter: parent.verticalCenter
            text: (root.item.trackCount || 0) + " "
                + QbzSession.tr("tracks", QbzSession.trRev)
            color: theme.textMuted
            font.pixelSize: 12
        }
        Item {
            width: 92
            height: parent.height
            QualityMini {
                tier: root.item.qualityTier || ""
                anchors.verticalCenter: parent.verticalCenter
            }
        }
        Item {
            visible: root.showSource
            readonly property var sourceValues: root.item.sources
                && root.item.sources.length > 0
                ? root.item.sources
                : ((root.item.sourceRaw || root.item.source || "") !== ""
                   ? [root.item.sourceRaw || root.item.source] : [])
            width: visible ? Math.max(34, sourceIcons.implicitWidth) : 0
            height: parent.height
            // AlbumListRow.slint:308-322 — through controls/SourceIcon.qml,
            // never QbzIcon: the Plex and Qobuz marks are MULTI-COLOUR and a
            // tint flattens them to a silhouette. This row used to draw an
            // accent-tinted `hard-drive` for Plex (a blue hard drive) and
            // `cloud-download` for an offline copy.
            Row {
                id: sourceIcons
                spacing: 3
                anchors.verticalCenter: parent.verticalCenter
                Repeater {
                    model: sourceIcons.parent.sourceValues
                    delegate: SourceIcon {
                        required property string modelData
                        kind: modelData
                        mono: true
                        glyphSize: 15
                        plexSize: 16
                        qobuzSize: 16
                        localTint: "muted"
                    }
                }
            }
        }
        Rectangle {
            visible: root.expandable
            width: visible ? 28 : 0
            height: 28
            radius: 6
            anchors.verticalCenter: parent.verticalCenter
            color: expandArea.containsMouse ? theme.surfaceElevated : "transparent"
            QbzIcon {
                name: root.expanded ? "chevron-down" : "chevron-right"
                width: 14
                height: 14
                anchors.centerIn: parent
                tintName: expandArea.containsMouse ? "textPrimary" : "muted"
            }
            MouseArea {
                id: expandArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: root.toggleExpanded()
            }
        }
        // Trailing ⋯ overflow (AlbumListRow.slint:360).
        Rectangle {
            width: 32
            height: 32
            radius: 6
            anchors.verticalCenter: parent.verticalCenter
            color: moreArea.containsMouse ? theme.surfaceElevated : "transparent"
            QbzIcon {
                name: "ellipsis"
                width: 18
                height: 18
                anchors.centerIn: parent
                tintName: moreArea.containsMouse ? "textPrimary" : "muted"
            }
            MouseArea {
                id: moreArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: function (mouse) {
                    root.openRowMenuAt(moreArea, mouse.x, mouse.y)
                }
            }
        }
    }

    // Genres Details header. LocalTrackRow holds back 26px for its source
    // indicator and 46px for its local menu; the formulas below use those
    // same gutters so quality, menu and version/source share exact centres.
    Item {
        id: detailsLayout
        visible: root.detailsMode
        anchors.fill: parent

        readonly property int qualityX: width - 26 - (cols.colMenu + cols.gap)
            - cols.padH - cols.colQuality
        readonly property int menuX: width - 70

        QbzIconButton {
            id: detailsFold
            visible: root.expandable
            x: 8
            anchors.verticalCenter: parent.verticalCenter
            btnSize: 26
            iconSize: 12
            name: root.expanded ? "minus" : "plus"
            onClicked: root.toggleExpanded()
        }

        Rectangle {
            x: 46
            width: 48
            height: 48
            anchors.verticalCenter: parent.verticalCenter
            radius: 5
            color: theme.surfaceElevated
            clip: true
            RoundedImage {
                id: detailsArt
                anchors.fill: parent
                source: root.artSource
                radius: 5
            }
            QbzSkeleton {
                variant: "art"
                anchors.fill: parent
                blockRadius: 5
                pending: root.view ? root.view.artWanted(root.item.artKey) : false
                coverReady: detailsArt.ready
                phase: root.view ? root.view.skelPhase : false
                settleMs: root.view ? root.view.artSettleMs : 0
                settleHold: root.view ? root.view.artPulse : false
            }
        }

        Column {
            x: 106
            y: 9
            width: Math.max(0, albumActions.x - x - 14)
            spacing: 1
            Text {
                width: parent.width
                text: root.item.title || ""
                color: theme.textPrimary
                font.pixelSize: 14
                font.weight: theme.weightMedium
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: root.item.artist || ""
                color: theme.textSecondary
                font.pixelSize: 12
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: {
                    var parts = []
                    if ((root.item.year || "") !== "") parts.push(root.item.year)
                    parts.push((root.item.trackCount || 0) + " "
                        + QbzSession.tr("tracks", QbzSession.trRev))
                    return parts.join(" · ")
                }
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
                elide: Text.ElideRight
            }
        }

        // Compact and deliberately not centred: it floats at the right edge
        // of the metadata region, before the table's quality/actions columns.
        Row {
            id: albumActions
            x: detailsLayout.qualityX - width - 16
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6
            QbzIconButton {
                btnSize: 26
                iconSize: 13
                name: "play-fill"
                onClicked: root.playRequested()
            }
            QbzIconButton {
                btnSize: 26
                iconSize: 13
                name: "shuffle"
                onClicked: root.shuffleRequested()
            }
        }

        Item {
            x: detailsLayout.qualityX
            width: cols.colQuality
            height: parent.height
            QualityMini {
                tier: root.item.qualityTier || ""
                anchors.centerIn: parent
            }
        }

        QbzIconButton {
            id: detailsMenuButton
            x: detailsLayout.menuX
            anchors.verticalCenter: parent.verticalCenter
            btnSize: cols.colMenu
            iconSize: 16
            name: "ellipsis"
            onClicked: root.openRowMenuBelow(detailsMenuButton)
        }

        VersionPicker {
            visible: root.showSource && root.versions.length > 0
            x: parent.width - width
            anchors.verticalCenter: parent.verticalCenter
            compact: true
            versions: root.versions
            current: root.versionIndex
            onPicked: function (index) { root.versionPicked(index) }
        }
    }
}
