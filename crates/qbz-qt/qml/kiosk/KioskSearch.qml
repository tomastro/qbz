// Search tabs share authoritative history. Only the active viewport is mounted.
import QtQuick
import com.blitzfc.qbz
import "../theme"
import "../controls"

Rectangle {
    id: root
    color: "transparent"
    QbzTheme { id: theme }
    readonly property var doc: { try { return JSON.parse(QbzSearch.searchJson) } catch(e) { return ({}) } }
    property string restoringQuery: ""
    property var shelfPositions: ({})
    function saveShelf(key, value) { var next = Object.assign({}, shelfPositions); next[key] = value; shelfPositions = next }
    property int restoringTab: 0
    readonly property int tab: restoringQuery !== "" ? restoringTab : (doc.tab || 0)
    onDocChanged: if (restoringQuery !== "" && doc.query === restoringQuery && !doc.loading) restoringQuery = ""
    readonly property string activeTab: String(tab)
    readonly property var tabNames: ["All", "Albums", "Tracks", "Artists", "Playlists"]
    readonly property var activeRows: restoringQuery !== "" ? [] : tab === 1 ? (doc.albums || []) : tab === 2 ? (doc.tracks || []) : tab === 3 ? (doc.artists || []) : tab === 4 ? (doc.playlists || []) : []
    readonly property int columns: Math.max(2, Math.floor((width - 32) / 142))
    readonly property bool contentFocus: QbzKioskNav.navActive && QbzKioskNav.zone === "content"
    readonly property int focusedItem: QbzKioskNav.index - 5
    readonly property var dashboard: {
        var result = [], mp = doc.mostPopular || ({})
        if (restoringQuery !== "") return result
        if (mp.kind && mp[mp.kind]) result.push({ title: "Most popular", kind: mp.kind, rows: [mp[mp.kind]], tab: -1 })
        var artists = doc.artistsCarousel || doc.artists || []
        if (artists.length) result.push({ title: "Artists", kind: "artist", rows: artists, tab: 3 })
        if ((doc.albums || []).length) result.push({ title: "Albums", kind: "album", rows: doc.albums, tab: 1 })
        if ((doc.tracks || []).length) result.push({ title: "Tracks", kind: "track", rows: doc.tracks, tab: 2 })
        if ((doc.playlists || []).length) result.push({ title: "Playlists", kind: "playlist", rows: doc.playlists, tab: 4 })
        return result
    }
    property var tabNavigationRequest: ({})
    onTabNavigationRequestChanged: if (tabNavigationRequest && tabNavigationRequest.tab !== undefined) activateTab(tabNavigationRequest.tab)
    function activateTab(value) { selectTab(Number(value)) }
    function selectTab(value) {
        if (value < 0 || value > 4 || value === tab) return
        navigation.recordTab(String(value))
        if (restoringQuery !== "") restoringTab = value
        QbzSearch.searchTabChanged(value)
    }
    function open(kind, id) {
        if (kind === "artist") QbzArtist.openArtist(id)
        else if (kind === "playlist") QbzBridge.openPlaylist(id)
        else if (kind === "track") QbzPlayer.playTrack(id)
        else QbzAlbum.openAlbum(id)
    }
    function publishNav() { QbzKioskNav.publishNav(5, tab === 2 ? 1 : columns, 5 + activeRows.length, false) }
    onActiveRowsChanged: Qt.callLater(publishNav)
    onTabChanged: Qt.callLater(publishNav)
    onColumnsChanged: Qt.callLater(publishNav)
    Component.onCompleted: publishNav()
    KioskNavigation {
        id: navigation
        route: "search"
        snapshot: ({ activeTab: String(root.tab), query: root.restoringQuery || root.doc.query || "", shelfPositions: root.shelfPositions })
        onRestore: function(saved) {
            root.shelfPositions = saved.shelfPositions || ({})
            var value = saved.activeTab !== undefined ? Number(saved.activeTab) : root.tab
            if (!isFinite(value) || value < 0 || value > 4) value = 0
            if (saved.query && saved.query !== root.doc.query) {
                root.restoringTab = value
                root.restoringQuery = saved.query
                QbzSearch.kioskRestoreSearch(saved.query, value)
            } else if (value !== root.tab) QbzSearch.searchTabChanged(value)
        }
    }
    Connections {
        target: QbzKioskNav
        function onIndexChanged() {
            if (root.contentFocus && root.focusedItem >= 0 && body.item && body.item.positionViewAtIndex)
                body.item.positionViewAtIndex(root.focusedItem, ListView.Contain)
        }
        function onActivateSeqChanged() {
            if (!root.contentFocus) return
            if (QbzKioskNav.index < 5) { root.selectTab(QbzKioskNav.index); return }
            var row = root.activeRows[root.focusedItem]
            if (row) root.open(root.tab === 2 ? "track" : root.tab === 3 ? "artist" : root.tab === 4 ? "playlist" : "album", row.id || "")
        }
    }
    Row {
        id: tabs
        x: 16; height: 64; spacing: 4
        Repeater {
            model: root.tabNames
            delegate: Rectangle {
                required property string modelData
                required property int index
                width: Math.max(64, Math.min((root.width - 48) / 5, label.implicitWidth + 22)); height: 64
                radius: theme.radiusSm; color: root.tab === index ? theme.surfaceElevated : "transparent"
                border.width: root.contentFocus && QbzKioskNav.index === index ? 2 : 0; border.color: theme.accent
                Text { id: label; anchors.centerIn: parent; text: QbzSession.tr(modelData, QbzSession.trRev); color: root.tab === index ? theme.textPrimary : theme.textSecondary; font.pixelSize: 16 }
                MouseArea { anchors.fill: parent; onClicked: root.selectTab(index) }
            }
        }
    }
    Loader {
        id: body
        anchors.top: tabs.bottom; anchors.bottom: parent.bottom; anchors.left: parent.left; anchors.right: parent.right
        sourceComponent: root.tab === 0 ? dashboardView : root.tab === 2 ? tracksView : cardsView
    }
    ScrollMemory { target: body.item; scope: "search:" + root.tab }
    KioskSkeleton {
        anchors.fill: body; kind: root.tab === 2 ? "list" : "grid"
        loading: (root.restoringQuery !== "" || !!root.doc.loading) && (root.tab === 0 ? root.dashboard.length === 0 : root.activeRows.length === 0)
        error: root.doc.error || ""
        empty: root.restoringQuery === "" && !root.doc.loading && (root.tab === 0 ? root.dashboard.length === 0 : root.activeRows.length === 0)
    }
    Component {
        id: cardsView
        GridView {
            id: grid
            clip: true; cacheBuffer: 0; reuseItems: true
            leftMargin: 16; rightMargin: 16; topMargin: 16; bottomMargin: 16
            boundsBehavior: Flickable.StopAtBounds
            cellWidth: (width - 32) / root.columns
            cellHeight: cellWidth + 48
            model: root.activeRows
            onAtYEndChanged: if (atYEnd && count > 0 && !root.doc.loading) QbzSearch.searchLoadMore(root.tab)
            delegate: Loader {
                id: cell
                required property var modelData
                required property int index
                width: grid.cellWidth - 14; height: grid.cellHeight - 14
                sourceComponent: root.tab === 3 ? artistCard : albumCard
                KioskCoverSource { id: cover; remote: cell.modelData.artUrl || ""; local: cell.modelData.artPath || ""; edge: cell.width }
                Component {
                    id: artistCard
                    KioskArtistCard {
                        artSize: cell.width
                        artist: ({ id: cell.modelData.id || "", title: cell.modelData.title || "", artwork: cover.source })
                        navFocused: root.contentFocus && root.focusedItem === cell.index
                        onClicked: function(id) { root.open("artist", id) }
                    }
                }
                Component {
                    id: albumCard
                    KioskCard {
                        artSize: cell.width
                        album: ({ id: cell.modelData.id || "", title: cell.modelData.title || "", artist: cell.modelData.artist || "", artwork: cover.source, qualityTier: cell.modelData.qualityTier || "" })
                        navFocused: root.contentFocus && root.focusedItem === cell.index
                        onClicked: function(id) { root.open(root.tab === 4 ? "playlist" : "album", id) }
                    }
                }
            }
        }
    }
    Component {
        id: tracksView
        ListView {
            id: tracks
            clip: true; cacheBuffer: 0; reuseItems: true
            model: root.activeRows
            boundsBehavior: Flickable.StopAtBounds
            onAtYEndChanged: if (atYEnd && count > 0 && !root.doc.loading) QbzSearch.searchLoadMore(2)
            delegate: KioskTrackRow {
                id: row
                required property var modelData
                required property int index
                width: tracks.width; height: 64
                KioskCoverSource { id: cover; remote: row.modelData.artUrl || ""; local: row.modelData.artPath || ""; edge: 46 }
                track: ({ id: modelData.id || "", title: modelData.title || "", artist: modelData.artist || "", duration: modelData.duration || "", artwork: cover.source })
                navFocused: root.contentFocus && root.focusedItem === index
                onClicked: root.open("track", modelData.id || "")
            }
        }
    }
    Component {
        id: dashboardView
        ListView {
            id: dashboardList
            clip: true; cacheBuffer: 0; reuseItems: true; spacing: 20
            leftMargin: 16; rightMargin: 16; topMargin: 12; bottomMargin: 16
            model: root.dashboard
            boundsBehavior: Flickable.StopAtBounds
            delegate: Column {
                id: section
                required property var modelData
                // GUARDED, STABLE section data. The preview Components below are
                // instantiated by a Loader, and at that instant the delegate's
                // `modelData` is briefly undefined; a Component that read
                // `section.modelData.rows` DIRECTLY threw "Cannot read property
                // 'rows' of undefined", which killed the binding for good and
                // left every dashboard shelf empty (the 2026-09-07 "search is
                // broken" regression from the hardening's dashboard rewrite —
                // reproduced in a bare `qml` scene; the guarded read below is
                // what stops the throw, NOT disabling reuse, which still threw).
                readonly property string sTitle: section.modelData && section.modelData.title ? section.modelData.title : ""
                readonly property string sKind: section.modelData && section.modelData.kind ? section.modelData.kind : "album"
                readonly property var sRows: section.modelData && section.modelData.rows ? section.modelData.rows : []
                readonly property int sTab: section.modelData && section.modelData.tab !== undefined ? section.modelData.tab : -1
                width: dashboardList.width - 32; spacing: 8
                Item {
                    width: parent.width; height: 44
                    Text { anchors.left: parent.left; anchors.right: viewAll.left; height: parent.height; text: QbzSession.tr(section.sTitle, QbzSession.trRev); color: theme.textPrimary; font.pixelSize: 18; font.weight: theme.weightSemibold; verticalAlignment: Text.AlignVCenter; elide: Text.ElideRight }
                    Rectangle {
                        id: viewAll
                        anchors.right: parent.right; width: 100; height: 44
                        visible: section.sTab >= 0; color: "transparent"
                        Text { anchors.centerIn: parent; text: QbzSession.tr("View all", QbzSession.trRev); color: theme.textSecondary; font.pixelSize: 14 }
                        MouseArea { anchors.fill: parent; onClicked: root.selectTab(section.sTab) }
                    }
                }
                // The preview is a DIRECT child of the delegate, NOT a
                // Component behind a Loader. The Loader form (a Component
                // defined in the delegate, reading `section.modelData`/`section`)
                // is what emptied every shelf on 2026-09-07: the Component's
                // bindings to the delegate resolved against an undefined
                // `section` at instantiation and never recovered. KioskDiscover
                // mounts its shelf as a direct child for exactly this reason;
                // this matches it. The two kinds are two direct children gated
                // by `visible`; the track preview is a bounded Repeater (<=3),
                // never a nested ListView.
                KioskShelf {
                    visible: section.sKind !== "track"
                    width: parent.width
                    kind: section.sKind
                    rows: section.sKind !== "track" ? section.sRows : []
                    restoreX: root.shelfPositions[(root.doc.query || "") + ":" + section.sTitle] || 0
                    onScrollSettled: function(x) { root.saveShelf((root.doc.query || "") + ":" + section.sTitle, x) }
                    onOpen: function(id) { root.open(section.sKind, id) }
                }
                Column {
                    visible: section.sKind === "track"
                    width: parent.width
                    spacing: 0
                    Repeater {
                        model: section.sKind === "track" ? section.sRows.slice(0, 3) : []
                        delegate: KioskTrackRow {
                            id: previewRow
                            required property var modelData
                            required property int index
                            width: parent.width; height: 64
                            KioskCoverSource { id: cover; remote: previewRow.modelData.artUrl || ""; local: previewRow.modelData.artPath || ""; edge: 46 }
                            track: ({ id: modelData.id || "", title: modelData.title || "", artist: modelData.artist || "", duration: modelData.duration || "", artwork: cover.source })
                            onClicked: root.open("track", previewRow.modelData.id || "")
                        }
                    }
                }
            }
        }
    }
}
