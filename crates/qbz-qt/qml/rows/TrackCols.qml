// TrackCols — THE column geometry of the track table. ONE source of truth,
// instantiated by BOTH rows/TrackRow.qml (which draws the cells) and
// rows/TrackListHeader.qml (which labels them).

import QtQuick

QtObject {
    property bool kioskHost: false
    property bool isMobile: Qt.platform.os === "android"

    /// Row-body horizontal padding.
    readonly property int padH: isMobile ? 12 : 12
    /// Inter-column gap.
    readonly property int gap: isMobile ? 10 : (kioskHost ? 8 : 14)

    readonly property int colReorder: isMobile ? 24 : (kioskHost ? 44 : 22)
    readonly property int colNumber: isMobile ? 0 : (kioskHost ? 44 : 32)
    readonly property int colArt: isMobile ? 42 : 36
    readonly property int colSource: 22
    readonly property int colAlbum: isMobile ? 0 : 220
    readonly property int colDuration: isMobile ? 0 : (kioskHost ? 60 : 70)
    readonly property int colQuality: isMobile ? 0 : 92
    readonly property int colFavorite: isMobile ? 0 : (kioskHost ? 44 : 28)
    readonly property int colDownload: isMobile ? 0 : (kioskHost ? 44 : 28)
    readonly property int colMenu: isMobile ? 36 : (kioskHost ? 44 : 32)

    function fixedWidth(artwork, albumCol, favorite, download, menu, reorder, source) {
        if (isMobile) {
            return colArt + (menu ? colMenu : 0) + (reorder ? colReorder : 0)
        }
        return colNumber + (artwork ? colArt : 0) + (albumCol ? colAlbum : 0)
            + colDuration + colQuality
            + (favorite ? colFavorite : 0) + (download ? colDownload : 0)
            + (menu ? colMenu : 0) + (reorder ? colReorder : 0)
            + (source ? colSource : 0)
    }

    function gapWidth(artwork, albumCol, favorite, download, menu, reorder, source) {
        if (isMobile) {
            return (1 + (menu ? 1 : 0) + (reorder ? 1 : 0)) * gap
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