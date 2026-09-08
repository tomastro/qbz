// Track Info modal — 1:1 QML port of crates/qbz-ui/ui/album/TrackInfoModal.slint.
//
// HOW IT OPENS (taken from the .slint, not chosen): a MODAL — a full-window
// #000000bf scrim with a centered card, NOT a flyout and NOT a docked panel.
// Slint mounts it in AppShell as `Rectangle scrim + centered card` gated on
// TrackInfoState.open; Escape and a scrim click close it (ADR-009: modal
// z >= 3000). Here it is a Popup parented to `Overlay.overlay` so the bars can
// own it without AppShell.qml having to mount anything (the NowPlayingBar
// Loader guarantees only ONE bar — hence only one instance — is alive).
//
// Geometry, verbatim from the .slint:
//   card width   = min(window - 40, 728)
//   card height  = min(window * 0.8, content preferred height)
//   credits col  = (card width - 48 /*content padding*/ - 24 /*gutter*/) / 2
//   metadata col = (card width - 48 - 2 * 24) / 3  (three equal columns)
//   scrollbar    = 14px, inset 4px from the card's right edge
// The Slint card carries drop-shadow-blur 32px; QML shadows need an effect
// (Qt5Compat / MultiEffect) and it is dropped here rather than faked.
// SUPERSEDED (2026-07-29): the reason used to be "effects render NOTHING on the
// software path in this port". Effects need shaders, and this port runs on the
// GPU (OpenGL RHI, measured); that note came from an offscreen session, which
// forces the software renderer by definition — theme/RoundedImage.qml now
// detects the software path with `GraphicsInfo.api` rather than assuming it.
// The shadow is still not added: it is a visual change owing its own parity
// pass against Slint's blur, not a perf fix.
//
// DATA: QbzAlbum.openTrackInfo(trackId) fetches and publishes
// QbzAlbum.trackInfoJson while QbzAlbum.trackInfoLoading is true. Document
// shape (1:1 with crates/qbz/src/info_modals.rs `TrackInfoData`):
//   { "error": "", "title": "", "album": "", "artist": "", "artistId": "",
//     "duration": "3:45", "quality": "24-bit / 96kHz", "isrc": "",
//     "label": "", "labelId": "", "copyright": "",
//     "credits": [ { "role": "PRODUCER", "roleRaw": "Producer",
//                    "names": ["A", "B"] } ] }
// Every read is guarded, so while the glue is missing the modal degrades to
// its (already 1:1) loading state instead of throwing.
//
// Artist, label and musician links all route through their live Qt bridges.
//
// Split (project size rule): the scrolling content is TrackInfoBody.qml and
// the two cell primitives are InfoMetaCell.qml / InfoCreditCell.qml — the
// .slint exports those two as components as well.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../theme"

Popup {
    id: root

    parent: Overlay.overlay
    x: 0
    y: 0
    width: parent ? parent.width : 0
    height: parent ? parent.height : 0
    padding: 0
    z: 3000
    modal: true
    // Our own scrim (the Slint's #000000bf) — the default modal dimmer would
    // darken it twice.
    dim: false
    closePolicy: Popup.CloseOnEscape

    QbzTheme { id: theme }

    // --- Data ------------------------------------------------------------
    readonly property var doc: parseDoc()
    function parseDoc() {
        try {
            return JSON.parse(QbzAlbum.trackInfoJson || "{}")
        } catch (e) {
            return ({})
        }
    }
    readonly property bool loading: {
        try {
            return QbzAlbum.trackInfoLoading === true
        } catch (e) {
            return false
        }
    }
    readonly property string errorText: doc.error || ""
    readonly property var credits: doc.credits || []
    // Even index -> left column, odd -> right (info_modals.rs step_by(2)).
    readonly property var creditsLeft: credits.filter(function (c, i) { return i % 2 === 0 })
    readonly property var creditsRight: credits.filter(function (c, i) { return i % 2 === 1 })

    // --- Actions ---------------------------------------------------------

    /// Open the modal for a track id — the Slint's media-action("track", id,
    /// "track-info"). The caller does the Qobuz-only gating.
    function openFor(trackId) {
        if (!trackId || trackId === "")
            return
        QbzAlbum.openTrackInfo(trackId)
        open()
    }

    function openArtist(artistId) {
        if (!artistId || artistId === "")
            return
        close()
        QbzArtist.openArtist(artistId)
    }
    function openLabel(labelId) {
        if (!labelId || labelId === "")
            return
        close()
        QbzHome.openLabel(labelId)
    }
    // Musician click — consumer #5 of six (contract §2.2). This was an empty
    // stub: the name hovered like a link and did nothing.
    //
    // CLOSE ORDER IS LOAD-BEARING, and it is the reverse of what reads
    // naturally: the reference dispatches FIRST and closes AFTER
    // (TrackInfoModal.svelte:20-27). Closing first would destroy this modal —
    // and, for a `weak`/`none` result, the global MusicianModal is summoned
    // by the very click that would have killed its opener. Dispatch, then close.
    function openMusician(name, roleRaw) {
        if (!name || name === "")
            return
        QbzArtist.resolveMusician(name, roleRaw || "")
        close()
    }

    background: Rectangle { color: "#bf000000" }

    contentItem: Item {

        // Scrim — click outside dismisses.
        MouseArea {
            anchors.fill: parent
            onClicked: root.close()
        }

        Rectangle {
            id: card
            width: Math.min(root.width - 40, 728)
            height: Math.min(root.height * 0.8, body.implicitHeight)
            x: Math.round((parent.width - width) / 2)
            y: Math.round((parent.height - height) / 2)
            radius: theme.radiusMd
            color: theme.surfaceCard
            border.width: 1
            border.color: theme.borderSubtle
            clip: true

            // Swallow clicks so they don't reach the scrim.
            MouseArea { anchors.fill: parent }

            Flickable {
                id: flick
                anchors.fill: parent
                contentWidth: width
                contentHeight: body.implicitHeight
                boundsBehavior: Flickable.StopAtBounds
                clip: true

                TrackInfoBody {
                    id: body
                    host: root
                    width: flick.width
                }
            }

            // The Slint mounts a 14px ListScrollbar inset 4px from the right.
            QbzScrollBar {
                target: flick
                anchors.right: parent.right
                anchors.rightMargin: 4
                anchors.top: parent.top
                anchors.bottom: parent.bottom
            }
        }
    }
}
