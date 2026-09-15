// TrackCols — THE column geometry of the track table. ONE source of truth,
// instantiated by BOTH rows/TrackRow.qml (which draws the cells) and
// rows/TrackListHeader.qml (which labels them), so the header and the rows
// under it cannot drift apart by construction.
//
// --- Why this file exists (the Slint reference is WRONG here) -------------
// Slint hardcodes these widths TWICE: once in primitives/TrackRow.slint's
// HorizontalLayout, and once again in every view that draws a column header.
// There are four such headers, and THREE of them disagree with the row:
//
//   playlist/PlaylistView.slint:979-1023   spacing 14, 32 · 36 · * · 220 ·
//                                          70 · 92 · 28 · 28 · 32   <- correct
//   album/AlbumPageView.slint:811-880      spacing 16, 32 · * · 80 · 80 ·
//                                          28 · 28 · 32             <- drifts
//   album/LocalAlbumView.slint:501-540     spacing 16, 32 · * · 80 · 80 ·
//                                          28 · 32                  <- drifts
//   mix/MixView.slint:246-284              spacing 16, 40 · * · 220 · 64 ·
//                                          92 · 28 · 32             <- drifts
//                                          (and no 36px artwork reserve at
//                                          all, though its rows set
//                                          show-artwork: true)
//
// against primitives/TrackRow.slint:264-745 = padding 12, spacing 14,
// number 32 · art 36 · title(stretch) · album 220 · duration 70 ·
// quality 92 · favorite 28 · offline 28 · menu 32.
//
// So the reference's own arrangement is the hazard the owner reported. The
// port takes Slint's NUMBERS (from the row, which is what actually renders)
// and drops its arrangement: the numbers live here once, and the arithmetic
// that turns them into a title-column width lives here once too, so a header
// and a row asking the same question always get the same answer.
//
// --- Responsive arms -----------------------------------------------------
// There are NONE. Neither primitives/TrackRow.slint nor any of the four
// Slint headers has a width breakpoint; `compact` in AlbumPageView.slint is
// the hero action rail, not the table. Every column is unconditional except
// the five documented arms (artwork / album / favorite / download / menu),
// and those are set per SURFACE, not per width. The only width-dependent
// term is the title column, and `titleWidth()` clamps it at 0 so a very
// narrow window collapses the title instead of dragging the trailing
// columns off the row — identically for the header and for the row, because
// it is the same function.

import QtQuick

QtObject {
    property bool kioskHost: false
    property bool isMobile: Qt.platform.os === "android"

    /// Row-body horizontal padding (TrackRow.slint:265-266).
    readonly property int padH: isMobile ? 8 : 12
    /// Inter-column gap (TrackRow.slint:267).
    readonly property int gap: isMobile ? 8 : (kioskHost ? 8 : 14)

    readonly property int colReorder: isMobile ? 24 : (kioskHost ? 44 : 22)
    readonly property int colNumber: isMobile ? 28 : (kioskHost ? 44 : 32)
    readonly property int colArt: isMobile ? 38 : 36
    readonly property int colSource: 22
    readonly property int colAlbum: isMobile ? 0 : 220
    readonly property int colDuration: isMobile ? 44 : (kioskHost ? 60 : 70)
    readonly property int colQuality: isMobile ? 0 : 92
    readonly property int colFavorite: isMobile ? 0 : (kioskHost ? 44 : 28)
    readonly property int colDownload: isMobile ? 0 : (kioskHost ? 44 : 28)
    readonly property int colMenu: isMobile ? 36 : (kioskHost ? 44 : 32)

    function fixedWidth(artwork, albumCol, favorite, download, menu, reorder, source) {
        if (isMobile) {
            return colNumber + (artwork ? colArt : 0) + colDuration
                + (menu ? colMenu : 0) + (reorder ? colReorder : 0)
        }
        return colNumber + (artwork ? colArt : 0) + (albumCol ? colAlbum : 0)
            + colDuration + colQuality
            + (favorite ? colFavorite : 0) + (download ? colDownload : 0)
            + (menu ? colMenu : 0) + (reorder ? colReorder : 0)
            + (source ? colSource : 0)
    }

    function gapWidth(artwork, albumCol, favorite, download, menu, reorder, source) {
        if (isMobile) {
            return (2 + (artwork ? 1 : 0) + (menu ? 1 : 0) + (reorder ? 1 : 0)) * gap
        }
        return (3 + (artwork ? 1 : 0) + (albumCol ? 1 : 0) + (favorite ? 1 : 0)
            + (download ? 1 : 0) + (menu ? 1 : 0) + (reorder ? 1 : 0)
            + (source ? 1 : 0)) * gap
    }

    function titleWidth(rowWidth, artwork, albumCol, favorite, download, menu, reorder, source) {
        var w = isMobile ? (rowWidth > 0 ? rowWidth : 360) : rowWidth
        return Math.max(isMobile ? 120 : 0, w - 2 * padH
            - fixedWidth(artwork, albumCol, favorite, download, menu, reorder, source)
            - gapWidth(artwork, albumCol, favorite, download, menu, reorder, source))
    }
}
