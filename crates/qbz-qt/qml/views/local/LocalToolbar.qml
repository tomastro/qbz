// Row-2 right-hand toolbar — the four per-tab control groups from
// LocalLibraryView.slint:774 (Albums), :924 (Tracks), :1009 (Folders) and
// :1103 (Artists). One file because they are one row: exactly one group is
// visible at a time and they share the search box, the select chrome and
// the 8px rhythm.
//
// Every Slint gate is kept: the Albums/Tracks groups only appear once the
// tab has rows OR a live search; the Folders flat-only block hides in tree
// mode while the Flat/Tree toggle stays; Artists shows the search only when
// there are artists.
//
// PRESENTATION DIVERGENCE (owner request — the row was too crowded):
//   * search   -> the EXPANDABLE form (Slint already uses ExpandableSearch
//                 here; the port used to mount an always-open 200px box);
//   * sort     -> LocalIconSelect "arrow-up-down"  (Slint: text QbzSelect)
//   * grouping -> LocalIconSelect "layers"         (Slint: text QbzSelect)
//   * filter   -> unchanged icon-only button (matches Slint) + a tooltip.
// Every option list, key mapping and callback is untouched.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Row {
    id: root

    /// The LocalLibraryView root (state + actions).
    property var view: null

    QbzTheme { id: theme }

    readonly property var sortIds: ["artist-asc", "artist-desc", "title-asc",
                                    "title-desc", "year-desc", "year-asc"]
    readonly property var sortLabels: [
        QbzSession.tr("Artist A-Z", QbzSession.trRev),
        QbzSession.tr("Artist Z-A", QbzSession.trRev),
        QbzSession.tr("Title A-Z", QbzSession.trRev),
        QbzSession.tr("Title Z-A", QbzSession.trRev),
        QbzSession.tr("Year (newest)", QbzSession.trRev),
        QbzSession.tr("Year (oldest)", QbzSession.trRev),
    ]
    readonly property var groupIds: ["off", "alpha", "artist"]
    readonly property var groupLabels: [
        QbzSession.tr("No grouping", QbzSession.trRev),
        QbzSession.tr("Group by letter", QbzSession.trRev),
        QbzSession.tr("Group by artist", QbzSession.trRev),
    ]
    // Tooltip leads for the icon-only controls.
    readonly property string sortTip: QbzSession.tr("Sort", QbzSession.trRev)
    readonly property string groupTip: QbzSession.tr("Grouping", QbzSession.trRev)

    readonly property var tracksSortIds: ["default", "artist-asc", "artist-desc",
        "title-asc", "title-desc", "year-desc", "year-asc", "added-desc"]
    readonly property var tracksSortLabels: [
        QbzSession.tr("Default", QbzSession.trRev),
        QbzSession.tr("Artist A-Z", QbzSession.trRev),
        QbzSession.tr("Artist Z-A", QbzSession.trRev),
        QbzSession.tr("Title A-Z", QbzSession.trRev),
        QbzSession.tr("Title Z-A", QbzSession.trRev),
        QbzSession.tr("Year (newest)", QbzSession.trRev),
        QbzSession.tr("Year (oldest)", QbzSession.trRev),
        QbzSession.tr("Date added", QbzSession.trRev),
    ]
    readonly property var tracksGroupIds: ["off", "album", "artist", "name"]
    readonly property var tracksGroupLabels: [
        QbzSession.tr("No grouping", QbzSession.trRev),
        QbzSession.tr("By album", QbzSession.trRev),
        QbzSession.tr("By artist", QbzSession.trRev),
        QbzSession.tr("By name", QbzSession.trRev),
    ]

    height: 30
    spacing: 8

    // ============================ ALBUMS =================================
    Row {
        visible: root.view && root.view.activeTab === "albums"
            && (root.view.albums.length > 0 || root.view.albumsSearch !== ""
                || QbzLocal.localAlbumsLoading
                || QbzLocal.localAlbumsNativeActive)
        spacing: 8
        height: parent.height

        LocalSearchBox {
            id: albumsSearchBox
            anchors.verticalCenter: parent.verticalCenter
            placeholder: QbzSession.tr("Search", QbzSession.trRev)
            text: root.view ? root.view.albumsSearch : ""
            // DEBOUNCED, like the Tracks box below — which had it and this one
            // did not, in the same file.
            //
            // `albumsSearch` is not a query, it is the FILTER INPUT: writing it
            // re-filters 1267 albums, rebuilds the collection's entry list, and
            // hands the ListView a new model — which tears down every delegate
            // and builds them again. Per keystroke.
            //
            // The owner's report names the shape exactly: "the first two
            // keystrokes hurt", i.e. while the filter still matches hundreds.
            // By the time it matches ten there is nothing left to rebuild, so
            // the tail of the word feels fine. That is a model swap, not a
            // slow filter.
            //
            // The box keeps showing what was typed immediately; only the
            // filter waits.
            property string pending: root.view ? root.view.albumsSearch : ""
            onEdited: function (v) {
                albumsSearchBox.pending = v
                albumsSearchDebounce.restart()
            }
            Timer {
                id: albumsSearchDebounce
                interval: 200
                onTriggered: root.view.albumsSearch = albumsSearchBox.pending
            }
        }
        LocalIconSelect {
            anchors.verticalCenter: parent.verticalCenter
            iconName: "arrow-up-down"
            label: root.sortTip
            menuWidth: 190
            options: root.sortLabels
            currentIndex: Math.max(0, root.sortIds.indexOf(root.view.albumsSort))
            onSelected: function (i) { root.view.albumsSort = root.sortIds[i] }
        }
        LocalIconSelect {
            anchors.verticalCenter: parent.verticalCenter
            iconName: "layers"
            label: root.groupTip
            menuWidth: 180
            options: root.groupLabels
            marked: root.view.albumsGroup !== "off"
            currentIndex: Math.max(0, root.groupIds.indexOf(root.view.albumsGroup))
            onSelected: function (i) { root.view.albumsGroup = root.groupIds[i] }
        }
        // Album identity ("Albums by") — what one album IS. A QUERY change
        // (the grouping happens in SQL), so it reloads the set; persisted,
        // shared with the Slint app. Kept as a LABELLED select: it changes
        // what the tab shows, not how it is ordered, and no icon says that.
        QbzSelect {
            menuWidth: 190
            height: 30
            anchors.verticalCenter: parent.verticalCenter
            options: [
                QbzSession.tr("Albums by folder", QbzSession.trRev),
                QbzSession.tr("Albums by metadata", QbzSession.trRev),
            ]
            currentIndex: QbzLocal.localAlbumMode === "metadata" ? 1 : 0
            onSelected: function (i) {
                QbzLocal.setAlbumMode(i === 1 ? "metadata" : "folder")
            }
        }
        LocalFilterButton {
            anchors.verticalCenter: parent.verticalCenter
            view: root.view
                ownerKey: "local-albums-filter"
        }
        QbzSegToggle {
            anchors.verticalCenter: parent.verticalCenter
            segments: [{ "id": "grid", "icon": "layout-grid" },
                       { "id": "list", "icon": "list" }]
            mode: root.view.albumsView
            onSetMode: function (v) { root.view.albumsView = v }
        }
        QbzNavButton {
            anchors.verticalCenter: parent.verticalCenter
            name: "square-check-big"
            onClicked: root.view.toggleAlbumsMultiSelect()
        }
    }

    // ============================ TRACKS =================================
    Row {
        visible: root.view && root.view.activeTab === "tracks"
            && ((QbzLocal.localTracksNativeActive
                    ? QbzLocal.localTracksNativeTotal > 0
                    : root.view.tracks.length > 0)
                || root.view.tracksSearch !== "")
        spacing: 8
        height: parent.height

        LocalSearchBox {
            anchors.verticalCenter: parent.verticalCenter
            placeholder: QbzSession.tr("Search", QbzSession.trRev)
            text: root.view ? root.view.tracksSearch : ""
            onEdited: function (v) {
                root.view.tracksSearch = v
                tracksSearchDebounce.restart()
            }
            // Server-side search: debounce so a keystroke is not a query.
            Timer {
                id: tracksSearchDebounce
                interval: 250
                onTriggered: QbzLocal.tracksSearch(root.view.tracksSearch)
            }
        }
        LocalIconSelect {
            anchors.verticalCenter: parent.verticalCenter
            iconName: "arrow-up-down"
            label: root.sortTip
            menuWidth: 190
            options: root.tracksSortLabels
            currentIndex: Math.max(0, root.tracksSortIds.indexOf(QbzLocal.localTracksSort))
            onSelected: function (i) { QbzLocal.tracksSetSort(root.tracksSortIds[i]) }
        }
        LocalIconSelect {
            anchors.verticalCenter: parent.verticalCenter
            iconName: "layers"
            label: root.groupTip
            menuWidth: 180
            options: root.tracksGroupLabels
            marked: root.view.tracksGroup !== "off"
            currentIndex: Math.max(0, root.tracksGroupIds.indexOf(root.view.tracksGroup))
            // Through the bridge, like the sort above and the album identity
            // below it: the choice is PERSISTED (locallibrary_ui.json, shared
            // with the Slint build). The legacy reader reorders its loaded
            // rows; Phase E resets an immutable SQL descriptor so grouping is
            // global across every keyset page.
            onSelected: function (i) { QbzLocal.tracksSetGroup(root.tracksGroupIds[i]) }
        }
        LocalFilterButton {
            anchors.verticalCenter: parent.verticalCenter
            view: root.view
            ownerKey: "local-tracks-filter"
        }
        // Multi-select LAST — never to the LEFT of a search box. The expandable
        // field keeps a 30px footprint and grows its inner box LEFT
        // (QbzLineEdit.qml:157-161, `x: root.width - width`), so anything
        // positioned before it is covered while the search is open and cannot
        // be clicked. This button used to lead the row and was exactly that.
        // Same slot the Albums group puts it in, so the two tabs also agree.
        QbzNavButton {
            anchors.verticalCenter: parent.verticalCenter
            name: "square-check-big"
            onClicked: root.view.toggleTracksMultiSelect()
        }
    }

    // ============================ FOLDERS ================================
    Row {
        visible: root.view && root.view.activeTab === "folders"
        spacing: 8
        height: parent.height

        // Flat-only controls — the tree rail carries its own search.
        Row {
            visible: root.view.foldersMode === "flat"
                && (root.view.folders.length > 0 || root.view.foldersSearch !== "")
            spacing: 8
            height: parent.height
            LocalSearchBox {
                anchors.verticalCenter: parent.verticalCenter
                placeholder: QbzSession.tr("Search", QbzSession.trRev)
                text: root.view ? root.view.foldersSearch : ""
                onEdited: function (v) { root.view.foldersSearch = v }
            }
            LocalIconSelect {
                anchors.verticalCenter: parent.verticalCenter
                iconName: "arrow-up-down"
                label: root.sortTip
                menuWidth: 170
                options: root.sortLabels
                currentIndex: Math.max(0, root.sortIds.indexOf(root.view.foldersSort))
                onSelected: function (i) { root.view.foldersSort = root.sortIds[i] }
            }
            LocalIconSelect {
                anchors.verticalCenter: parent.verticalCenter
                iconName: "layers"
                label: root.groupTip
                menuWidth: 180
                options: root.groupLabels
                marked: root.view.foldersGroup !== "off"
                currentIndex: Math.max(0, root.groupIds.indexOf(root.view.foldersGroup))
                onSelected: function (i) { root.view.foldersGroup = root.groupIds[i] }
            }
            QbzSegToggle {
                anchors.verticalCenter: parent.verticalCenter
                segments: [{ "id": "grid", "icon": "layout-grid" },
                           { "id": "list", "icon": "list" }]
                mode: root.view.foldersGridView
                onSetMode: function (v) { root.view.foldersGridView = v }
            }
        }
        // Flat / Tree — always visible on the Folders tab.
        // ASSET GAP: Slint uses disc-album / folder-tree; neither glyph is
        // baked in the Qt icon set yet (see GLUE), so this uses the closest
        // shipped pair.
        QbzSegToggle {
            anchors.verticalCenter: parent.verticalCenter
            segments: [{ "id": "flat", "icon": "disc" },
                       { "id": "tree", "icon": "folder-open" }]
            mode: root.view.foldersMode
            onSetMode: function (v) { root.view.foldersMode = v }
        }
    }

    // ============================ ARTISTS ================================
    Row {
        visible: root.view && root.view.activeTab === "artists"
            && ((QbzLocal.localArtistsNativeActive
                    ? QbzLocal.localArtistsNativeTotal > 0
                    : root.view.artists.length > 0)
                || root.view.artistsSearch !== ""
                || QbzLocal.localArtistsLoading)
        spacing: 8
        height: parent.height
        LocalSearchBox {
        anchors.verticalCenter: parent.verticalCenter
        placeholder: QbzSession.tr("Search artists", QbzSession.trRev)
            text: root.view ? root.view.artistsSearch : ""
            property string pending: root.view ? root.view.artistsSearch : ""
        onEdited: function (v) {
            pending = v
            artistsSearchDebounce.restart()
        }
        Timer {
            id: artistsSearchDebounce
            interval: 200
            onTriggered: root.view.artistsSearch = parent.pending
        }
    }
        LocalIconSelect {
            anchors.verticalCenter: parent.verticalCenter
            iconName: "arrow-up-down"
            label: root.sortTip
            menuWidth: 180
            options: [
                QbzSession.tr("Name A-Z", QbzSession.trRev),
                QbzSession.tr("Name Z-A", QbzSession.trRev),
                QbzSession.tr("Year (newest)", QbzSession.trRev),
                QbzSession.tr("Year (oldest)", QbzSession.trRev),
            ]
            currentIndex: Math.max(0, ["name-asc", "name-desc", "year-desc", "year-asc"]
                .indexOf(root.view.artistsSort))
            onSelected: function(i) {
                root.view.artistsSort = ["name-asc", "name-desc", "year-desc", "year-asc"][i]
            }
        }
        LocalFilterButton {
            anchors.verticalCenter: parent.verticalCenter
            view: root.view
            ownerKey: "local-artists-filter"
        }
    }

    // ============================ GENRES =================================
    Row {
        visible: root.view && root.view.activeTab === "genres"
        spacing: 8
        height: parent.height
        LocalIconSelect {
            anchors.verticalCenter: parent.verticalCenter
            iconName: "layout-grid"
            label: QbzSession.tr("Explorer columns", QbzSession.trRev)
            menuWidth: 190
            options: [
                QbzSession.tr("Genre", QbzSession.trRev),
                QbzSession.tr("Year", QbzSession.trRev),
                QbzSession.tr("Genre and year", QbzSession.trRev),
            ]
            currentIndex: Math.max(0, ["genre", "year", "both"]
                .indexOf(root.view.explorerColumns))
            onSelected: function(i) {
                root.view.setExplorerColumns(["genre", "year", "both"][i])
            }
        }
        LocalIconSelect {
            anchors.verticalCenter: parent.verticalCenter
            iconName: "arrow-up-down"
            label: root.sortTip
            menuWidth: 180
            options: [
                QbzSession.tr("Name A-Z", QbzSession.trRev),
                QbzSession.tr("Name Z-A", QbzSession.trRev),
                QbzSession.tr("Year (newest)", QbzSession.trRev),
                QbzSession.tr("Year (oldest)", QbzSession.trRev),
            ]
            currentIndex: Math.max(0, ["title-asc", "title-desc", "year-desc", "year-asc"]
                .indexOf(root.view.genresSort))
            onSelected: function(i) {
                root.view.genresSort = ["title-asc", "title-desc", "year-desc", "year-asc"][i]
            }
        }
        LocalFilterButton {
            anchors.verticalCenter: parent.verticalCenter
            view: root.view
            ownerKey: "local-genres-filter"
        }
        QbzSegToggle {
            anchors.verticalCenter: parent.verticalCenter
            segments: [{ "id": "grid", "icon": "layout-grid" },
                       { "id": "list", "icon": "list" },
                       { "id": "details", "icon": "rows-3" }]
            mode: root.view.genresView
            onSetMode: function(v) { root.view.genresView = v }
        }
    }
}
