// Local Library > Albums, kiosk body.
//
// THE FIX THIS FILE CARRIES. The previous kiosk Local Library read
// `QbzLocal.localAlbumsJson` unconditionally. That document is only published
// by the LEGACY reader: once the paged catalog surface is live
// (`localAlbumsNativeActive`, the production default) Rust stops republishing
// it, so the kiosk rendered whatever stale array it happened to hold — an
// empty grid on a machine whose library is fine. This body consumes the SAME
// two readers the desktop tab does, and the native one is authoritative
// whenever it is active.
//
// It owns exactly three things: the native query descriptor, the model's
// `pageMiss` answer, and the tab's loading/error/empty chrome. Everything
// visual is KioskLocalAlbumGrid.
//
// NO SEARCH / SORT / FILTER UI. The kiosk shell has none, so the descriptor is
// the neutral one (empty search, the desktop default sort, no grouping, no
// funnel) and the grid never re-queries on user input — only on a column
// change, which is a geometry event.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    property var view: null

    // The native reader knows no favorites (LocalLibraryView.qml:341-344):
    // under "Favorites only" the host loads the legacy document instead.
    readonly property bool nativeActive: QbzLocal.localAlbumsNativeActive
        && !(root.view && root.view.favoriteOnly)
    readonly property var nativeModel: QbzLocalAlbums
    readonly property int albumTotal: root.nativeActive
        ? (QbzLocal.localAlbumsNativeTotal || 0)
        : (root.view ? root.view.albums.length : 0)

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    // ---------------------------------------------------------------------
    // Native query — the descriptor is width-dependent (Rust chunks the rows
    // into `columns` cards), so it is (re)issued whenever the grid settles on
    // a new column count. Coalesced: a mount that resizes twice queries once.
    // ---------------------------------------------------------------------
    Timer {
        id: queryCoalescer
        interval: 0
        repeat: false
        onTriggered: root.resetNativeQuery()
    }
    function resetNativeQuery() {
        if (grid.columns <= 0)
            return
        QbzLocal.albumsNativeReset("", "artist-asc", "off",
                                   root.view ? root.view.filterJson : "{}", grid.columns)
    }
    Component.onCompleted: {
        queryCoalescer.restart()
        root.publishNav()
    }

    // The mounted body owns the nav geometry: only it knows its column count
    // and how many items the focus ring can reach.
    function publishNav() {
        if (root.view)
            root.view.publishNav(Math.max(1, grid.columns), root.albumTotal)
    }
    onAlbumTotalChanged: root.publishNav()

    Connections {
        target: grid
        function onColumnsChanged() {
            queryCoalescer.restart()
            root.publishNav()
        }
    }
    Connections {
        target: QbzLocal
        // Album identity is a query, not a filter: flipping it invalidates the
        // descriptor exactly as it does on the desktop tab.
        function onLocalAlbumModeChanged() { queryCoalescer.restart() }
    }
    // The funnel IS part of the descriptor: a chip toggled in the host's
    // sheet re-issues the native query.
    Connections {
        target: root.view
        function onFilterJsonChanged() { queryCoalescer.restart() }
    }

    // The model only asks for a page when this signal has a receiver
    // (LocalAlbumsModel::requestPage bails on `!isSignalConnected`), so the
    // subscription belongs to the MOUNTED tab and to nothing else.
    Connections {
        target: root.nativeModel
        function onPageMiss(page, generation) {
            QbzLocal.albumsNativePageMiss(page, generation)
        }
    }
    // Covers resolve id-keyed; the native page holds `artPath` per row, so a
    // resolved key has to reach the model or the resident rows never show it.
    Connections {
        target: QbzLocal
        function onLocalArtworkReady(key, path) {
            root.nativeModel.setArtwork(key, path)
        }
    }

    KioskLocalAlbumGrid {
        id: grid
        anchors.fill: parent
        visible: !QbzLocal.localAlbumsLoading
            && QbzLocal.localAlbumsError === ""
            && root.albumTotal > 0
        view: root.view
        surface: "albums"
        scrollScope: "local:albums"
        pad: root.view ? root.view.pad : 16
        nativeActive: root.nativeActive
        nativeModel: root.nativeModel
        rows: root.nativeActive ? [] : (root.view ? root.view.albums : [])
        onOpen: function (id) {
            if (root.view)
                root.view.openAlbum(id)
        }
    }

    // Loading / error / empty. One layer, static, mounts no artwork and no
    // model; it unmounts itself entirely once the grid is populated.
    KioskSkeleton {
        anchors.fill: parent
        kind: "grid"
        columns: Math.max(2, grid.columns)
        pad: grid.pad
        gap: grid.gap
        loading: QbzLocal.localAlbumsLoading
        error: QbzLocal.localAlbumsError
        empty: !QbzLocal.localAlbumsLoading
            && QbzLocal.localAlbumsError === ""
            && root.albumTotal === 0
        emptyText: root.t("No albums in your local library yet.")
    }
}
