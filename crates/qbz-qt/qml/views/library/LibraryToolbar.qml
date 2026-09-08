// LibraryToolbar — the Library view's fixed chrome row: the six-tab menu on
// the left, the per-tab ACTION cluster beside it, and the per-tab controls on
// the right (FavoritesView.slint:444-942, rows 1 and 2 folded into one 56px
// band because this port's header carries no "Library" title).
//
// EXTRACTED from views/LibraryView.qml (track rule 2 — the file was 1,881
// lines). Everything it reads and writes goes through `view`, the LibraryView
// instance, exactly as views/local/LocalToolbar.qml does for LocalLibraryView.
// Toolbar choices are written through `view.setPref(...)` rather than straight
// onto the property, because they PERSIST (library_prefs.rs -> the same
// favorites_ui.json the Slint build writes).

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root

    /// The LibraryView root.
    property var view: null

    QbzTheme { id: theme }

    height: 56
    // The width here is the REAL content pane after the navigation and
    // Queue/Lyrics columns have taken their space.  At this breakpoint the
    // six-tab strip and the All controls fit in one row without either side
    // eating the other.
    readonly property bool compactChrome: width < 1040

    // Small toolbar toggle button (ToggleButton sm): 30px, active = accent.
    component ToolToggle: Rectangle {
        property string name: ""
        property bool active: false
        signal clicked()
        width: 30
        height: 30
        radius: 6
        color: active ? theme.surfaceElevated
             : ttArea.containsMouse ? theme.surfaceHover : "transparent"
        QbzIcon {
            name: parent.name
            width: 16
            height: 16
            anchors.centerIn: parent
            tintName: parent.active ? "accent"
                   : ttArea.containsMouse ? "textPrimary" : "secondary"
        }
        MouseArea {
            id: ttArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: parent.clicked()
        }
    }

    // Filter-by-genre trigger (FavoritesView.slint's FavGenreButton, sm):
    // accent fill + "N genres" while the "library-all" selection is active.
    component GenreToolButton: Rectangle {
        id: gtb
        readonly property bool active: root.view.genreCount > 0
        width: root.compactChrome ? 30 : gtbRow.implicitWidth
        height: 30
        radius: 6
        color: gtb.active ? theme.accent
             : gtbArea.containsMouse ? theme.surfaceHover : theme.surfaceElevated
        Row {
            id: gtbRow
            anchors.centerIn: parent
            height: parent.height
            leftPadding: root.compactChrome ? 0 : 10
            rightPadding: root.compactChrome ? 0 : 12
            spacing: root.compactChrome ? 0 : 7
            QbzIcon {
                name: "list-filter"
                width: 13
                height: 13
                anchors.verticalCenter: parent.verticalCenter
                // Same pair-of-halves fix as controls/BrowseGenreButton.qml:
                // the glyph said "primary" (legacy alias of a literal
                // #ffffff) next to an accent-text label, i.e. a white glyph
                // beside a black label on the pale-accent themes.
                // favorites/FavoritesView.slint:301 + :309 are consistently
                // #ffffff, but that white is 1.70:1 on high-contrast, 1.74
                // on ikari, 1.82 on wcag-dark and under 2.6:1 on 16 of the
                // 35 palettes — deliberate divergence from both lines.
                // theme/QbzTheme.qml, "ON AN ACCENT FILL".
                tintName: gtb.active ? theme.accentGlyphTint : "secondary"
            }
            Text {
                visible: !root.compactChrome
                anchors.verticalCenter: parent.verticalCenter
                text: root.view.genreCount === 0
                    ? QbzSession.tr("Filter by genre", QbzSession.trRev)
                    : root.view.genreCount === 1
                        ? QbzSession.tr("1 genre", QbzSession.trRev)
                        : QbzSession.tr("{} genres", QbzSession.trRev)
                            .replace("{}", root.view.genreCount)
                // The colour twin of the glyph's tint above — accent-text on
                // 34 of the 35 palettes.
                color: gtb.active ? theme.accentGlyphColor : theme.textSecondary
                font.pixelSize: 12
            }
        }
        MouseArea {
            id: gtbArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: {
                libFilterTip.exit()
                root.view.toggleGenrePopup()
            }
            onEntered: libFilterTip.enter()
            onExited: libFilterTip.exit()
        }

        // The genre chip is the toolbar's filter affordance, so it carries the
        // whole bar's summary: the genres it owns, plus the source switches
        // that live next to it and are just as invisible once set.
        QbzFilterTip {
            id: libFilterTip
            ownerKey: "library-all-filter"
            anchor: gtb
            groups: {
                var out = []
                var names = (root.view.genreDoc.names || {})["library-all"] || []
                if (names.length > 0)
                    out.push({ group: QbzSession.tr("Genre", QbzSession.trRev),
                               values: names })
                var ex = root.view.filterSummaryGroups || []
                for (var i = 0; i < ex.length; i++)
                    out.push(ex[i])
                return out
            }
        }
    }

    // --- Group-by selects (FavoritesView.slint:604-670, 760-775).
    // One control, three tabs: each carries its own option set and writes its
    // own pref. The reference draws a QbzSelect per tab; this is the same menu
    // shape the sort popups beside it already use, so the toolbar stays
    // visually of a piece.
    component GroupSelect: Rectangle {
        id: gsRoot
        property var options: []      // [{value, label}]
        property string current: "off"
        signal picked(string value)
        visible: false
        width: gsRow.width
        height: 30
        radius: 6
        color: gsArea.containsMouse ? theme.surfaceHover : theme.surfaceElevated
        readonly property string currentLabel: {
            for (var i = 0; i < options.length; i++)
                if (options[i].value === current) return options[i].label
            return ""
        }
        Row {
            id: gsRow
            height: parent.height
            leftPadding: 10
            rightPadding: 10
            spacing: 6
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: gsRoot.currentLabel
                color: theme.textSecondary
                font.pixelSize: 12
            }
            QbzIcon {
                name: "chevron-down"
                width: 12
                height: 12
                anchors.verticalCenter: parent.verticalCenter
                tintName: "secondary"
            }
        }
        MouseArea {
            id: gsArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: gsMenu.openBelowRight(gsArea)
        }
        QbzContextMenu {
            id: gsMenu
            menuWidth: 156
            Repeater {
                model: gsRoot.options
                delegate: Rectangle {
                    required property var modelData
                    width: parent ? parent.width : 0
                    height: 33
                    radius: 5
                    color: gsOptArea.containsMouse ? theme.surfaceHover : "transparent"
                    Text {
                        anchors.fill: parent
                        anchors.leftMargin: 8
                        text: modelData.label
                        color: theme.textSecondary
                        font.pixelSize: 13
                        font.weight: gsRoot.current === modelData.value
                            ? theme.weightSemibold : theme.weightRegular
                        verticalAlignment: Text.AlignVCenter
                        elide: Text.ElideRight
                    }
                    MouseArea {
                        id: gsOptArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: { gsRoot.picked(modelData.value); gsMenu.close() }
                    }
                }
            }
        }
    }

    // FavTabMenu (SegmentedTabBar with count badges).
    Rectangle {
        x: 32
        y: 25 - height / 2
        width: tabRow.width
        height: tabRow.height
        color: theme.surfaceElevated
        radius: 6
        Row {
            id: tabRow
            padding: 3
            spacing: 4
            QbzTabBar {
                counts: !root.compactChrome
                underline: true
                activeId: root.view.activeTab
                tabs: [
                    { "id": "all", "label": QbzSession.tr("All", QbzSession.trRev), "count": root.view.counts.all || 0 },
                    { "id": "tracks", "label": QbzSession.tr("Tracks", QbzSession.trRev), "count": root.view.tabTotals.tracks || 0 },
                    { "id": "albums", "label": QbzSession.tr("Albums", QbzSession.trRev), "count": root.view.tabTotals.albums || 0 },
                    { "id": "artists", "label": QbzSession.tr("Artists", QbzSession.trRev), "count": root.view.counts.artists || 0 },
                    { "id": "playlists", "label": QbzSession.tr("Playlists", QbzSession.trRev), "count": root.view.counts.playlists || 0 },
                    { "id": "labels", "label": QbzSession.tr("Labels", QbzSession.trRev), "count": root.view.counts.labels || 0 },
                ]
                onSelected: function (id) { root.view.activeTab = id }
            }
        }
    }

    // --- Per-tab ACTION cluster (FavoritesView.slint:462-565) -------------
    // The reference draws these in its own row, to the right of a "Library"
    // title this port's header does not have; they sit beside the tab bar
    // instead. FUNCTION is 1:1 — this is issue #554 ("users reported the
    // feature as removed"), so what matters is that Play all / Shuffle / the
    // per-tab random / the multi-select toggles exist and work.
    //
    // Each gates on its tab having something to act on, exactly as the
    // reference's `*-visible.length > 0` guards do: an action that would play
    // nothing is not drawn.
    Row {
        id: actionRow
        x: 32 + tabRow.width + 12
        y: 25 - height / 2
        height: 30
        spacing: 8

        function randomVisibleId(kind) {
            var pool = []
            var rows = root.view.visibleRows
            for (var i = 0; i < rows.length; i++)
                if (rows[i].kind === kind) pool.push(rows[i])
            if (pool.length === 0) return null
            return pool[Math.floor(Math.random() * pool.length)]
        }

        // Tracks — play all / shuffle all / select multiple.
        QbzIconButton {
            visible: root.view.activeTab === "tracks" && root.view.tabHasItems
            btnSize: 30
            name: "play-fill"
            onClicked: QbzLibrary.libraryPlayAll(
                JSON.stringify(root.view.visibleTrackIds()), false)
        }
        QbzIconButton {
            visible: root.view.activeTab === "tracks" && root.view.tabHasItems
            btnSize: 30
            name: "shuffle"
            onClicked: QbzLibrary.libraryPlayAll(
                JSON.stringify(root.view.visibleTrackIds()), true)
        }
        ToolToggle {
            visible: root.view.activeTab === "tracks" && root.view.tabHasItems
            name: "square-check-big"
            active: root.view.tracksMultiSelect
            onClicked: root.view.setTracksMultiSelect(!root.view.tracksMultiSelect)
        }
        // Albums — play a RANDOM album (the reference's `albums_shuffle`,
        // which picks one visible album, not a shuffled play of every album).
        QbzIconButton {
            visible: root.view.activeTab === "albums" && root.view.tabHasItems
            btnSize: 30
            name: "shuffle"
            onClicked: {
                var pick = actionRow.randomVisibleId("album")
                if (pick) QbzPlayer.playAlbum(pick.id)
            }
        }
        // Albums multi-select — LIST mode ONLY (owner 2026-07-24,
        // FavoritesView.slint:512-523): the grid card has no checkbox slot.
        ToolToggle {
            visible: root.view.activeTab === "albums" && root.view.tabHasItems
                && root.view.albumsView === "list"
            name: "square-check-big"
            active: root.view.albumsMultiSelect
            onClicked: root.view.setAlbumsMultiSelect(!root.view.albumsMultiSelect)
        }
        // Artists — open a random artist.
        QbzIconButton {
            visible: root.view.activeTab === "artists" && root.view.tabHasItems
            btnSize: 30
            name: "shuffle"
            onClicked: {
                var pick = actionRow.randomVisibleId("artist")
                if (pick) QbzArtist.openArtist(pick.id)
            }
        }
        // Playlists — play a random playlist.
        QbzIconButton {
            visible: root.view.activeTab === "playlists" && root.view.tabHasItems
            btnSize: 30
            name: "shuffle"
            onClicked: {
                var pick = actionRow.randomVisibleId("playlist")
                if (pick) QbzBridge.openPlaylist(pick.id)
            }
        }
        // Labels — open a random label's landing.
        QbzIconButton {
            visible: root.view.activeTab === "labels" && root.view.tabHasItems
            btnSize: 30
            name: "shuffle"
            onClicked: {
                var pick = actionRow.randomVisibleId("label")
                if (pick) QbzHome.openLabel(pick.id)
            }
        }
    }

    // Per-tab controls.
    Row {
        x: parent.width - width - 32
        y: 25 - height / 2
        height: 30
        spacing: 8

        // ===== All toolbar =====
        Row {
            visible: root.view.activeTab === "all"
            spacing: 8
            height: parent.height
            // FavoritesView.slint:801-813 — collapsed magnifier that
            // opens LEFT over the tab bar; the All tab is the only
            // one that debounces after the keystroke.
            QbzLineEdit {
                searchMode: true
                expandable: true
                sm: true
                elevated: false          // ExpandableSearch fill = surface-card
                openWidth: 196           // = max-open-width
                placeholder: QbzSession.tr("Search your library", QbzSession.trRev)
                onEdited: function (v) { root.view.setAllSearch(v) }
            }
            // Source switches (tooltips are the Slint copies).
            ToolToggle { name: "shopping-bag"; active: root.view.showPurchases; onClicked: root.view.setShowPurchases(!root.view.showPurchases) }
            ToolToggle { name: "heart"; active: root.view.showFavorites; onClicked: root.view.setShowFavorites(!root.view.showFavorites) }
            ToolToggle { name: "user-plus"; active: root.view.showFollowing; onClicked: root.view.setShowFollowing(!root.view.showFollowing) }
            // The ONE source switch the reference persists (favorites_prefs.rs
            // `all_show_local`) — the other three are session-local there too.
            ToolToggle {
                name: "hard-drive"
                active: root.view.showLocal
                onClicked: root.view.setShowLocal(!root.view.showLocal)
            }
            // Filter by genre — shared popup, own "library-all"
            // context (FavoritesView.slint:864).
            GenreToolButton { }
            // Grid / list toggle.
            ToolToggle {
                name: root.view.viewMode === "list" ? "layout-grid" : "list"
                active: false
                onClicked: root.view.viewMode = root.view.viewMode === "list" ? "grid" : "list"
            }
            // Sort menu (PlaylistView-style: field + direction caret).
            Rectangle {
                width: root.compactChrome ? 40 : allSortRow.implicitWidth
                height: 30
                radius: 6
                color: allSortArea.containsMouse ? theme.surfaceHover : theme.surfaceElevated
                Row {
                    id: allSortRow
                    anchors.centerIn: parent
                    height: parent.height
                    leftPadding: root.compactChrome ? 8 : 10
                    rightPadding: root.compactChrome ? 8 : 10
                    spacing: root.compactChrome ? 0 : 6
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.compactChrome ? "A-Z"
                            : QbzSession.tr("Sort", QbzSession.trRev) + ": " + (
                            root.view.sortBy === "title" ? QbzSession.tr("Title", QbzSession.trRev)
                            : root.view.sortBy === "artist" ? QbzSession.tr("Artist", QbzSession.trRev)
                            : QbzSession.tr("Date added", QbzSession.trRev))
                        color: theme.textSecondary
                        font.pixelSize: 12
                    }
                    QbzIcon {
                        visible: !root.compactChrome
                        name: root.view.sortAsc ? "chevron-up" : "chevron-down"
                        width: 12
                        height: 12
                        anchors.verticalCenter: parent.verticalCenter
                        tintName: "secondary"
                    }
                }
                MouseArea {
                    id: allSortArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: allSortMenu.openBelowRight(allSortArea)
                }
                QbzContextMenu {
                    id: allSortMenu
                    menuWidth: 172
                    Repeater {
                        model: [
                            { "field": "date", "label": QbzSession.tr("Date added", QbzSession.trRev) },
                            { "field": "title", "label": QbzSession.tr("Title", QbzSession.trRev) },
                            { "field": "artist", "label": QbzSession.tr("Artist", QbzSession.trRev) },
                        ]
                        delegate: Rectangle {
                            required property var modelData
                            width: parent ? parent.width : 0
                            height: 33
                            radius: 5
                            color: sortOptArea.containsMouse ? theme.surfaceHover : "transparent"
                            Row {
                                anchors.fill: parent
                                anchors.leftMargin: 8
                                spacing: 6
                                Text {
                                    width: parent.width - 26
                                    height: parent.height
                                    text: modelData.label
                                    color: theme.textSecondary
                                    font.pixelSize: 13
                                    font.weight: root.view.sortBy === modelData.field ? theme.weightSemibold : theme.weightRegular
                                    verticalAlignment: Text.AlignVCenter
                                }
                                QbzIcon {
                                    visible: root.view.sortBy === modelData.field
                                    name: root.view.sortAsc ? "chevron-up" : "chevron-down"
                                    width: 12
                                    height: 12
                                    anchors.verticalCenter: parent.verticalCenter
                                    tintName: "accent"
                                }
                            }
                            MouseArea {
                                id: sortOptArea
                                anchors.fill: parent
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    // Re-pick flips direction; a new field
                                    // resets to its natural default.
                                    if (root.view.sortBy === modelData.field) {
                                        root.view.sortAsc = !root.view.sortAsc
                                    } else {
                                        root.view.sortBy = modelData.field
                                        root.view.sortAsc = modelData.field !== "date"
                                    }
                                    allSortMenu.close()
                                }
                            }
                        }
                    }
                }
            }
        }

        // ===== Other-tab toolbars =====
        // ONE search instance for the five tabs: the reference clears the
        // query on every tab entry (see LibraryView.onActiveTabChanged), so
        // per-tab instances would only implement a persistence that does not
        // exist. A Row skips `visible: false` children, so the 30px slot
        // appears/disappears cleanly and, because the ROOT width never leaves
        // collapsedSize while the field opens, nothing to its right ever moves.
        QbzLineEdit {
            id: tabSearchBox
            objectName: "libraryTabSearch"
            visible: root.view.activeTab !== "all" && root.view.tabHasItems
            searchMode: true
            expandable: true
            sm: true
            elevated: false
            openWidth: 196
            placeholder: QbzSession.tr("Search", QbzSession.trRev)
            onEdited: function (v) { root.view.tabSearch = v }
        }

        // Tracks / Albums source switches. They deliberately sit between the
        // fixed 30px search slot and Filter by genre: ExpandableSearch grows
        // left, so opening it can never cover or push either toggle.
        ToolToggle {
            visible: (root.view.activeTab === "tracks" || root.view.activeTab === "albums")
                && root.view.tabHasItems
            name: "shopping-bag"
            active: root.view.showPurchases
            onClicked: root.view.setShowPurchases(!root.view.showPurchases)
        }
        ToolToggle {
            visible: (root.view.activeTab === "tracks" || root.view.activeTab === "albums")
                && root.view.tabHasItems
            name: "heart"
            active: root.view.showFavorites
            onClicked: root.view.setShowFavorites(!root.view.showFavorites)
        }

        // Filter by genre — the SAME shared popup the All toolbar uses. The
        // reference draws it on Tracks and Albums too (FavoritesView.slint:
        // 609, :652); this port had it on All only, so the two biggest tabs
        // could not be filtered at all.
        GenreToolButton {
            visible: (root.view.activeTab === "tracks" || root.view.activeTab === "albums")
                && root.view.tabHasItems
        }
        GroupSelect {
            visible: root.view.activeTab === "tracks" && root.view.tabHasItems
            current: root.view.tracksGroup
            options: [
                { "value": "off", "label": QbzSession.tr("Group: Off", QbzSession.trRev) },
                { "value": "album", "label": QbzSession.tr("Group: Album", QbzSession.trRev) },
                { "value": "artist", "label": QbzSession.tr("Group: Artist", QbzSession.trRev) },
                { "value": "name", "label": QbzSession.tr("Group: Name", QbzSession.trRev) },
            ]
            onPicked: function (v) { root.view.setPref("tracksGroup", v) }
        }
        GroupSelect {
            visible: root.view.activeTab === "albums" && root.view.tabHasItems
            current: root.view.albumsGroup
            options: [
                { "value": "off", "label": QbzSession.tr("Group: Off", QbzSession.trRev) },
                { "value": "alpha", "label": QbzSession.tr("Group: A-Z", QbzSession.trRev) },
                { "value": "artist", "label": QbzSession.tr("Group: Artist", QbzSession.trRev) },
            ]
            onPicked: function (v) { root.view.setPref("albumsGroup", v) }
        }
        // Artists group (A-Z) — GRID mode only, like the reference
        // (FavoritesView.slint:763: "Tauri hides it in sidepanel"; the
        // sidepanel's left rail is ALWAYS A-Z grouped).
        GroupSelect {
            visible: root.view.activeTab === "artists" && root.view.tabHasItems
                && root.view.artistsView === "grid"
            current: root.view.artistsGroup
            options: [
                { "value": "off", "label": QbzSession.tr("Group: Off", QbzSession.trRev) },
                { "value": "alpha", "label": QbzSession.tr("Group: A-Z", QbzSession.trRev) },
            ]
            onPicked: function (v) { root.view.setPref("artistsGroup", v) }
        }

        // Albums sort popup (real, JS-side).
        Rectangle {
            visible: root.view.activeTab === "albums" && root.view.tabHasItems
            width: albumsSortRow.width
            height: 30
            radius: 6
            color: albumsSortArea.containsMouse ? theme.surfaceHover : theme.surfaceElevated
            Row {
                id: albumsSortRow
                height: parent.height
                leftPadding: 10
                rightPadding: 10
                spacing: 6
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: QbzSession.tr("Sort", QbzSession.trRev) + ": " + (
                        root.view.albumsSort === "title-asc" ? QbzSession.tr("Title A-Z", QbzSession.trRev)
                        : root.view.albumsSort === "title-desc" ? QbzSession.tr("Title Z-A", QbzSession.trRev)
                        : root.view.albumsSort === "artist-asc" ? QbzSession.tr("Artist A-Z", QbzSession.trRev)
                        : QbzSession.tr("Default", QbzSession.trRev))
                    color: theme.textSecondary
                    font.pixelSize: 12
                }
                QbzIcon { name: "chevron-down"; width: 12; height: 12; anchors.verticalCenter: parent.verticalCenter; tintName: "secondary" }
            }
            MouseArea {
                id: albumsSortArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: albumsSortMenu.openBelowRight(albumsSortArea)
            }
            QbzContextMenu {
                id: albumsSortMenu
                menuWidth: 172
                Repeater {
                    model: [
                        { "value": "default", "label": QbzSession.tr("Default", QbzSession.trRev) },
                        { "value": "title-asc", "label": QbzSession.tr("Title A-Z", QbzSession.trRev) },
                        { "value": "title-desc", "label": QbzSession.tr("Title Z-A", QbzSession.trRev) },
                        { "value": "artist-asc", "label": QbzSession.tr("Artist A-Z", QbzSession.trRev) },
                    ]
                    delegate: Rectangle {
                        required property var modelData
                        width: parent ? parent.width : 0
                        height: 33
                        radius: 5
                        color: abSortOptArea.containsMouse ? theme.surfaceHover : "transparent"
                        Text {
                            anchors.fill: parent
                            anchors.leftMargin: 8
                            text: modelData.label
                            color: theme.textSecondary
                            font.pixelSize: 13
                            font.weight: root.view.albumsSort === modelData.value ? theme.weightSemibold : theme.weightRegular
                            verticalAlignment: Text.AlignVCenter
                        }
                        MouseArea {
                            id: abSortOptArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                root.view.setPref("albumsSort", modelData.value)
                                albumsSortMenu.close()
                            }
                        }
                    }
                }
            }
        }
        // Albums grid / list toggle (FavoritesView.slint:697-705 ViewToggle).
        ToolToggle {
            visible: root.view.activeTab === "albums" && root.view.tabHasItems
            name: root.view.albumsView === "list" ? "layout-grid" : "list"
            active: false
            onClicked: root.view.setPref("albumsView",
                root.view.albumsView === "list" ? "grid" : "list")
        }
        // Playlists grid / list toggle (FavoritesView.slint:737-745).
        ToolToggle {
            visible: root.view.activeTab === "playlists" && root.view.tabHasItems
            name: root.view.playlistsView === "list" ? "layout-grid" : "list"
            active: false
            onClicked: root.view.setPref("playlistsView",
                root.view.playlistsView === "list" ? "grid" : "list")
        }
        // Artists grid / sidepanel toggle (FavoritesView.slint:780-793 —
        // `active: false` there too, "keeps the two icon states visually
        // identical").
        ToolToggle {
            visible: root.view.activeTab === "artists" && root.view.tabHasItems
            name: root.view.artistsView === "sidepanel" ? "layout-grid" : "list"
            active: false
            onClicked: root.view.setPref("artistsView",
                root.view.artistsView === "sidepanel" ? "grid" : "sidepanel")
        }
        // Playlists sub-tab (Library / Following).
        Rectangle {
            visible: root.view.activeTab === "playlists" && root.view.tabHasItems
            width: plSubRow.width
            height: 30
            radius: 6
            color: theme.surfaceElevated
            Row {
                id: plSubRow
                padding: 3
                spacing: 4
                Repeater {
                    model: [
                        { "id": "favorites", "label": QbzSession.tr("Library", QbzSession.trRev) },
                        { "id": "following", "label": QbzSession.tr("Following", QbzSession.trRev) },
                    ]
                    delegate: Rectangle {
                        required property var modelData
                        property bool active: root.view.playlistsSubTab === modelData.id
                        width: plSubText.implicitWidth + 20
                        height: 24
                        radius: 4
                        color: active ? theme.surfaceMain : "transparent"
                        Text {
                            id: plSubText
                            anchors.centerIn: parent
                            text: modelData.label
                            color: parent.active ? theme.textPrimary : theme.textMuted
                            font.pixelSize: 12
                            font.weight: theme.weightMedium
                        }
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.view.playlistsSubTab = modelData.id
                        }
                    }
                }
            }
        }
    }

    /// Collapse the shared per-tab search field (LibraryView calls this on
    /// every tab change — the reference destroys and re-creates it collapsed).
    function closeTabSearch() {
        if (tabSearchBox) tabSearchBox.closeSearch()
    }
}
