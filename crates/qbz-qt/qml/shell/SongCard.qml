// SongCard — the now-playing card of the full bar (cover + title + context
// meta + the in-card audio stamp), extracted VERBATIM from PlayerBar.qml
// (phase 25 size split). 1:1 with crates/qbz-ui/ui/shell/SongCard.slint.
//
// `glass` renders the Classic contained card (60px art, 64px tall, radius 8,
// surface-elevated fill); New uses the flush 74px variant. Large mounts it
// with showArt/showBadges false — the cover lives in the sidebar dock and the
// AudioStamp moves to the right cluster, so the text column reclaims the
// stamp's 132px. `textCenter` is the SMALL-bar variant (SongCard.slint:329,
// mounted by PlayerBarSmall.slint:239-251): a 37px bordered cover, a 13px
// title and the title+meta collapsed into a tight centred group, no in-card
// stamp.
//
// The title click opens Track Info (SongCard.slint: `root.track-info()` from
// the title TouchArea, Qobuz-only) — the host bar owns the modal, this just
// emits `trackInfoRequested()`.
//
// META ROW (SongCard.slint:20-49 `MetaLink` + :486-540): [layers] artist · album.
// The two labels share the leftover width on the Slint's stretch budget —
// artist 2, album 1 — each capped at its natural width and free to shrink to
// nothing. A plain Row of unbounded Texts is what let the album slide past the
// card wall and vanish under the audio stamp / the bar column's `clip: true`
// (PlayerBar.qml:249): `elide` does NOTHING on a Text with no explicit width.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root

    property bool showArt: true
    property bool showBadges: true
    property bool glass: false
    property int artSize: glass ? 60 : 74
    // Optional cover border (SongCard.slint art-border-width / -color). OFF by
    // default so New/Classic/Large are untouched; Small passes 3px surface-card.
    property int artBorderWidth: 0
    property color artBorderColor: "transparent"
    // Cover -> text gap (SongCard.slint art-text-gap: 11px default, 9px Small).
    property int artTextGap: 11
    // Title size (SongCard.slint title-font-size: Typography.body default,
    // 13px on the Small bar).
    property real titleFontSize: theme.fontBody
    // Small bar: title+meta as a TIGHT centred group instead of the
    // full-height, space-distributed column.
    property bool textCenter: false
    /// AppShell's shared topmost tooltip, forwarded to the in-card stamp.
    property Item tooltip: null

    // Furniture is admitted by intrinsic fit, not by a window breakpoint.
    // When a host becomes genuinely narrow, metadata keeps the runway and the
    // cover/stamp yield before either can sit on top of text.
    readonly property bool stampFits: root.width >= stamp.width + 24
    readonly property bool effectiveShowArt: root.showArt
        && root.width >= root.artSize + root.artTextGap
            + (stamp.visible ? stamp.width + 4 : 0)

    /// Title clicked — the host opens the Track Info modal.
    signal trackInfoRequested()

    readonly property bool ambientOn: theme.ambientOn
    // Track Info is a Qobuz-only surface (Slint gates it on
    // NowPlayingState.source == "qobuz"). The Qt port publishes no np_source,
    // so a numeric track id is the proxy — local / Plex / ephemeral rows do
    // not carry one. TODO(glue): publish np_source (see the report).
    readonly property bool qobuzTrack: QbzPlayer.npHasTrack
        && /^[0-9]+$/.test(QbzPlayer.npTrackId)

    // "Playing from" origin, 1:1 with SongCard.slint:151-156: the CURRENT
    // track's own stamped container, falling back to its album when it carries
    // none (the Slint fallback in playback.rs:1959-1965).
    readonly property string ctxKind: QbzPlayer.npContextId !== ""
        ? QbzPlayer.npContextKind : "album"
    readonly property string ctxId: QbzPlayer.npContextId !== ""
        ? QbzPlayer.npContextId : QbzPlayer.npAlbumId

    // SongCard.slint fires `open-context(kind, id)`, which the bar routes to
    // `media-action(kind, id, "open")` — qbz/src/main.rs:12695 (artist), :12701
    // (album), :12707 (playlist), :13206 (label).
    function openContext() {
        if (root.ctxId === "")
            return
        if (root.ctxKind === "artist") {
            QbzArtist.openArtist(root.ctxId)
        } else if (root.ctxKind === "playlist") {
            QbzBridge.openPlaylist(root.ctxId)
        } else if (root.ctxKind === "ephemeral") {
            // A disc or an ad-hoc folder. Its "album" is a synthetic grouping
            // key (`cdda|||Fear Inoculum`) that no catalogue can open, so the
            // origin is the SESSION and the place that shows it is the Open
            // pane's own tab in Local Library.
            QbzLocal.setPendingRoute('{"tab":"ephemeral"}')
            QbzShell.navigateTo("locallibrary")
        } else if (root.ctxKind === "album") {
            QbzAlbum.openAlbum(root.ctxId)
        } else if (QbzPlayer.npAlbumId !== "") {
            // "label" (and anything added later) has no landing page in this
            // port — LabelCard.qml documents the same gap. Fall back to the
            // track's album so the glyph is never a dead control.
            QbzAlbum.openAlbum(QbzPlayer.npAlbumId)
        }
    }

    QbzTheme { id: theme }

    // SongCard.slint:504 tints the context-stack glyph text-primary on hover,
    // text-muted idle. Both are runtime-tinted (see QbzIconButton's header),
    // so this is the live token, not a fixed bake picked by lightness.
    readonly property string tintStrong: "textPrimary"

    width: 200
    height: glass ? 64 : 74
    radius: glass ? 8 : 0
    color: ambientOn ? "transparent" : (glass ? theme.surfaceElevated : "transparent")

    Row {
        anchors.left: parent.left
        // SongCard.slint's inner-row width formula: the card minus the
        // stamp + a 4px gap ONLY when the stamp is rendered (Large and Small
        // hide it — the AudioStamp moves to the right column — so the text
        // column must reclaim those 132px).
        anchors.right: stamp.visible ? stamp.left : parent.right
        anchors.rightMargin: stamp.visible ? 4 : 0
        height: parent.height
        spacing: 0

        Item {
            visible: root.effectiveShowArt
            width: root.effectiveShowArt ? root.artSize : 0
            height: parent.height
            Rectangle {
                width: root.artSize
                height: root.artSize
                anchors.centerIn: parent
                radius: 6
                color: theme.surfaceElevated
                border.width: root.artBorderWidth
                border.color: root.artBorderColor
                // No clip: RoundedImage confines itself on both arms; a clip is an
                // unconditional batch root, and this one is always on screen.
                RoundedImage {
                    visible: QbzPlayer.npHasTrack
                    anchors.fill: parent
                    source: QbzPlayer.npArtworkPath
                    radius: 6
                }
                QbzIcon {
                    visible: !QbzPlayer.npHasTrack
                    name: "music"
                    width: root.artSize * 0.5
                    height: root.artSize * 0.5
                    anchors.centerIn: parent
                    tintName: "muted"
                }
                // Fetch overlay (resolving/downloading). DELIBERATE literal,
                // not a theme token: this scrim lies over ALBUM ARTWORK, not
                // over a themed surface, so it must darken in BOTH polarities
                // — theme.alphaTier() is white-based on dark themes and would
                // wash the cover out instead of dimming it, and cardShadow is
                // a per-theme shadow colour, not a scrim. 1:1 with
                // SongCard.slint:278 (`background: #000000aa`).
                Rectangle {
                    visible: QbzPlayer.npHasTrack && QbzPlayer.npLoading
                    anchors.fill: parent
                    radius: 6
                    color: "#aa000000"
                    QbzSpinner {
                        width: root.artSize * 0.5
                        height: root.artSize * 0.5
                        anchors.centerIn: parent
                        size: root.artSize * 0.5
                    }
                }
                // Hover trigger for the floating 220px preview
                // (SongCard.slint:296). Topmost so the cover, the idle glyph
                // and the fetch scrim below it stay reachable; NoButton so it
                // reports hover only and every click passes through to them.
                // The anchor is published in WINDOW coordinates because the
                // overlay lives at the shell root.
                MouseArea {
                    id: artHover
                    anchors.fill: parent
                    hoverEnabled: true
                    acceptedButtons: Qt.NoButton
                    onContainsMouseChanged: {
                        if (containsMouse && QbzPlayer.npHasTrack) {
                            var pt = mapToItem(null, width / 2, 0)
                            QbzShell.artPreviewX = pt.x
                            QbzShell.artPreviewY = pt.y
                            QbzShell.artPreviewShow = true
                        } else if (!containsMouse) {
                            QbzShell.artPreviewShow = false
                        }
                    }
                }
            }
        }
        Item { width: root.effectiveShowArt ? root.artTextGap : 0; height: 1 }

        Column {
            width: Math.max(0, parent.width
                - (root.effectiveShowArt ? root.artSize + root.artTextGap : 0))
            anchors.verticalCenter: parent.verticalCenter
            // textCenter tightens the inter-line gap to 1px so the two lines
            // read as one block beside the small cover (SongCard.slint:333);
            // the default column keeps its 4px.
            spacing: root.textCenter ? 1 : 4
            Text {
                width: parent.width
                text: QbzPlayer.npHasTrack ? QbzPlayer.npTitle
                    : QbzSession.tr("Nothing playing", QbzSession.trRev)
                color: theme.textPrimary
                font.pixelSize: root.titleFontSize
                font.weight: theme.weightMedium
                elide: Text.ElideRight
                // Title -> Track Info (SongCard.slint). The MouseArea is a
                // CHILD of the Text so it can't take layout space in the
                // Column.
                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: root.qobuzTrack ? Qt.PointingHandCursor : Qt.ArrowCursor
                    onClicked: if (root.qobuzTrack) root.trackInfoRequested()
                }
            }

            // --- Meta row: [layers] artist · album ------------------------
            Item {
                id: metaRow
                visible: QbzPlayer.npHasTrack
                width: parent.width
                // Row height is driven by the 16x18 context box (SongCard.slint).
                height: 18

                // Natural (unelided) label widths. `implicitWidth` on a Text
                // that already HAS an explicit width is not a safe source here
                // — TextMetrics measures the string itself and never feeds the
                // width binding back into the layout.
                TextMetrics {
                    id: artistMetrics
                    font: artistText.font
                    text: artistText.text
                }
                TextMetrics {
                    id: albumMetrics
                    font: albumText.font
                    text: albumText.text
                }
                TextMetrics {
                    id: ellipsisMetrics
                    font: artistText.font
                    text: "…"
                }

                // Fixed furniture: the context box (+ its 7px gap when shown)
                // and the separator dot with a 7px gap on each side.
                readonly property real fixedW: (ctxBox.visible ? 16 + 7 : 0) + 3 + 14
                readonly property real avail: Math.max(0, metaRow.width - metaRow.fixedW)
                // Slint stretch 2 (artist) : 1 (album), each capped at its own
                // natural width and free to shrink to 0 (SongCard.slint:22-31):
                // a long album elides before the artist does, and both keep a
                // readable slice instead of one pushing the other off the card.
                readonly property var budget: {
                    // `width` is the painted bounding box; `advanceWidth` is
                    // what the line layout actually consumes. Budget the
                    // larger one plus a 2px bearing/rounding guard — assigning
                    // the exact bounding width made "Metallica" render as
                    // "Metalli…" despite spare room.
                    var a = Math.ceil(Math.max(artistMetrics.width,
                                               artistMetrics.advanceWidth)) + 2
                    var b = Math.ceil(Math.max(albumMetrics.width,
                                               albumMetrics.advanceWidth)) + 2
                    var av = metaRow.avail
                    if (a + b <= av)
                        return [a, b]
                    var qa = av * 2 / 3
                    var qb = av - qa
                    if (a < qa) {
                        qa = a
                        qb = av - a
                    } else if (b < qb) {
                        qb = b
                        qa = av - b
                    }
                    // An ellipsis costs roughly the same runway as the final
                    // one or two glyphs. If a label is within that distance of
                    // fitting, finish the word and take the small deficit from
                    // the other, genuinely long label instead.
                    var near = Math.ceil(ellipsisMetrics.advanceWidth) + 3
                    if (a > qa && a - qa <= near && av >= a) {
                        qa = a
                        qb = av - a
                    } else if (b > qb && b - qb <= near && av >= b) {
                        qb = b
                        qa = av - b
                    }
                    return [Math.floor(qa), Math.floor(qb)]
                }

                readonly property bool artistLinked: QbzPlayer.npArtistId !== ""
                readonly property bool albumLinked: QbzPlayer.npAlbumId !== ""

                Row {
                    anchors.fill: parent
                    spacing: 7

                    // Context-stack icon ("Show track playing context" pref) —
                    // opens the container playback was launched from.
                    Rectangle {
                        id: ctxBox
                        visible: QbzPlayer.showContextIcon
                        width: 16
                        height: 18
                        radius: 3
                        color: ctxArea.containsMouse ? theme.surfaceHover : "transparent"
                        QbzIcon {
                            name: "layers"
                            width: 13
                            height: 13
                            // Flush LEFT (SongCard.slint pins x: 0) so the meta
                            // line starts on the title's column.
                            x: 0
                            anchors.verticalCenter: parent.verticalCenter
                            tintName: ctxArea.containsMouse ? root.tintStrong : "muted"
                        }
                        MouseArea {
                            id: ctxArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: root.ctxId !== "" ? Qt.PointingHandCursor : Qt.ArrowCursor
                            onClicked: root.openContext()
                        }
                    }

                    Text {
                        id: artistText
                        width: metaRow.budget[0]
                        height: parent.height
                        verticalAlignment: Text.AlignVCenter
                        text: QbzPlayer.npArtist
                        color: artistArea.containsMouse && metaRow.artistLinked
                            ? theme.textPrimary : theme.textMuted
                        font.pixelSize: 11
                        elide: Text.ElideRight
                        MouseArea {
                            id: artistArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: metaRow.artistLinked ? Qt.PointingHandCursor : Qt.ArrowCursor
                            onClicked: if (metaRow.artistLinked) QbzArtist.openArtist(QbzPlayer.npArtistId)
                        }
                    }

                    Rectangle {
                        width: 3
                        height: 3
                        radius: 1.5
                        color: theme.textMuted
                        anchors.verticalCenter: parent.verticalCenter
                    }

                    Text {
                        id: albumText
                        width: metaRow.budget[1]
                        height: parent.height
                        verticalAlignment: Text.AlignVCenter
                        text: QbzPlayer.npAlbum
                        color: albumArea.containsMouse && metaRow.albumLinked
                            ? theme.textPrimary : theme.textMuted
                        font.pixelSize: 11
                        elide: Text.ElideRight
                        MouseArea {
                            id: albumArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: metaRow.albumLinked ? Qt.PointingHandCursor : Qt.ArrowCursor
                            onClicked: if (metaRow.albumLinked) QbzAlbum.openAlbum(QbzPlayer.npAlbumId)
                        }
                    }
                }
            }
        }
    }

    SongCardStamp {
        id: stamp
        tooltipHost: root.tooltip
        visible: root.showBadges && root.stampFits && QbzPlayer.npHasTrack
        anchors.right: parent.right
        // Classic (glass) insets the stamp by pad + stamp-right-margin
        // (2px each) so it clears the visible card border.
        anchors.rightMargin: root.glass ? 4 : 0
        anchors.verticalCenter: parent.verticalCenter
    }
}
