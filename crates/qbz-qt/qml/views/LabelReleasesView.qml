// "See all releases" — QML port of label/LabelReleasesView.slint: the
// sub-view reached from the label landing's Releases carousel. A circular
// label header over the full album catalog, with a toolbar (search /
// group-by-artist / Hi-Res filter / sort / grid-list) and load-more paging.
//
// NO FIXED HEADER: everything scrolls in one Flickable (unlike the browse
// pages). The scrolling-page convention applies — Column padding 32 / 32 /
// top 11 / bottom 100, the .slint's NavButtons row dropped but its 22px
// spacer kept (see LabelView.qml).
//
// The rendered set (`visible` / `grouped`) is derived in RUST
// (src/label_qt.rs::derive_releases, the port of label.rs:177-249), including
// the fast path that reuses the live catalog when nothing is filtered.
// `hiresCount` is computed over the FULL loaded catalog BEFORE filtering
// (label.rs:190-193) — the count line reports the catalog's Hi-Res total,
// not the filtered one.
//
// Load more is driven by `hasMore`, NEVER by `total`: /label/getAlbums caps
// each page well below the catalog, so `total` is an unreliable gate
// (.slint:296-298).
//
// View mode is UI-only in the .slint (`LabelState.view-mode` is written
// straight from the toggle), so it stays purely in QML here too.

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    color: ambientOn ? "transparent" : theme.surfaceMain
    readonly property bool ambientOn: theme.ambientOn
    radius: 12

    QbzTheme { id: theme }

    readonly property var doc: {
        try {
            return JSON.parse(QbzHome.labelReleasesJson)
        } catch (e) {
            return {}
        }
    }
    readonly property var visibleAlbums: doc.visible || []
    readonly property var grouped: doc.grouped || []
    readonly property bool groupBy: doc.groupBy === true
    readonly property int shown: doc.shown || 0
    readonly property int total: doc.total || 0
    readonly property int hiresCount: doc.hiresCount || 0

    /// UI-only (see the header note).
    property string viewMode: "grid"

    readonly property var sortLabels: [
        QbzSession.tr("Newest", QbzSession.trRev),
        QbzSession.tr("Oldest", QbzSession.trRev),
        QbzSession.tr("Title A-Z", QbzSession.trRev),
        QbzSession.tr("Title Z-A", QbzSession.trRev),
        QbzSession.tr("Artist A-Z", QbzSession.trRev),
    ]
    readonly property var sortKeys: ["newest", "oldest", "title-asc", "title-desc", "artist-asc"]
    readonly property int sortIndex: Math.max(0, root.sortKeys.indexOf(root.doc.sortBy || "newest"))

    QbzOfflinePlaceholder {
        visible: QbzSession.offline
        anchors.centerIn: parent
        showSettingsAction: true
        onSettingsClicked: QbzShell.navigateTo("settings")
    }

    Flickable {
        id: flick
        anchors.fill: parent
        visible: !QbzSession.offline
        clip: true
        contentWidth: width
        contentHeight: page.implicitHeight
        boundsBehavior: Flickable.StopAtBounds

        Column {
            id: page
            width: parent.width
            leftPadding: 32
            rightPadding: 32
            topPadding: 11
            bottomPadding: 100
            spacing: 0

            Item { width: 1; height: 22 }

            // --- Label header (row spacing 24, right column spacing 6) ----
            Row {
                width: parent.width - 64
                spacing: 24

                Rectangle {
                    width: 180
                    height: 180
                    radius: 90
                    color: theme.surfaceElevated
                    clip: true
                    QbzIcon {
                        visible: (root.doc.imageUrl || "") === ""
                        name: "disc-3"
                        width: 60
                        height: 60
                        anchors.centerIn: parent
                        tintName: "muted"
                    }
                    // Contain-fit, same conscious deviation as LabelView.qml
                    // (both .slint label views use cover).
                    RoundedImage {
                        anchors.fill: parent
                        anchors.margins: 26
                        source: root.doc.artPath || ""
                        radius: 0
                        fit: "contain"
                    }
                }

                Column {
                    width: parent.width - 180 - 24
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 6
                    Text {
                        text: QbzSession.tr("LABEL", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: 10
                        font.weight: theme.weightSemibold
                        font.letterSpacing: 1.5
                    }
                    Text {
                        width: parent.width
                        text: root.doc.name || ""
                        color: theme.textPrimary
                        font.pixelSize: theme.fontSection
                        font.weight: theme.weightBold
                        elide: Text.ElideRight
                    }
                    Text {
                        visible: root.total > 0
                        text: root.total + " " + QbzSession.tr("albums", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: theme.fontBody
                    }
                }
            }

            Item { width: 1; height: 24 }

            // --- Toolbar ---------------------------------------------------
            Item {
                width: parent.width - 64
                height: 44

                Column {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    width: Math.max(0, parent.width - tools.width - 12)
                    spacing: 2
                    Text {
                        width: parent.width
                        text: QbzSession.tr("Releases", QbzSession.trRev)
                        color: theme.textPrimary
                        font.pixelSize: theme.fontHeading
                        font.weight: theme.weightSemibold
                        elide: Text.ElideRight
                    }
                    Text {
                        width: parent.width
                        text: root.shown + " " + QbzSession.tr("albums", QbzSession.trRev)
                            + (root.hiresCount > 0 ? " · " + root.hiresCount + " Hi-Res" : "")
                        color: theme.textMuted
                        font.pixelSize: theme.fontLegal
                        elide: Text.ElideRight
                    }
                }

                Row {
                    id: tools
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8

                    QbzLineEdit {
                    kioskHost: root.kioskHost
                        searchMode: true
                        expandable: true
                        anchors.verticalCenter: parent.verticalCenter
                        placeholder: QbzSession.tr("Search this label…", QbzSession.trRev)
                        onEdited: function (v) { QbzHome.labelReleasesSearch(v) }
                    }
                    QbzToggleButton {
                        width: root.kioskHost ? 44 : (sm ? 30 : 34)
                        height: root.kioskHost ? 44 : (sm ? 30 : 34)
                        name: "user"
                        active: root.groupBy
                        anchors.verticalCenter: parent.verticalCenter
                        onClicked: QbzHome.labelReleasesSetGroup(!root.groupBy)
                    }
                    QbzToggleButton {
                        width: root.kioskHost ? 44 : (sm ? 30 : 34)
                        height: root.kioskHost ? 44 : (sm ? 30 : 34)
                        name: "list-filter"
                        active: root.doc.filterHires === true
                        anchors.verticalCenter: parent.verticalCenter
                        onClicked: QbzHome.labelReleasesSetHires(root.doc.filterHires !== true)
                    }
                    QbzSelect {
                        kioskHost: root.kioskHost
                        anchors.verticalCenter: parent.verticalCenter
                        menuWidth: 168
                        options: root.sortLabels
                        currentIndex: root.sortIndex
                        onSelected: function (i) { QbzHome.labelReleasesSetSort(root.sortKeys[i]) }
                    }
                    ViewModeToggle {
                    visible: !root.kioskHost
                        anchors.verticalCenter: parent.verticalCenter
                        mode: root.viewMode
                        onSetMode: function (m) { root.viewMode = m }
                    }
                }
            }

            Item { width: 1; height: 24 }

            // --- States ----------------------------------------------------
            QbzSpinner {
                visible: QbzHome.labelReleasesLoading && root.shown === 0
                    && root.grouped.length === 0
                size: 36
                anchors.horizontalCenter: parent.horizontalCenter
            }

            // Search / filter matched nothing (the catalog itself is loaded).
            Column {
                visible: !QbzHome.labelReleasesLoading && root.total > 0 && root.shown === 0
                anchors.horizontalCenter: parent.horizontalCenter
                spacing: 10
                QbzIcon {
                    name: "search"
                    width: 32
                    height: 32
                    anchors.horizontalCenter: parent.horizontalCenter
                    tintName: "muted"
                }
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: QbzSession.tr("No results", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: 14
                }
            }

            // --- Album collection -----------------------------------------
            AlbumCollection {
                kioskHost: root.kioskHost
                id: collection
                width: parent.width - 64
                // Identity of the catalog on screen. Navigating to ANOTHER
                // label disarms a tail fade that was armed by a click whose
                // page never landed, so the new catalog's first paint is
                // never coloured by it (AlbumCollection.collectionKey).
                collectionKey: root.doc.id || ""
                albums: root.visibleAlbums
                grouped: root.grouped
                isGrouped: root.groupBy
                viewMode: root.viewMode
                cardWidth: 200
                cardHeight: 266
                cardGap: 24
                listRowGap: 4
                flick: flick
                // .slint:280 — 11 padding-top + 22 spacer + 180 header + 24
                // + ~42 toolbar + 24 ~= 303.
                contentOffset: root.kioskHost ? y : (303)
            }

            // --- Load more (gated on hasMore, NEVER on total) -------------
            // Ported to the shared controls/QbzLoadMore.qml (2026-08-02).
            // This site is the ONE that already had a real busy arm — the
            // "Loading…" label swap and the disarmed MouseArea this block used
            // to spell out by hand are what the control's `busy` property was
            // modelled on, so the wiring is a straight rename and the idle
            // pixels are unchanged (plain arm, 32-tall box, same 13px label,
            // same `implicitWidth + 24` hit box).
            //
            // The wrapper Item STAYS: `- 64` is the page Column's own
            // left+right padding, and a Column's padding does NOT narrow
            // `parent.width` for its children, so the wrapper is what keeps
            // this block inside the gutters. Its collapsed height is still 0.
            Item {
                visible: root.shown > 0 && root.doc.hasMore === true
                width: parent.width - 64
                // Was a literal `visible ? 32 : 0`. The box now also has to
                // make room for the skeleton block the control drops UNDER
                // the button while a page is in flight; idle that block is
                // 0-tall, so this is still exactly 32 (and still 0 hidden).
                height: visible ? loadMore.height : 0

                QbzLoadMore {
                    id: loadMore
                    width: parent.width
                    // 32 here, not the 28 of the plain sites — this block was
                    // always a 32-tall box.
                    buttonHeight: root.kioskHost ? 64 : 32
                    busy: root.doc.loadMoreLoading === true
                    // The placeholder must be the shape of what will land, and
                    // this page has both arms: `viewMode` is the UI-only grid /
                    // list toggle (see the header note), read here exactly as
                    // AlbumCollection reads it ("list" vs everything else).
                    skeleton: root.viewMode === "list" ? "rows" : "cards"
                    // The grid PITCH, not the card size: the collection above
                    // is mounted with 200x266 cards and a 24px gap, so the
                    // pitch is 224 x 290 and the control derives the drawn
                    // card back as pitch - 24 = the same 200x266.
                    cellW: 224
                    cellH: 290
                    // List arm: AlbumListRow is 64 tall and this page mounts
                    // the collection with listRowGap 4; its art cell is the
                    // 44px thumb (AlbumListRow.qml:144-150), not a full-height
                    // square.
                    rowH: 64
                    rowGap: 4
                    rowCount: 2
                    rowArtSize: 44
                    onClicked: {
                        // Snapshot the album ids already on screen so ONLY the
                        // page that lands fades in. BEFORE the bridge call:
                        // labelReleasesLoadMore() may republish synchronously.
                        collection.armTailFade()
                        QbzHome.labelReleasesLoadMore()
                    }
                }
            }
        }
    }

    // Back/forward scroll memory (controls/ScrollMemory.qml): reports
    // this container's offset while it is the live page, and restores it
    // when a back/forward step arms this route.
    ScrollMemory { target: flick; scope: "labelreleases" }
    QbzScrollBar {
        anchors.right: parent.right
        anchors.rightMargin: 4
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        target: flick
    }
}
