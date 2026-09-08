// QbzSectionHeader — the section/rail header (Slint Carousel header:
// title + [View all] LEFT of the page chevrons), consolidated in phase 22
// from HomeView.RailHeader (+ its ViewAllLink), SectionRail's header and
// SearchView.SectionHeader. Slint has NO shared SectionHeader primitive
// (each carousel inlines one) — this is a POC consolidation of the three
// identical-in-intent copies.
//
// Arms: `title` (fontSection semibold); `showViewAll` + `viewAllLabel` +
// `viewAllAccent` (Search's accent chip vs Home's secondary link) +
// viewAllClicked(); `showChevrons` + `leftEnabled`/`rightEnabled` +
// pageLeft()/pageRight() (the host owns the page-step math); and, since the
// Playlist Manager landed, `collapsible` + `collapsed` + toggled() and
// `titleSecondary`.
// Deliberately NOT absorbed: ArtistView.ReleaseSection's header (adds a sort
// dropdown and a "See discography" link — two arms this header does not have,
// and both are now LIVE controls rather than the stubs this note used to
// describe) and the uppercase 10-11px eyebrow mini-headers (different type
// scale).
//
// ── THE `id: root` REFACTOR CAME FIRST, AND IT WAS NOT COSMETIC ───────────
// This file used to have no root id: its Text read `text: parent.title` and
// three deeper bindings walked `parent.parent…`. Wrapping that Text in the
// Row the collapse chevron needs would have changed its `parent` and silently
// BLANKED the title on all thirteen existing instantiations
// (HomeView.qml:346,537,612,773,1096,1150 · SearchView.qml:526,555,584,690,790
// · SectionRail.qml:28 · LabelView.qml:139). Every property read now goes
// through `root`, so the tree can be restructured again without that trap.
//
// The collapse chevron sits IMMEDIATELY RIGHT OF THE TITLE (the Playlist
// Manager's `PmSectionHeader`, playlist/PlaylistManagerView.slint:1034-1058),
// not in the right-hand Row. `Row` skips invisible children, so with
// `collapsible: false` — the default — the thirteen existing sites are
// pixel-identical: no width, no spacing, nothing.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root
    property bool kioskHost: false

    property string title: ""
    property bool showViewAll: false
    property string viewAllLabel: ""
    property bool viewAllAccent: false
    property bool showChevrons: true
    property bool leftEnabled: false
    property bool rightEnabled: false
    /// Paint the title `text-secondary` instead of `text-primary` — the
    /// Playlist Manager's two section headers do
    /// (playlist/PlaylistManagerView.slint:1029).
    property bool titleSecondary: false
    /// Render the 22 × 22 collapse chevron right of the title.
    property bool collapsible: false
    property bool collapsed: false
    signal viewAllClicked()
    signal pageLeft()
    signal pageRight()
    signal toggled()

    QbzTheme { id: theme }

    width: parent ? parent.width : 0
    height: root.kioskHost ? 44 : 28

    Row {
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        spacing: 8

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: root.title
            color: root.titleSecondary ? theme.textSecondary : theme.textPrimary
            font.pixelSize: theme.fontSection
            font.weight: theme.weightSemibold
        }
        // The chevron's 22 × 22 PLATE lights on hover; the glyph itself stays
        // `text-muted` in every state (`:1040-1049`).
        Rectangle {
            visible: root.collapsible
            anchors.verticalCenter: parent.verticalCenter
            width: root.kioskHost ? 44 : 22
            height: root.kioskHost ? 44 : 22
            radius: 4
            color: chevArea.containsMouse ? theme.surfaceHover : "transparent"

            QbzIcon {
                anchors.centerIn: parent
                name: root.collapsed ? "chevron-right" : "chevron-down"
                width: 14
                height: 14
                tintName: "muted"
            }
            MouseArea {
                id: chevArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: root.toggled()
            }
        }
    }

    /// Optional control mounted at the START of the right-hand row, i.e. LEFT
    /// of "View all" and the page chevrons. Added for the Qobuz-Playlists
    /// rail's category filter, which the reference puts on this very line
    /// (HomeView.slint:358-374).
    ///
    /// A Component, not an Item: it is declared by the HOST, so it keeps the
    /// host's scope and can see the host's ids — and it is only instantiated
    /// where it is set. The `visible`/`active` gate keeps the thirteen existing
    /// instantiations pixel-identical, because `Row` skips invisible children
    /// and adds no spacing for them.
    property Component leading: null

    Row {
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        spacing: 4
        Loader {
            visible: root.leading !== null
            active: root.leading !== null
            sourceComponent: root.leading
            anchors.verticalCenter: parent.verticalCenter
        }
        // "View all" link/chip (accent = Search's, secondary = Home's).
        Rectangle {
            visible: root.showViewAll
            anchors.verticalCenter: parent.verticalCenter
            width: vaText.implicitWidth + 16
            height: root.kioskHost ? 44 : 26
            radius: 4
            color: vaArea.containsMouse ? theme.surfaceHover : "transparent"
            Text {
                id: vaText
                anchors.centerIn: parent
                text: root.viewAllLabel !== ""
                    ? root.viewAllLabel
                    : QbzSession.tr("View all →", QbzSession.trRev)
                color: root.viewAllAccent ? theme.accent
                    : vaArea.containsMouse ? theme.textPrimary : theme.textSecondary
                font.pixelSize: 14
                font.weight: theme.weightMedium
            }
            MouseArea {
                id: vaArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: root.viewAllClicked()
            }
        }
        // `ambient: true` on BOTH: these two are the carousel page chevrons,
        // and all six Slint carousels give them the third idle-fill arm —
        // Carousel / ArtistCarousel / PlaylistCarousel / RadioCarousel /
        // SlimCarousel / PinnedCarousel each declare a local `component
        // NavButton` at :11-:15 with the identical three-arm `background`
        // (verified: same `app-background-active` ->
        // `surface-elevated.with-alpha(app-background-surface-alpha)` line in
        // all six). Hardcoded rather than exposed as a property because every
        // consumer of this header is one of those carousels: HomeView,
        // SectionRail, SearchView and LabelView all render inside AppShell's
        // frosted contentFrame (AppShell.qml:149, surface-main @0.22), and
        // upstream HomeView.slint / SearchResultsView.slint /
        // LabelPageView.slint import the carousel components literally. A
        // section header on OPAQUE chrome would need the arm off — there is
        // no such host today, and adding one means adding the property then,
        // not carrying a dead knob now.
        //
        // The Playlist Manager's two headers are the first consumers that are
        // NOT carousels, and they say so with `showChevrons: false`.
        QbzNavButton {
            width: root.kioskHost ? 44 : 28
            height: root.kioskHost ? 44 : 28
            visible: root.showChevrons
            name: "chevron-left"
            ambient: true
            btnEnabled: root.leftEnabled
            onClicked: root.pageLeft()
        }
        QbzNavButton {
            width: root.kioskHost ? 44 : 28
            height: root.kioskHost ? 44 : 28
            visible: root.showChevrons
            name: "chevron-right"
            ambient: true
            btnEnabled: root.rightEnabled
            onClicked: root.pageRight()
        }
    }
}
