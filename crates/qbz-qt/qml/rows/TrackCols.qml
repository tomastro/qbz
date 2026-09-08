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

    /// Row-body horizontal padding (TrackRow.slint:265-266).
    readonly property int padH: 12
    /// Inter-column gap (TrackRow.slint:267).
    readonly property int gap: kioskHost ? 8 : 14

    /// Reorder gutter — the up/down chevron stack at the LEADING edge of a
    /// custom-order row (TrackRow.slint:272-280, `width: 22px`). Drawn only
    /// on a reorderable surface (the playlist detail under custom sort, and
    /// a local playlist under its natural order), so it is the one arm the
    /// Slint header does NOT reserve. It is reserved here, because a header
    /// that ignores a column the row draws slides every label after it by
    /// width + gap — the exact drift this file exists to prevent.
    readonly property int colReorder: kioskHost ? 44 : 22
    /// Number / play-cell (TrackRow.slint:344 `number-width: 32px`).
    readonly property int colNumber: kioskHost ? 44 : 32
    /// Artwork thumbnail (TrackRow.slint:336, show-artwork arm).
    readonly property int colArt: 36
    /// Origin mark for mixed/offline playlist rows.
    readonly property int colSource: 22
    /// Album link column (TrackRow.slint:540, show-album arm).
    readonly property int colAlbum: 220
    /// Duration (TrackRow.slint:569).
    readonly property int colDuration: kioskHost ? 60 : 70
    /// Quality badge cell (TrackRow.slint:581).
    readonly property int colQuality: 92
    /// Heart (TrackRow.slint:600, show-favorite arm).
    readonly property int colFavorite: kioskHost ? 44 : 28
    /// Offline/cloud slot (TrackRow.slint:650, show-download arm).
    readonly property int colDownload: kioskHost ? 44 : 28
    /// ⋯ context menu (TrackRow.slint:740, show-menu arm).
    readonly property int colMenu: kioskHost ? 44 : 32

    /// Sum of every FIXED cell that is actually drawn for these arms.
    ///
    /// `reorder` is TRAILING and optional on all three functions: it landed
    /// after the call sites did, and `undefined` is falsy, so a caller that
    /// does not know about the gutter keeps its old answer exactly.
    function fixedWidth(artwork, albumCol, favorite, download, menu, reorder, source) {
        return colNumber + (artwork ? colArt : 0) + (albumCol ? colAlbum : 0)
            + colDuration + colQuality
            + (favorite ? colFavorite : 0) + (download ? colDownload : 0)
            + (menu ? colMenu : 0) + (reorder ? colReorder : 0)
            + (source ? colSource : 0)
    }

    /// Sum of the inter-column gaps. The unconditional cells are number,
    /// title, duration and quality = 3 gaps; each arm adds exactly one.
    /// (QML's Row skips invisible children entirely — no cell, no gap — and
    /// so does Slint's `if` in a HorizontalLayout, which is why the count is
    /// arm-dependent on both sides.)
    function gapWidth(artwork, albumCol, favorite, download, menu, reorder, source) {
        return (3 + (artwork ? 1 : 0) + (albumCol ? 1 : 0) + (favorite ? 1 : 0)
            + (download ? 1 : 0) + (menu ? 1 : 0) + (reorder ? 1 : 0)
            + (source ? 1 : 0)) * gap
    }

    /// The stretch column. `rowWidth` is the OUTER width of the row (the
    /// padding is subtracted here), so a header and a row that are the same
    /// width place every column at the same x.
    function titleWidth(rowWidth, artwork, albumCol, favorite, download, menu, reorder, source) {
        return Math.max(0, rowWidth - 2 * padH
            - fixedWidth(artwork, albumCol, favorite, download, menu, reorder, source)
            - gapWidth(artwork, albumCol, favorite, download, menu, reorder, source))
    }
}
