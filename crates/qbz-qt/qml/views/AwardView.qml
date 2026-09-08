// Award landing page — QML port of award/AwardView.slint (itself a 1:1 port
// of Tauri's AwardView.svelte). Route "award".
//
// A circular gold/laurel hero with the award name + magazine + a Follow
// heart, a section header ("Acclaimed Releases" + count + "See all" when more
// exist), a bounded PREVIEW grid, and an "Other awards" carousel.
//
// SCROLLING-PAGE CONVENTION (AlbumView / ArtistView / LabelView): one
// Flickable whose Column pads 32 / 32 / top 11 / bottom 100. The .slint's
// NavButtons row is dropped — back/forward live in the shell header — but its
// 22px spacer is KEPT so the hero sits where every other detail page's does.
//
// Composition: the grid is views/AlbumCollection.qml (the Discover/Label
// grid) and the heart is controls/QbzCircleAction.qml. The ONE local
// component is the "Other awards" card — see AwardRailCard below for why the
// shared ArtistCard could not be reused here.
//
// THE ONE ADDITION over the reference: the all-awards dropdown, pinned to the
// page's top-right. The Slint can only reach an award by finding an album that
// won it;
// this lists every award the `/award/explore` crawl found, so you can jump
// straight to any of them. The crawl already existed to resolve names to ids —
// the control is new, the data is not.

import QtQuick
import "../kiosk"
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    readonly property bool ambientOn: theme.ambientOn
    color: root.ambientOn ? "transparent" : theme.surfaceMain
    radius: 12

    QbzTheme { id: theme }

    // One "Other awards" card. Local, and deliberately NOT cards/ArtistCard:
    // that one calls `QbzArtist.openArtist(item.id)` INSIDE its own MouseArea
    // and again from its menu (ArtistCard.qml:197, :294, :331) with no signal
    // to override — handing it an award id would open an ARTIST page for a
    // number that means something else entirely. The Slint could reuse its
    // carousel because the Slint one emits `artist-clicked(id)` outward; this
    // port's card swallows the click, so reuse here is not the same trade.
    //
    // It also drops the deviation the reference had to accept: an award with
    // no image gets a LAUREL on gold, not the person glyph ArtistCard falls
    // back to.
    component AwardRailCard: Item {
        id: railCard
        property var entry: ({})
        width: 160
        height: 200

        Column {
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 10
            Rectangle {
                width: 140
                height: 140
                radius: 70
                clip: true
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0.0; color: "#b45309" }
                    GradientStop { position: 1.0; color: "#eab308" }
                }
                QbzIcon {
                    anchors.centerIn: parent
                    visible: (railCard.entry.imageUrl || "") === ""
                    name: "award"
                    width: 56
                    height: 56
                    tintName: theme.accentGlyphTint
                }
                Loader { anchors.fill: parent; sourceComponent: root.kioskHost ? kioskImage3265 : desktopImage3265
 Component { id: kioskImage3265; KioskArtwork {
                    anchors.fill: parent
                    visible: (railCard.entry.imageUrl || "") !== ""
                    radius: 70
                    source: railCard.entry.imageUrl || ""
                } }
 Component { id: desktopImage3265; RoundedImage {
                    anchors.fill: parent
                    visible: (railCard.entry.imageUrl || "") !== ""
                    radius: 70
                    source: railCard.entry.imageUrl || ""
                } }
 }
            }
            Text {
                width: 140
                horizontalAlignment: Text.AlignHCenter
                text: railCard.entry.title || ""
                color: railArea.containsMouse ? theme.accent : theme.textPrimary
                font.pixelSize: theme.fontLegal
                font.weight: theme.weightMedium
                elide: Text.ElideRight
            }
            Text {
                width: 140
                visible: (railCard.entry.artist || "") !== ""
                horizontalAlignment: Text.AlignHCenter
                text: railCard.entry.artist || ""
                color: theme.textMuted
                font.pixelSize: 11
                elide: Text.ElideRight
            }
        }
        MouseArea {
            id: railArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: QbzHome.openAward(railCard.entry.id || "",
                                         railCard.entry.title || "")
        }
    }

    readonly property var doc: {
        try {
            return JSON.parse(QbzHome.awardJson)
        } catch (e) {
            return ({})
        }
    }
    readonly property var albums: root.doc.albums || []
    readonly property var otherAwards: root.doc.otherAwards || []
    readonly property var catalog: root.doc.catalog || []
    readonly property bool loading: QbzHome.awardLoading === true
    readonly property bool loadError: root.doc.loadError === true

    Flickable {
        id: page
        anchors.fill: parent
        contentWidth: width
        contentHeight: col.implicitHeight
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Column {
            id: col
            width: page.width - 64
            x: 32
            topPadding: 11
            bottomPadding: 100
            spacing: 0

            // Reserves exactly the pinned bar's height. Without it the hero
            // would jump up by that height the moment the bar was lifted out
            // of the flow (the ArtistView jumpSlot rule).
            Item {
                id: navSlot
                width: 1
                height: navBar.height
            }
            // The .slint's NavButtons row is not ported; its spacer is, so the
            // hero lands at the same y as every other detail page's header.
            Item { width: 1; height: 22 }

            // ---- Hero -------------------------------------------------------
            Row {
                width: parent.width
                spacing: 32

                Rectangle {
                    width: root.kioskHost ? 112 : 180
                    height: root.kioskHost ? 112 : 180
                    radius: 90
                    clip: true
                    gradient: Gradient {
                        orientation: Gradient.Horizontal
                        GradientStop { position: 0.0; color: "#b45309" }
                        GradientStop { position: 1.0; color: "#eab308" }
                    }
                    QbzIcon {
                        anchors.centerIn: parent
                        visible: (root.doc.imageUrl || "") === ""
                        name: "award"
                        width: 84
                        height: 84
                        // The glyph sits on a saturated gold fill, which is the
                        // on-accent case the tint selector exists for.
                        tintName: theme.accentGlyphTint
                    }
                    Loader { anchors.fill: parent; sourceComponent: root.kioskHost ? kioskImage7419 : desktopImage7419
 Component { id: kioskImage7419; KioskArtwork {
                        anchors.fill: parent
                        visible: (root.doc.imageUrl || "") !== ""
                        radius: 90
                        source: root.doc.imageUrl || ""
                    } }
 Component { id: desktopImage7419; RoundedImage {
                        anchors.fill: parent
                        visible: (root.doc.imageUrl || "") !== ""
                        radius: 90
                        source: root.doc.imageUrl || ""
                    } }
 }
                }

                Column {
                    width: parent.width - (root.kioskHost ? 112 : 180) - 32
                    spacing: 0

                    Item { width: 1; height: 8 }
                    Text {
                        text: QbzSession.tr("Award", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: 10
                        font.weight: theme.weightSemibold
                        font.letterSpacing: 1.5
                    }
                    Item { width: 1; height: 6 }
                    Text {
                        width: parent.width
                        text: root.doc.name || ""
                        color: theme.textPrimary
                        font.pixelSize: theme.fontSection
                        font.weight: theme.weightBold
                        elide: Text.ElideRight
                    }
                    Item {
                        visible: (root.doc.magazineName || "") !== ""
                        width: 1
                        height: 10
                    }
                    Text {
                        visible: (root.doc.magazineName || "") !== ""
                        width: parent.width
                        // Server text — never translated.
                        text: root.doc.magazineName || ""
                        color: theme.textSecondary
                        font.pixelSize: theme.fontBody
                        elide: Text.ElideRight
                    }

                    Item { width: 1; height: 18 }

                    QbzCircleAction {
                        diameterOverride: root.kioskHost ? 64 : 0
                        name: root.doc.isFollowing === true ? "heart-filled" : "heart"
                        active: root.doc.isFollowing === true
                        btnEnabled: root.doc.followToggling !== true
                        onClicked: QbzHome.awardFollow()
                    }
                }
            }

            Item { width: 1; height: 32 }

            // ---- Section header ---------------------------------------------
            // Anchored, not a Row: the "See all" chip pins right and the
            // title group pins left, so neither has to know the other's width
            // (a `parent.children[0].width` subtraction is how a header ends
            // up mis-measuring itself the first time a child is hidden).
            Item {
                width: parent.width
                height: 26

                Row {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 10
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: QbzSession.tr("Acclaimed Releases", QbzSession.trRev)
                        color: theme.textPrimary
                        font.pixelSize: theme.fontHeading
                        font.weight: theme.weightSemibold
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: (root.doc.total || 0) > 0
                        text: root.doc.total || 0
                        color: theme.textMuted
                        font.pixelSize: theme.fontBody
                        font.weight: theme.weightMedium
                    }
                }

                // "See all" — only when there is more than is loaded. The
                // search and the shuffle used to sit beside it; they live in
                // the pinned nav bar now, next to the award picker.
                Rectangle {
                    id: seeAll
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    visible: root.doc.hasMore === true
                    width: visible ? seeAllText.implicitWidth + 16 : 0
                    height: 26
                    radius: 4
                    color: seeAllArea.containsMouse ? theme.surfaceHover : "transparent"
                    Text {
                        id: seeAllText
                        anchors.centerIn: parent
                        // "See All" is the msgid; the arrow is decoration
                        // appended OUTSIDE it (the reference is explicit).
                        text: QbzSession.tr("See All", QbzSession.trRev) + " →"
                        color: seeAllArea.containsMouse ? theme.textPrimary : theme.textSecondary
                        font.pixelSize: 14
                        font.weight: theme.weightMedium
                    }
                    MouseArea {
                        id: seeAllArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: QbzHome.awardOpenAlbums()
                    }
                }
            }

            Item { width: 1; height: 16 }

            // ---- Body: loading / error+retry / empty / grid (exclusive) -----
            Item {
                visible: root.loading
                width: parent.width
                height: visible ? 280 : 0
                Column {
                    anchors.centerIn: parent
                    spacing: 18
                    QbzSpinner {
                        anchors.horizontalCenter: parent.horizontalCenter
                        size: 36
                    }
                    Text {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: QbzSession.tr("Loading…", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: 13
                    }
                }
            }

            Item {
                visible: !root.loading && root.loadError
                width: parent.width
                height: visible ? 240 : 0
                Column {
                    anchors.centerIn: parent
                    spacing: 16
                    Text {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: QbzSession.tr("Failed to load Library", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: 14
                    }
                    SettingsButton {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: QbzSession.tr("Retry", QbzSession.trRev)
                        onClicked: QbzHome.awardRetry()
                    }
                }
            }

            Item {
                visible: !root.loading && !root.loadError && root.albums.length === 0
                width: parent.width
                height: visible ? 200 : 0
                Text {
                    anchors.centerIn: parent
                    text: QbzSession.tr("No award-winning releases yet.", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: 14
                }
            }

            AlbumCollection {
                id: collection
                kioskHost: root.kioskHost
                visible: !root.loading && !root.loadError && root.albums.length > 0
                width: parent.width
                // Identity of the catalog on screen — navigating to ANOTHER
                // award must not inherit the previous one's tail fade
                // (AlbumCollection.collectionKey, the LabelReleasesView note).
                collectionKey: root.doc.id || ""
                albums: root.albums
                viewMode: "grid"
                isGrouped: false
                cardWidth: root.kioskHost ? 144 : 200
                cardHeight: root.kioskHost ? 210 : 266
                cardGap: 24
                flick: page
                contentOffset: collection.y
            }

            // ---- Load more ---------------------------------------------------
            // A BUTTON, not infinite scroll: the listing sub-view owns the
            // scroll-driven paging, and this page has a rail under the grid —
            // auto-loading here would keep pushing that rail out of reach.
            // Suppressed while a query is active, like the listing: paging in
            // rows the filter then hides is work with no visible result.
            Item {
                visible: root.doc.hasMore === true && (root.doc.searchQuery || "") === ""
                width: parent.width
                height: visible ? 64 : 0
                SettingsButton {
                    anchors.centerIn: parent
                    enabled: root.doc.loadingMore !== true
                    text: root.doc.loadingMore === true
                        ? QbzSession.tr("Loading…", QbzSession.trRev)
                        : QbzSession.tr("Load more", QbzSession.trRev)
                    onClicked: QbzHome.awardAlbumsLoadMore()
                }
            }

            // ---- Other awards -----------------------------------------------
            Item {
                visible: root.otherAwards.length > 0
                width: 1
                height: visible ? 32 : 0
            }
            Column {
                visible: root.otherAwards.length > 0
                width: parent.width
                spacing: 12
                Text {
                    text: QbzSession.tr("Other awards", QbzSession.trRev)
                    color: theme.textPrimary
                    font.pixelSize: theme.fontHeading
                    font.weight: theme.weightSemibold
                }
                Flickable {
                    id: awardRailViewport
                    readonly property int first: root.kioskHost ? Math.max(0,Math.floor(contentX/176)-1) : 0
                    readonly property int last: root.kioskHost ? Math.min(root.otherAwards.length,Math.ceil((contentX+width)/176)+1) : root.otherAwards.length
                    width: parent.width
                    height: 210
                    contentWidth: root.kioskHost ? root.otherAwards.length*176 : rail.width
                    contentHeight: height
                    flickableDirection: Flickable.HorizontalFlick
                    boundsBehavior: Flickable.StopAtBounds
                    clip: true
                    Row {
                        id: rail
                        x: root.kioskHost ? awardRailViewport.first*176 : 0
                        height: parent.height
                        spacing: 16
                        Repeater {
                            model: root.kioskHost ? root.otherAwards.slice(awardRailViewport.first,awardRailViewport.last) : root.otherAwards
                            delegate: AwardRailCard {
                                required property var modelData
                                entry: modelData
                            }
                        }
                    }
                }
            }
        }

        // Back/forward scroll memory (controls/ScrollMemory.qml): reports
        // this container's offset while it is the live page, and restores it
        // when a back/forward step arms this route.
        ScrollMemory { target: page; scope: "award" }
        QbzScrollBar {
            target: page
            anchors.right: parent.right
            anchors.rightMargin: 4
            anchors.top: parent.top
            anchors.bottom: parent.bottom
        }
    }

    // ---- The award nav bar (this port only) ---------------------------------
    // Pinned at the top of the page, OUTSIDE the Flickable, given the
    // ArtistView jump-nav treatment: 44px, its own fill, a 1px bottom
    // hairline, and the page's 32px horizontal rhythm.
    //
    // THE ROUNDED-CORNER RULE, and it is a real defect this codebase has hit
    // twice (ArtistView.qml:2795, the Discover full-bleed toolbar): pinned at
    // the pane's top edge the bar's SQUARE corners poke out past the shell's
    // rounded bezel. Round them — but only WHILE it is actually at the top.
    // Mid-page the bar is surrounded by content and a notch there is worse
    // than the square corner it fixes, so the radius is driven off the live y.
    //
    // The fill is not transparent: pinned, the page scrolls underneath, so it
    // needs enough body to stay readable. Under the dynamic background that is
    // surface-main at the thin-bar alpha tier, the same one ArtistView uses.
    Rectangle {
        id: navBar
        x: 0
        width: root.width
        height: root.kioskHost ? 64 : 44
        y: Math.max(page.y + navSlot.y - page.contentY, 0)
        z: 5
        color: root.ambientOn ? theme.surfaceMainA30 : theme.surfaceMain
        topLeftRadius: y <= 0 ? theme.radiusMd : 0
        topRightRadius: y <= 0 ? theme.radiusMd : 0

        Rectangle {
            x: 0
            y: parent.height - 1
            width: parent.width
            height: 1
            color: theme.borderSubtle
        }

        // All three controls in one cluster, right-aligned, with the
        // navigation dropdown LAST — the search and the shuffle act on the
        // page you are on, the dropdown takes you off it, so it reads
        // left-to-right from "narrow this" to "leave here".
        Row {
            anchors.right: parent.right
            anchors.rightMargin: 32
            anchors.verticalCenter: parent.verticalCenter
            spacing: 10

            // Filters the loaded set. Shares the LISTING's query — one award,
            // one filter — so it is still in force behind "See all".
            QbzLineEdit {
                kioskHost: root.kioskHost
                anchors.verticalCenter: parent.verticalCenter
                width: 220
                searchMode: true
                sm: true
                placeholder: QbzSession.tr("Search albums & tracks…", QbzSession.trRev)
                text: root.doc.searchQuery || ""
                onEdited: function (v) { QbzHome.awardAlbumsSearch(v) }
            }

            // Play a RANDOM album out of what is LOADED (the LibraryToolbar
            // precedent). Not the whole award: the rest is not here to pick
            // from, and reaching for it would put a blocking fetch behind a
            // button that should feel instant.
            QbzToolButton {
                width: root.kioskHost ? 64 : implicitWidth
                height: root.kioskHost ? 64 : 30
                anchors.verticalCenter: parent.verticalCenter
                visible: root.albums.length > 0
                name: "shuffle"
                label: ""
                onClicked: {
                    var pick = root.albums[
                        Math.floor(Math.random() * root.albums.length)]
                    if (pick && pick.id)
                        QbzPlayer.playAlbum(pick.id)
                }
            }

            // Jump to ANY award. Hidden until the background crawl has both a
            // list and THIS award's place in it: `currentIndex` tracks the
            // open page, so navigating away and arriving somewhere else moves
            // the selection with you instead of stranding it on the award you
            // came from.
            QbzSelect {
                kioskHost: root.kioskHost
                id: awardPicker
                anchors.verticalCenter: parent.verticalCenter
                visible: root.catalog.length > 1
                    && (root.doc.catalogIndex !== undefined)
                    && root.doc.catalogIndex >= 0
                menuWidth: 320
                popupWidth: 320
                searchable: true
                options: {
                    var out = []
                    for (var i = 0; i < root.catalog.length; i++)
                        out.push(root.catalog[i].name)
                    return out
                }
                currentIndex: root.doc.catalogIndex || 0
                onSelected: function (i) {
                    var entry = root.catalog[i]
                    if (entry && entry.id !== (root.doc.id || ""))
                        QbzHome.openAward(entry.id, entry.name || "")
                }
            }
        }
    }
}
