// Horizontal PLAYLIST rail (discover/PlaylistCarousel.slint) — section
// header + page chevrons + clipped ListView of the shared PlaylistCard,
// with soft edge fades.
//
// This is SectionRail.qml with a different delegate, and that is deliberate:
// the reference keeps `Carousel.slint` and `PlaylistCarousel.slint` apart for
// the same reason. SectionRail's delegate is a fully-bound AlbumCard mounted
// three times by AlbumView; adding a `kind`/`delegateComponent` switch there
// would put a branch in the hot path of every album rail to serve one caller.
//
// Body click OPENS the playlist — the reference mounts this carousel with
// `body-opens: true` on the artist page (ArtistPageView.slint:985-995), which
// is also PlaylistCard.qml's built-in behaviour, so nothing is passed for it.
//
// The card's own hover Behaviors (opacity, 150ms) and the edge fades' are
// finite hover transitions, identical to the ones SectionRail already runs on
// the album pages — no steady-state presents/s is added, and there is no
// timer, no data-fed Behavior and no continuous animation anywhere here.

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

    QbzTheme { id: theme }
    width: parent ? parent.width : 0
    spacing: 12

    // 200px card + 32px gap = the 232px pitch, the same maths SectionRail
    // runs and the same geometry PlaylistCarousel.slint:75-78 declares.
    readonly property int perPage: Math.max(1, Math.floor((rail.width + 32) / 232))
    readonly property int step: perPage * 232
    readonly property real maxScroll: Math.max(0, rail.contentWidth - rail.width)

    QbzSectionHeader {
        title: root.title
        // PlaylistCarousel's `show-view-all` is off on the artist page (the
        // .slint passes no `view-all-*` there), so this rail has no link arm
        // at all — the header simply draws none.
        showViewAll: false
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
            delegate: PlaylistCard {
                item: modelData
                artSource: root.coverMap[modelData.artUrl] || ""
                // The row carries the pin state (the producer stamps it from
                // the per-user store); hardcoding false made a pinned
                // playlist draw hollow, so the first click UN-pinned it.
                // `artworkUrl` needs no hand-over: the card defaults it off
                // `item.artUrl`, which is the REMOTE url the pin payload
                // wants — coverMap holds file:// paths, worthless as a
                // stored display snapshot.
                isPinned: modelData.isPinned === true
            }
        }
        // Edge fades — see SectionRail.qml for why they hide under the
        // app-wide dynamic background (Carousel.slint / PlaylistCarousel
        // .slint:163,170, `visible: !ShellState.app-background-active`).
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
