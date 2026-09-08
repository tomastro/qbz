// Award "See all" listing — QML port of award/AwardAlbumsView.slint. Route
// "awardalbums".
//
// A search box over the LOADED set, the award name as the page title, and an
// explicitly paginated grid over `/award/getAlbums`. Same four exclusive body
// branches as the landing
// page, in the reference's order: loading · error+retry · no-results (a search
// that matched nothing) · the grid.
//
// The search is CLIENT-SIDE over what has been paginated in — 1:1 with the
// reference, which does not re-query for this box either. That is also why
// load-more is suppressed while a query is active: appending pages the user
// cannot see, to filter them out again, is work with no visible result.
//
// Every number, filter and page in the document comes from src/award_qt.rs;
// this file renders and reports scroll, nothing else.

import QtQuick
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

    readonly property var doc: {
        try {
            return JSON.parse(QbzHome.awardAlbumsJson)
        } catch (e) {
            return ({})
        }
    }
    readonly property var albums: root.doc.albums || []
    readonly property bool loading: QbzHome.awardAlbumsLoading === true
    readonly property bool loadError: root.doc.loadError === true
    readonly property string query: root.doc.searchQuery || ""

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

            Item { width: 1; height: 22 }

            // ---- Search ------------------------------------------------------
            QbzLineEdit {
                    kioskHost: root.kioskHost
                width: 320
                searchMode: true
                placeholder: QbzSession.tr("Search albums & tracks…", QbzSession.trRev)
                text: root.query
                onEdited: function (v) { QbzHome.awardAlbumsSearch(v) }
            }

            Item { width: 1; height: 20 }

            // ---- Title -------------------------------------------------------
            Text {
                width: parent.width
                text: root.doc.name || ""
                color: theme.textPrimary
                font.pixelSize: theme.fontSection
                font.weight: theme.weightBold
                elide: Text.ElideRight
            }
            Item { width: 1; height: 4 }
            Row {
                spacing: 10
                Text {
                    text: QbzSession.tr("Acclaimed Releases", QbzSession.trRev)
                    color: theme.textSecondary
                    font.pixelSize: theme.fontBody
                }
                Text {
                    visible: (root.doc.total || 0) > 0
                    text: root.doc.total || 0
                    color: theme.textMuted
                    font.pixelSize: theme.fontBody
                    font.weight: theme.weightMedium
                }
            }

            Item { width: 1; height: 16 }

            // ---- Body: loading / error+retry / no-results / grid ------------
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

            // A search that matched nothing is NOT the same as an award with
            // no releases, and the reference keeps them as two strings.
            Item {
                visible: !root.loading && !root.loadError
                    && root.albums.length === 0 && root.query !== ""
                width: parent.width
                height: visible ? 200 : 0
                Text {
                    anchors.centerIn: parent
                    text: QbzSession.tr("No results found.", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: 14
                }
            }
            Item {
                visible: !root.loading && !root.loadError
                    && root.albums.length === 0 && root.query === ""
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
                kioskHost: root.kioskHost
                id: collection
                visible: !root.loading && !root.loadError && root.albums.length > 0
                width: parent.width
                collectionKey: root.doc.id || ""
                albums: root.albums
                viewMode: "grid"
                isGrouped: false
                cardWidth: 200
                cardHeight: 266
                cardGap: 24
                flick: page
                contentOffset: root.kioskHost ? y : (collection.y)
            }

            Item {
                visible: !root.loading && !root.loadError
                    && root.albums.length > 0
                    && root.doc.hasMore === true
                    && root.query === ""
                width: parent.width
                height: visible ? loadMore.height : 0

                QbzLoadMore {
                    id: loadMore
                    width: parent.width
                    buttonHeight: root.kioskHost ? 64 : 32
                    busy: root.doc.loadingMore === true
                    skeleton: "cards"
                    cellW: 224
                    cellH: 290
                    onClicked: {
                        collection.armTailFade()
                        QbzHome.awardAlbumsLoadMore()
                    }
                }
            }
        }

        // Back/forward scroll memory (controls/ScrollMemory.qml): reports
        // this container's offset while it is the live page, and restores it
        // when a back/forward step arms this route.
        ScrollMemory { target: page; scope: "awardalbums" }
        QbzScrollBar {
            target: page
            anchors.right: parent.right
            anchors.rightMargin: 4
            anchors.top: parent.top
            anchors.bottom: parent.bottom
        }
    }
}
