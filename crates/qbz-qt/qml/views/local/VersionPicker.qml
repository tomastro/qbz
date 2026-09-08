// Local album version picker (album/LocalAlbumView.slint:41 VersionPicker) —
// shown only when an album has several PHYSICAL copies (distinct source
// folders), so two copies never merge into a duplicated track list.
//
// Sized like QbzSelect `sm` (30px, radius 6, 10px left pad) so it reads as
// one of the small selects rather than towering over them: source icon +
// label + chevron, with a 280px dropdown that repeats the icon and ticks the
// current entry.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Rectangle {
    property bool kioskHost: false
    id: root

    /// [{ version, trackCount, quality, source }]
    property var versions: []
    property int current: 0
    /// Details headers have one 28px trailing origin/version column. In this
    /// arm the current source is the button and the full description stays in
    /// the flyout; the routed AlbumView keeps the labelled select.
    property bool compact: false
    /// Light-on-dark palette when mounted over an artwork atmosphere. The
    /// flyout remains a normal themed surface; only its launcher is over art.
    property bool overlay: false
    signal picked(int index)

    QbzTheme { id: theme }

    readonly property var currentVersion: versions[current] || ({})

    function optionText(version) {
        var parts = []
        if ((version.version || "") !== "")
            parts.push(version.version)
        parts.push((version.trackCount || 0) + " "
                   + QbzSession.tr("tracks", QbzSession.trRev))
        if ((version.quality || "") !== "")
            parts.push(version.quality)
        return parts.join(" · ")
    }

    height: root.kioskHost ? 44 : 30
    width: compact ? 28 : row.width
    radius: 6
    color: root.overlay
        ? (pickArea.containsMouse && versions.length > 1
            ? "#3dffffff" : (compact ? "transparent" : "#24ffffff"))
        : (pickArea.containsMouse && versions.length > 1
            ? theme.surfaceHover : (compact ? "transparent" : theme.surfaceElevated))

    Row {
        id: row
        visible: !root.compact
        height: parent.height
        leftPadding: 10
        rightPadding: 8
        spacing: 7
        SourceIcon {
            anchors.verticalCenter: parent.verticalCenter
            kind: root.currentVersion.source || "local"
            // A dense ROW: the media marks draw monochrome and tinted, like the
            // hard-drive beside them. Colour logos are for cards — a list of
            // them fights the text it labels.
            mono: true
            localTint: root.overlay ? "white" : "secondary"
            // No size overrides on purpose: LocalAlbumView.slint:24-26 draws
            // ALL THREE kinds at a flat 14px here, which is SourceIcon's
            // default (the row glyphs are the ones that grow the marks).
        }
        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: root.optionText(root.currentVersion)
            color: root.overlay ? "#e0ffffff" : theme.textSecondary
            font.pixelSize: theme.fontLegal
        }
        QbzIcon {
            anchors.verticalCenter: parent.verticalCenter
            name: "chevron-down"
            width: 12
            height: 12
            tintName: root.overlay ? "white" : "muted"
        }
    }
    Item {
        visible: root.compact
        anchors.fill: parent
        SourceIcon {
            anchors.centerIn: parent
            anchors.horizontalCenterOffset: root.versions.length > 1 ? -2 : 0
            kind: root.currentVersion.source || "local"
            mono: true
            glyphSize: 15
            plexSize: 16
            qobuzSize: 16
            localTint: root.overlay ? "white" : "muted"
        }
        QbzIcon {
            visible: root.versions.length > 1
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.rightMargin: 1
            anchors.bottomMargin: 2
            name: "chevron-down"
            width: 8
            height: 8
            tintName: "accent"
        }
    }
    MouseArea {
        id: pickArea
        anchors.fill: parent
        enabled: root.versions.length > 1
        hoverEnabled: true
        cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: {
            versionMenuLoader.active = true
            versionMenuLoader.item.openBelowRight(pickArea)
        }
    }

    // The popup and every option delegate are cold until the user asks for
    // them. Genres Details mounts a picker per visible album, while AlbumView
    // mounts one; neither should pay for an invisible Controls.Popup tree.
    Loader {
        id: versionMenuLoader
        active: false
        sourceComponent: QbzContextMenu { kioskHost: root.kioskHost;
            id: versionMenu
            menuWidth: 380
            Repeater {
                model: root.versions
                delegate: Rectangle {
                    id: opt
                    required property var modelData
                    required property int index
                    width: parent ? parent.width : 0
                    height: root.kioskHost ? 44 : 30
                    radius: 6
                    color: index === root.current ? theme.surfaceElevated
                         : optArea.containsMouse ? theme.surfaceHover : "transparent"
                    Row {
                        anchors.fill: parent
                        anchors.leftMargin: 8
                        anchors.rightMargin: 8
                        spacing: 8
                        SourceIcon {
                            anchors.verticalCenter: parent.verticalCenter
                            kind: opt.modelData.source || "local"
                            mono: true
                        }
                        Text {
                            width: parent.width - 22 - (opt.index === root.current ? 20 : 0)
                            height: parent.height
                            text: root.optionText(opt.modelData)
                            color: theme.textPrimary
                            font.pixelSize: theme.fontLegal
                            verticalAlignment: Text.AlignVCenter
                            elide: Text.ElideRight
                        }
                        QbzIcon {
                            visible: opt.index === root.current
                            anchors.verticalCenter: parent.verticalCenter
                            name: "check"
                            width: 12
                            height: 12
                            tintName: "accent"
                        }
                    }
                    MouseArea {
                        id: optArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            versionMenu.close()
                            root.picked(opt.index)
                        }
                    }
                }
            }
        }
    }
}
