// Horizontal album rail (Carousel.slint) — extracted for reuse by the
// detail views (AlbumView / ArtistView): section header + page chevrons +
// clipped ListView of the shared AlbumCard, with soft edge fades.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../cards"
import "../theme"

Column {
    id: root
    property string title: ""
    property var items: []
    // The host's url-keyed cover map ({url: file://path}).
    property var coverMap: ({})
    /// Carousel.slint's `show-view-all` (default false, as there). The ONE
    /// mount that turns it on today is AlbumView's "From the same artist" rail
    /// — album/AlbumPageView.slint:1124-1132 — whose link opens the artist's
    /// discography. Every other SectionRail in the tree is untouched: the
    /// property defaults off and QbzSectionHeader draws nothing for it.
    property bool showViewAll: false
    signal viewAllClicked()

    QbzTheme { id: theme }
    width: parent ? parent.width : 0
    spacing: 12

    readonly property int perPage: Math.max(1, Math.floor((rail.width + 32) / 232))
    readonly property int step: perPage * 232
    readonly property real maxScroll: Math.max(0, rail.contentWidth - rail.width)



    QbzSectionHeader {
        title: root.title
        // The shared header already implements both halves
        // (QbzSectionHeader.qml:42-54, :107-131) — this only forwards them.
        showViewAll: root.showViewAll
        onViewAllClicked: root.viewAllClicked()
        leftEnabled: rail.contentX > 1
        rightEnabled: rail.contentX < root.maxScroll - 1
        onPageLeft: rail.contentX = Math.max(0, rail.contentX - root.step)
        onPageRight: rail.contentX = Math.min(root.maxScroll, rail.contentX + root.step)
    }

    Item {
        width: parent.width
        height: 246
        ListView {
            id: rail
            anchors.fill: parent
            orientation: ListView.Horizontal
            spacing: 32
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: root.items
            delegate: AlbumCard {
                albumId: modelData.id
                source: modelData.source || ""
                sources: modelData.sources || []
                title: modelData.title
                artist: modelData.artist
                artistId: modelData.artistId
                genre: modelData.genre
                year: modelData.year
                qualityTier: modelData.qualityTier
                qualityDetail: modelData.qualityDetail || ""
                artSource: root.coverMap[modelData.artUrl] || ""
                // The row carries the heart (home_qt / recommendations_qt
                // stamp it from fav_cache at build time); hardcoding false
                // made a library album draw hollow and read "Add to
                // Library", so the first click REMOVED it.
                isFavorite: modelData.isFavorite === true
                // Pin glyph: the row carries the state, and the pin payload
                // needs the REMOTE url (coverMap holds local file:// paths,
                // which are worthless as a stored display snapshot).
                isPinned: modelData.isPinned === true
                artworkUrl: modelData.artUrl || ""
            }
        }
        // Edge fades — content dissolves into the page background at the
        // scrolled edges instead of a hard cut. They fade to the OPAQUE
        // surface-main, so under the app-wide dynamic background they would
        // paint two dark 56px slabs over the moving field: the reference hides
        // them outright for exactly that reason (Carousel.slint:304 and :312,
        // `visible: !ShellState.app-background-active`). This is what put the
        // black band down the right edge of "From the same artist" in the
        // owner's 2026-08-04 capture, where the Slint shows the field through
        // the half-scrolled card.
        Rectangle {
            anchors.left: parent.left
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            width: 56
            visible: !theme.ambientOn
            opacity: rail.contentX > 1 ? 1.0 : 0.0
            Behavior on opacity { NumberAnimation { duration: 150 } }
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: theme.surfaceMain }
                GradientStop { position: 1.0; color: "transparent" }
            }
        }
        Rectangle {
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            width: 56
            visible: !theme.ambientOn
            opacity: rail.contentX < root.maxScroll - 1 ? 1.0 : 0.0
            Behavior on opacity { NumberAnimation { duration: 150 } }
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: "transparent" }
                GradientStop { position: 1.0; color: theme.surfaceMain }
            }
        }
    }
}
