// Qobuz mix detail page (DailyQ / WeeklyQ / FavQ / TopQ) — QML port of
// mix/MixView.slint. Route "mix", opened from the Qobuz Mixes tiles on
// Discover > For You.
//
// ONE JSON document (QbzHome.mixJson, src/foryou_qt.rs MixDoc): kind, title,
// subtitle, trackCount, totalDuration, loading, tracks[]. The gradient is
// keyed off `kind` through the shared cards/MixArtwork.qml, so the page's
// artwork is literally the tile the user clicked, enlarged.
//
// SCROLLING-PAGE CONVENTION (AlbumView / LabelView / PlaylistView): one
// Flickable whose Column pads 32 / 32 / top 11 / bottom 100. The .slint's
// NavButtons row is dropped (back/forward live in the shell header,
// HeaderBar.qml:230) but its 22px spacer is KEPT, so the header sits where
// every other detail page's does.
//
// Numbers from MixView.slint: art 224 radius Radius.md · header gap 32 ·
// eyebrow 11px semibold letter-spacing 1.5 · title Typography.section bold ·
// subtitle Typography.body wrapped at `root.width - 320` · info line
// Typography.legal · spacers 6/8/10/20/28 · then the column header, a 3px
// gap, a 1px border-subtle rule, 6px, then the rows.
//
// The column header is rows/TrackListHeader.qml — NOT the .slint's numbers.
// MixView.slint:246-284 lays its header out with 16px gaps, a 40px "#", a
// 64px Duration and NO artwork reserve, while its rows are the shared
// TrackRow (14px gaps, 32px number, 36px artwork, 70px Duration): the labels
// there sit right of the columns they name, which is exactly the defect
// being fixed here. The port takes the ROW's geometry from
// rows/TrackCols.qml for both.
//
// POC-NOTEs, each deliberate and each with a precedent in this port:
// - NO multi-select bar — and the ORIGINAL reason is now STALE on both
//   halves: rows/TrackRow.qml:116 DOES have the `selectMode` arm, and
//   add-to-playlist DOES have a bridge seam (QbzPlaylistPicker.openForTracks,
//   src/playlist_picker_bridge.rs). What is still missing here is only the
//   selection STATE and the bar itself — LibraryView.qml's `tracksSelected`
//   map + qml/controls/MultiSelectBar-style host is the pattern to copy.
//   Until then the .slint header's select disc renders DIMMED and inert here
//   (the ArtistView / LabelView precedent). Per-ROW add-to-playlist and
//   add-to-mixtape are both LIVE on this page already (the shared TrackRow
//   menu).
// - The "Add to playlist" disc (list-plus) is inert in the .slint too — it
//   carries no `clicked` there (MixView.slint:182-188) — so it is inert
//   here, dimmed the same way, rather than silently absent.
// - Back/forward scroll restore is handled by ScrollMemory below.
// - CIRCLE ACTION SIZE: the .slint MixView declares a LOCAL CircleAction
//   fixed at 38px, predating the shared primitives/CircleAction.slint
//   (44 primary / 32 secondary). This port mounts the shared
//   controls/QbzCircleAction.qml — the port of the SHARED primitive — so the
//   mix page's discs match every other detail page in this build.

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz
import "../cards"
import "../controls"
import "../rows"
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
            return JSON.parse(QbzHome.mixJson)
        } catch (e) {
            return ({})
        }
    }
    readonly property var tracks: root.doc.tracks || []
    readonly property bool loading: root.doc.loading === true
    readonly property string kind: root.doc.kind || "daily"

    // --- skeleton pulse (the HomeView gating rule: freeze on NOT VISIBLE,
    // never on lost focus) --------------------------------------------------
    property bool skelPhase: false
    readonly property bool windowShowing: root.Window.window
        ? (root.Window.window.visibility !== Window.Minimized
           && root.Window.window.visibility !== Window.Hidden)
        : true
    Timer {
        interval: 900
        repeat: true
        running: root.visible && root.windowShowing && root.loading
        onTriggered: root.skelPhase = !root.skelPhase
    }

    // ============================ offline gate ============================
    QbzOfflinePlaceholder {
        visible: QbzSession.offline
        anchors.centerIn: parent
        showSettingsAction: true
        onSettingsClicked: QbzShell.navigateTo("settings")
    }

    // ============================ the page ================================
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

            // The .slint's NavButtons row is not drawn (shell header owns
            // back/forward) but its 22px spacer is kept.
            Item { width: 1; height: 22 }

            // --- Header -----------------------------------------------------
            Row {
                width: parent.width - 64
                spacing: 32

                // The mix gradient — the SAME square the For You tile drew,
                // at 224px with no badge and a 34px name.
                MixArtwork {
                    kind: root.kind
                    size: root.kioskHost ? 112 : 224
                    titleSize: 34
                    cornerRadius: theme.radiusMd
                    showBadge: false
                    interactive: false
                }

                Column {
                    width: parent.width - (root.kioskHost ? 112 : 224) - 32
                    anchors.top: parent.top
                    anchors.topMargin: 4
                    spacing: 0

                    Text {
                        text: QbzSession.tr("Qobuz Mixes", QbzSession.trRev)
                        color: theme.textMuted
                        font.pixelSize: 11
                        font.weight: theme.weightSemibold
                        font.letterSpacing: 1.5
                    }
                    Item { width: 1; height: 6 }
                    Text {
                        width: parent.width
                        text: root.doc.title || ""
                        color: theme.textPrimary
                        font.pixelSize: theme.fontSection
                        font.weight: theme.weightBold
                        elide: Text.ElideRight
                    }
                    Item { width: 1; height: 8 }
                    Text {
                        // .slint: `width: root.width - 320px`.
                        width: Math.max(0, root.width - 320)
                        text: root.doc.subtitle || ""
                        color: theme.textSecondary
                        font.pixelSize: theme.fontBody
                        wrapMode: Text.WordWrap
                    }
                    Item { width: 1; height: 10 }
                    Text {
                        visible: (root.doc.trackCount || 0) > 0
                        text: (root.doc.trackCount || 0) + " "
                            + QbzSession.tr("tracks", QbzSession.trRev)
                            + "  •  " + (root.doc.totalDuration || "")
                        color: theme.textSecondary
                        font.pixelSize: theme.fontLegal
                    }
                    Item { width: 1; height: 20 }

                    // Action discs (MixView.slint:161-210).
                    Row {
                        spacing: 12
                        QbzCircleAction {
                            diameterOverride: root.kioskHost ? 64 : 0
                            name: "play-fill"
                            primary: true
                            btnEnabled: root.tracks.length > 0
                            anchors.verticalCenter: parent.verticalCenter
                            onClicked: QbzHome.mixPlayAll()
                        }
                        QbzCircleAction {
                            diameterOverride: root.kioskHost ? 64 : 0
                            name: "shuffle"
                            btnEnabled: root.tracks.length > 0
                            anchors.verticalCenter: parent.verticalCenter
                            onClicked: QbzHome.mixShuffle()
                        }
                        // Add-to-playlist: inert in the .slint too (no
                        // `clicked` there) — see the header note.
                        QbzCircleAction {
                            diameterOverride: root.kioskHost ? 64 : 0
                            name: "list-plus"
                            btnEnabled: false
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        // Multi-select: DIMMED and inert (see the header note).
                        QbzCircleAction {
                            diameterOverride: root.kioskHost ? 64 : 0
                            name: "square-check-big"
                            btnEnabled: false
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        QbzCircleAction {
                            diameterOverride: root.kioskHost ? 64 : 0
                            name: "home-gear"
                            anchors.verticalCenter: parent.verticalCenter
                            onClicked: QbzHome.mixRefresh()
                        }
                    }
                }
            }

            Item { width: 1; height: 28 }

            // --- Loading / empty --------------------------------------------
            // The .slint mounts a bare centred spinner. This port shows the
            // SHAPE of the list that is coming (the PlaylistView / LabelView
            // convention): a column of track-row placeholders.
            QbzSkeleton {
                visible: root.loading && root.tracks.length === 0
                width: parent.width - 64
                height: visible ? 8 * 56 : 0
                variant: "rowList"
                rowH: 50
                rowGap: 6
                rowArt: true
                rowArtSize: 36
                phase: root.skelPhase
            }
            Text {
                visible: !root.loading && root.tracks.length === 0
                text: QbzSession.tr("No tracks for this mix.", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontBody
            }

            // --- Track list column header -----------------------------------
            // `parent.width - 64` is the width the rows get (see the delegate
            // below) — the header component's contract is that it is exactly
            // as wide as the rows it labels.
            TrackListHeader {
                kioskHost: root.kioskHost
                visible: root.tracks.length > 0
                width: parent.width - 64
                showArtwork: true
                showAlbum: true
            }
            Item { visible: root.tracks.length > 0; width: 1; height: visible ? 3 : 0 }
            Rectangle {
                visible: root.tracks.length > 0
                width: parent.width - 64
                height: visible ? 1 : 0
                color: theme.borderSubtle
            }
            Item { width: 1; height: 6 }

            // --- Rows --------------------------------------------------------
            // A Repeater, not a ListView: the mix is capped at 50 (dynamic
            // suggest) / 100 (TopQ) / 200 (FavQ) rows and the page is ONE
            // Flickable, exactly like LabelView's Popular Tracks. The 2000-row
            // case that forces windowing does not arise here.
            Repeater {
                model: root.kioskHost ? [] : root.tracks
                delegate: TrackRow {
                    required property var modelData
                    required property int index
                    // `parent` is the padded page Column (the LabelView
                    // guard: it is momentarily null while the delegate is
                    // being reparented).
                    width: parent ? parent.width - 64 : 0
                    item: modelData
                    number: index + 1
                    showArtwork: true
                    showAlbum: true
                    showFavorite: true
                    // The mix list is not reorderable and is not a drag
                    // source for playlists (flat catalog rows, no container).
                    onPlayRequested: QbzHome.mixPlayTrack(modelData.id)
                    onEnqueueRequested: function (mode) {
                        QbzHome.mixEnqueueTrack(modelData.id, mode)
                    }
                    // MyQBZ "Add to mixtape" — the HOST builds the AddItem
                    // array (TrackRow does not know itemType/source).
                    //
                    // SOURCE: these rows carry no source field, and they do
                    // not need one — every one of the four mixes is built
                    // from `qbz_models::Track` values that came back from the
                    // Qobuz API (foryou_qt.rs:855-873: dynamic/suggest for
                    // daily+weekly, `get_favorites("tracks")` for fav,
                    // `get_playlist` for top), and the DailyQ/WeeklyQ seed
                    // explicitly drops local / Plex / ephemeral recents
                    // (foryou_qt.rs:774-776). `modelData.id` is therefore a
                    // Qobuz catalog id by construction of the document, not
                    // by assumption at this call site.
                    onMixtapeRequested: QbzMyQbzAdd.open(JSON.stringify([{
                        "itemType": "track", "source": "qobuz",
                        "sourceItemId": modelData.id,
                        "title": modelData.title || "",
                        "subtitle": modelData.artist || "",
                        // artworkUrl STAYS EMPTY here, deliberately: the mix row carries no remote art url.
                        // A file:// cache path must NOT be stored — the collection's
                        // artwork_url is a snapshot other machines read — so this needs a
                        // remote-url field on the document first. The five sister sites
                        // that HAD one were stamped 2026-08-22.
                        "artworkUrl": "", "year": null, "trackCount": null
                    }]))
                }
            }
            Item {
                id: mixWindow
                visible: root.kioskHost
                width: parent.width - 64
                height: visible ? root.tracks.length * 64 : 0
                readonly property real viewTop: { var h=flick.contentHeight; return flick.contentY-mapToItem(flick.contentItem,0,0).y }
                readonly property int first: Math.max(0, Math.floor(viewTop/64)-1)
                readonly property int last: Math.max(first,Math.min(root.tracks.length,Math.ceil((viewTop+flick.height)/64)+1))
                Repeater {
                    model: root.kioskHost ? root.tracks.slice(mixWindow.first,mixWindow.last) : []
                delegate: TrackRow {
                    kioskHost: true
                    y: (mixWindow.first + index) * 64
                    required property var modelData
                    required property int index
                    // `parent` is the padded page Column (the LabelView
                    // guard: it is momentarily null while the delegate is
                    // being reparented).
                    width: mixWindow.width
                    item: modelData
                    number: mixWindow.first + index + 1
                    showArtwork: true
                    showAlbum: true
                    showFavorite: true
                    // The mix list is not reorderable and is not a drag
                    // source for playlists (flat catalog rows, no container).
                    onPlayRequested: QbzHome.mixPlayTrack(modelData.id)
                    onEnqueueRequested: function (mode) {
                        QbzHome.mixEnqueueTrack(modelData.id, mode)
                    }
                    // MyQBZ "Add to mixtape" — the HOST builds the AddItem
                    // array (TrackRow does not know itemType/source).
                    //
                    // SOURCE: these rows carry no source field, and they do
                    // not need one — every one of the four mixes is built
                    // from `qbz_models::Track` values that came back from the
                    // Qobuz API (foryou_qt.rs:855-873: dynamic/suggest for
                    // daily+weekly, `get_favorites("tracks")` for fav,
                    // `get_playlist` for top), and the DailyQ/WeeklyQ seed
                    // explicitly drops local / Plex / ephemeral recents
                    // (foryou_qt.rs:774-776). `modelData.id` is therefore a
                    // Qobuz catalog id by construction of the document, not
                    // by assumption at this call site.
                    onMixtapeRequested: QbzMyQbzAdd.open(JSON.stringify([{
                        "itemType": "track", "source": "qobuz",
                        "sourceItemId": modelData.id,
                        "title": modelData.title || "",
                        "subtitle": modelData.artist || "",
                        // artworkUrl STAYS EMPTY here, deliberately: the mix row carries no remote art url.
                        // A file:// cache path must NOT be stored — the collection's
                        // artwork_url is a snapshot other machines read — so this needs a
                        // remote-url field on the document first. The five sister sites
                        // that HAD one were stamped 2026-08-22.
                        "artworkUrl": "", "year": null, "trackCount": null
                    }]))
                }
                }
            }

        }
    }

    // Thin auto-hiding scrollbar in the right gutter (ListScrollbar).
    // Back/forward scroll memory (controls/ScrollMemory.qml): reports
    // this container's offset while it is the live page, and restores it
    // when a back/forward step arms this route.
    ScrollMemory { target: flick; scope: "mix" }
    QbzScrollBar {
        anchors.right: parent.right
        anchors.rightMargin: 4
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        target: flick
    }
}
