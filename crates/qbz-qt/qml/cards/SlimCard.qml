// SlimCard — THE slim row card (discover/SlimCard.slint — a DISTINCT
// component in Slint, NOT an AlbumCard variant: 60px row, optional rank +
// 44px thumb + title/subtitle, single whole-row click, no overlay). The
// Slint mounts it via SlimCarousel (Home recent/popular grids, ForYou,
// ExternalReco, offline rail); the POC has it on the Home popular grid
// (SlimGrid). Promoted from HomeView in phase 21.
//
// card contract: { id, title, artist, rank, artPath } — slim rows are
// albums in the POC: click opens the album view (phase 8).

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    property var card: ({})

    QbzTheme { id: theme }

    height: 60
    radius: theme.radiusSm
    color: slArea.containsMouse ? theme.surfaceHover : "transparent"

    Row {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        spacing: 12
        Text {
            visible: card.rank !== ""
            width: 20
            anchors.verticalCenter: parent.verticalCenter
            text: card.rank
            color: theme.textMuted
            font.pixelSize: theme.fontBody
            horizontalAlignment: Text.AlignHCenter
        }
        Rectangle {
            width: 44
            height: 44
            radius: 4
            anchors.verticalCenter: parent.verticalCenter
            color: theme.surfaceElevated
            // No clip: the child RoundedImage confines itself on both arms, and
            // a rectangular scissor never produced this radius. A clip is an
            // unconditional batch root, so this one cost a draw call per item.
            RoundedImage {
                anchors.fill: parent
                source: card.artPath
                radius: 4
            }
        }
        Column {
            width: parent.width - (card.rank !== "" ? 20 : 0) - 44 - 2 * 12
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            Text {
                width: parent.width
                text: card.title
                color: theme.textPrimary
                font.pixelSize: theme.fontLink
                font.weight: theme.weightMedium
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: card.artist
                color: theme.textMuted
                font.pixelSize: 12
                elide: Text.ElideRight
            }
        }
    }
    MouseArea {
        id: slArea
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        // Phase 8: slim rows are albums — click opens the album view.
        onClicked: QbzAlbum.openAlbum(card.id)
    }
}
