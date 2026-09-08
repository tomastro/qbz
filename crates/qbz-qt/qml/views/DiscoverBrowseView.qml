// Discover "View all" full-list page — QML port of
// discover/DiscoverBrowseView.slint. One /discover/<module> endpoint (New
// Releases, Press Accolades, Ideal Discography, Qobuzissimes, Albums of the
// Week, Most Streamed) as a paginated full grid or list.
//
// Layout, read off the .slint:
//   :44  fixed 56px header — title LEFT, search / genre / grid-list RIGHT
//   :139 body padding 32 / 32, top 8, bottom 100
//   :26  200x266 cards, gap 24; :29 64px list rows, gap 4
//   :126 used to be infinite scroll. Qt deliberately promotes that trigger to
//        the shared explicit Load-more control: republishing a larger model
//        from inside `onContentYChanged` changes the Flickable extent while it
//        still carries momentum, which is the source of the tail rubber-band.
//   :205 scrollbar below the header; :233 genre popup anchored y=56
//
// NAV BUTTONS: the .slint draws a NavButtons pair at x=32 and starts the
// title 16px after it. In this port back/forward live in the shell header
// (HeaderBar.qml:235), so the title starts at x=48 — the 32 + 0 + 16 the
// HomeView toolbar already uses (HomeView.qml:744).
//
// Back/forward scroll restore is handled by the shared ScrollMemory below;
// `windowed` grid mounting is the AlbumCollection sampler rather than the
// .slint's own.

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
            return JSON.parse(QbzHome.discoverBrowseJson)
        } catch (e) {
            return {}
        }
    }
    readonly property var items: doc.items || []
    readonly property string query: doc.query || ""
    // Seeded to "grid" by the Rust opener — an empty mode renders nothing.
    readonly property string viewMode: doc.viewMode || "grid"

    // --- Shared genre selection ------------------------------------------
    // `genre_filter_qt::after_selection_change` only knows how to reload
    // Home, so this page watches the published selection itself and asks for
    // a re-fetch from offset 0. The signature is the selected NAMES, not the
    // count: swapping one genre for another leaves the count unchanged.
    readonly property string genreSig: {
        try {
            var d = JSON.parse(QbzBridge.genreFilterJson)
            return JSON.stringify((d.names || {})["discover"] || [])
        } catch (e) {
            return ""
        }
    }
    property string lastGenreSig: ""
    onGenreSigChanged: {
        if (root.lastGenreSig !== "" && root.lastGenreSig !== root.genreSig)
            QbzHome.discoverBrowseGenreChanged()
        root.lastGenreSig = root.genreSig
    }
    Component.onCompleted: root.lastGenreSig = root.genreSig

    // ============================ offline gate ============================
    QbzOfflinePlaceholder {
        visible: QbzSession.offline
        anchors.centerIn: parent
        showSettingsAction: true
        onSettingsClicked: QbzShell.navigateTo("settings")
    }

    Column {
        anchors.fill: parent
        spacing: 0
        visible: !QbzSession.offline

        // --- Fixed 56px header -------------------------------------------
        Item {
            id: header
            width: parent.width
            height: root.kioskHost ? 64 : 56

            Rectangle {
                anchors.fill: parent
                // Rounded at the TOP because this bar is full-bleed at y=0 of
                // the content pane, and under the dynamic background the pane's
                // own rounding cannot reach it: Qt's `clip` is a rectangular
                // scissor that ignores `radius`, and AppShell hides its bezel
                // nubs while the field is meant to show through the corners.
                // So a full-bleed pane child rounds ITSELF — AppShell.qml says
                // exactly this ("there is no mask that can do it for it here")
                // and this bar was the counterexample the owner spotted in
                // Discover. Invisible with the background off: the view root
                // paints the same colour underneath.
                topLeftRadius: theme.radiusMd
                topRightRadius: theme.radiusMd
                color: root.ambientOn ? theme.surfaceMainA30 : theme.surfaceMain
            }

            Text {
                x: 48
                y: (root.kioskHost ? 32 : 25) - height / 2
                width: Math.max(0, tools.x - 48 - 16)
                text: root.doc.title || ""
                color: theme.textPrimary
                font.pixelSize: theme.fontSection
                font.weight: theme.weightBold
                elide: Text.ElideRight
            }

            Row {
                id: tools
                x: parent.width - width - 32
                y: (root.kioskHost ? 32 : 25) - height / 2
                spacing: 8

                QbzLineEdit {
                    kioskHost: root.kioskHost
                    searchMode: true
                    width: 200
                    placeholder: QbzSession.tr("Search…", QbzSession.trRev)
                    text: root.query
                    onEdited: function (v) { QbzHome.discoverBrowseSearch(v) }
                }
                BrowseGenreButton {
                    btnHeight: root.kioskHost ? 44 : 34
                    context: "discover"
                    onClicked: genrePopup.toggle()
                }
                ViewModeToggle {
                    visible: !root.kioskHost
                    mode: root.viewMode
                    onSetMode: function (m) { QbzHome.discoverBrowseSetViewMode(m) }
                }
            }
        }

        // --- Scrolling list ----------------------------------------------
        Item {
            width: parent.width
            height: parent.height - (root.kioskHost ? 64 : 56)

            Flickable {
                id: flick
                anchors.fill: parent
                clip: true
                contentWidth: width
                contentHeight: page.implicitHeight
                boundsBehavior: Flickable.StopAtBounds

                Column {
                    id: page
                    width: parent.width
                    leftPadding: 32
                    rightPadding: 32
                    topPadding: 8
                    bottomPadding: 100
                    spacing: 0

                    QbzSpinner {
                        visible: QbzHome.discoverBrowseLoading
                        size: 36
                        anchors.horizontalCenter: parent.horizontalCenter
                    }

                    // Search filtered everything out.
                    Text {
                        visible: !QbzHome.discoverBrowseLoading
                            && root.items.length === 0 && root.query !== ""
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: QbzSession.tr("No results found.", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: 14
                    }

                    AlbumCollection {
                kioskHost: root.kioskHost
                        id: collection
                        visible: !QbzHome.discoverBrowseLoading
                        width: parent.width - 64
                        collectionKey: root.doc.endpoint || ""
                        albums: root.items
                        viewMode: root.viewMode
                        cardWidth: 200
                        cardHeight: 266
                        cardGap: 24
                        listRowGap: 4
                        flick: flick
                        contentOffset: root.kioskHost ? y : (8)
                    }

                    Item {
                        visible: !QbzHome.discoverBrowseLoading
                            && root.items.length > 0
                            && root.doc.hasMore === true
                            && root.query === ""
                        width: parent.width - 64
                        height: visible ? loadMore.height : 0

                        QbzLoadMore {
                            id: loadMore
                            width: parent.width
                            buttonHeight: root.kioskHost ? 64 : 32
                            busy: QbzHome.discoverBrowseLoadingMore
                            skeleton: root.viewMode === "list" ? "rows" : "cards"
                            // AlbumCollection pitch: 200x266 + 24px gutter.
                            cellW: 224
                            cellH: 290
                            rowH: 64
                            rowGap: 4
                            rowCount: 2
                            rowArtSize: 44
                            onClicked: {
                                collection.armTailFade()
                                QbzHome.discoverBrowseLoadMore()
                            }
                        }
                    }
                }
            }

            // Tracks only the scrolling list (the header is a sibling above).
            // Back/forward scroll memory (controls/ScrollMemory.qml): reports
            // this container's offset while it is the live page, and restores it
            // when a back/forward step arms this route.
            ScrollMemory { target: flick; scope: "discoverbrowse" }
            QbzScrollBar {
                anchors.right: parent.right
                anchors.rightMargin: 4
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                target: flick
            }
        }
    }

    // Declared LAST — declaration order IS z-order, so the popup keeps its
    // own presses instead of the toolbar swallowing them.
    GenreFilterPopup {
        id: genrePopup
        anchors.fill: parent
        context: "discover"
        // The .slint anchors this page's popup at y=56 (not HomeView's 52).
        anchorTop: 56
        anchorRight: 32
    }
}
