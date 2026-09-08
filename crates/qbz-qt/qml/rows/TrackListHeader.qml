// TrackListHeader — the column header of the track table, the labelled twin
// of rows/TrackRow.qml. Both take their geometry from rows/TrackCols.qml, so
// a label sits over the column it names by construction rather than by two
// people typing the same numbers.
//
// --- THE CONTRACT (two lines, both checkable) ---------------------------
//   1. `width` MUST be the width the TrackRows below get. Not the page
//      width — the ROW width. A list inset for a scrollbar gutter
//      (PlaylistView's `anchors.rightMargin: 14`) or padded by its page
//      (MixView's `parent.width - 64`) hands its rows a narrower width, and
//      the header has to be narrowed the same way or every label after the
//      stretch column slides right by the difference.
//   2. The five arms MUST be the arms the delegate sets. They gate BOTH the
//      cell and its gap, so a header that reserves a column the row does not
//      draw (or vice versa) shifts everything after it by width + 14.
//
// The owner's report was the second half of rule 2: the trailing unlabelled
// columns (heart, offline cloud, ⋯) were missing from some headers, so they
// took no space and every label was pushed right of its column. They are
// reserved here as empty slots — `favoriteGlyph` / `downloadGlyph` draw the
// 14px heart / cloud-download that AlbumPageView.slint:853-877 puts in the
// heads of those two columns, the only labelling Slint gives them.
//
// `trailingReserve` covers the one surface whose row draws controls OUTSIDE
// the shared TrackRow: views/local/LocalTrackRow.qml narrows the shared row
// and re-draws the ⋯ plus a source glyph in the gutter it freed. The header
// reserves the same slice so it stays the same width as the row it labels.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    property bool kioskHost: false

    id: root

    // --- Arms: MIRROR the TrackRow arms of the list underneath ----------
    // Defaults are TrackRow's own defaults, so a host that sets nothing gets
    // a header for a row that sets nothing.
    property bool showArtwork: false
    property bool showSource: false
    property bool showAlbum: false
    property bool showFavorite: true
    property bool showDownload: false
    property bool showMenu: true
    /// Leading reorder gutter (the row's up/down chevrons). MIRROR the
    /// delegate's `showReorder`, or every label slides by 22 + 14 the moment
    /// the user switches the list to a custom / reorderable order.
    property bool showReorder: false

    /// Draw the 14px heart / cloud-download in the head of those two
    /// reserved columns (AlbumPageView.slint). Off = an empty reserved slot,
    /// which is what PlaylistView.slint and MixView.slint do.
    property bool favoriteGlyph: false
    property bool downloadGlyph: false
    /// When enabled, the cloud header becomes the whole-collection cache
    /// button. It remains an inert column label everywhere else.
    property bool downloadActionEnabled: false
    property bool downloadActionLoading: false
    signal downloadClicked()

    /// Label band height. 0 = the label's own line height, which is what the
    /// playlist / mix headers use (Slint gives those HorizontalLayouts no
    /// height, so they take the text's — 15px at Typography.legal, and
    /// hardcoding an 18 here would have added 3px to those pages' rhythm).
    /// The album headers set an explicit 40px band (AlbumPageView.slint:812).
    property int bandHeight: 0
    /// `letter-spacing: 0.5px` on the album headers; the playlist / mix
    /// headers have none.
    property real labelSpacing: 0
    /// Width the ROW draws outside the shared TrackRow (LocalTrackRow only).
    property int trailingReserve: 0

    TrackCols { id: cols; kioskHost: root.kioskHost }
    QbzTheme { id: theme }

    width: parent ? parent.width : 0
    height: bandH

    /// Resolved band height (see `bandHeight`). The metric below is the same
    /// font the labels use, so "natural" tracks a font-size change.
    readonly property int bandH: root.bandHeight > 0
        ? root.bandHeight : Math.ceil(metric.implicitHeight)
    Text {
        id: metric
        visible: false
        text: "Xg"
        font.pixelSize: theme.fontLegal
    }

    /// The row's own outer width minus whatever it spends outside the shared
    /// TrackRow — i.e. exactly the width the shared row lays its cells in.
    readonly property int innerWidth: Math.max(0, root.width - root.trailingReserve)

    component ColLabel: Text {
        color: theme.textMuted
        font.pixelSize: theme.fontLegal
        font.letterSpacing: root.labelSpacing
        height: root.bandH
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }

    Row {
        anchors.fill: parent
        anchors.leftMargin: cols.padH
        anchors.rightMargin: cols.padH
        spacing: cols.gap

        // Reorder gutter — unlabelled, reserved (the row draws two chevrons
        // there; there is nothing to name).
        Item {
            visible: root.showReorder
            width: cols.colReorder
            height: root.bandH
        }
        // "#" — centred, because the cell it labels is TrackRow's play cell
        // and its number is centred in the same 32px. (Slint's album headers
        // left-align this one over a centred number; the playlist header
        // centres it. Centred is the one that lines up.)
        ColLabel {
            width: cols.colNumber
            text: "#"
            horizontalAlignment: Text.AlignHCenter
        }
        // Artwork cell — unlabelled, reserved (PlaylistView.slint:990).
        Item {
            visible: root.showArtwork
            width: cols.colArt
            height: root.bandH
        }
        Item {
            visible: root.showSource
            width: cols.colSource
            height: root.bandH
        }
        ColLabel {
            width: cols.titleWidth(root.innerWidth, root.showArtwork, root.showAlbum,
                                   root.showFavorite, root.showDownload, root.showMenu,
                                   root.showReorder, root.showSource)
            text: QbzSession.tr("Title", QbzSession.trRev)
        }
        ColLabel {
            visible: root.showAlbum
            width: cols.colAlbum
            text: QbzSession.tr("Album", QbzSession.trRev)
        }
        ColLabel {
            width: cols.colDuration
            text: QbzSession.tr("Duration", QbzSession.trRev)
            horizontalAlignment: Text.AlignHCenter
        }
        ColLabel {
            width: cols.colQuality
            text: QbzSession.tr("Quality", QbzSession.trRev)
            horizontalAlignment: Text.AlignHCenter
        }
        // Heart column. `height: root.bandH` (not 1) so `centerIn`
        // centres the glyph on the band instead of pinning it to the top —
        // the bug AlbumView.qml carried and documented.
        Item {
            visible: root.showFavorite
            width: cols.colFavorite
            height: root.bandH
            QbzIcon {
                visible: root.favoriteGlyph
                anchors.centerIn: parent
                name: "heart"
                width: 14
                height: 14
                tintName: "muted"
            }
        }
        // Offline / cloud column.
        Item {
            visible: root.showDownload
            width: cols.colDownload
            height: root.bandH
            QbzIcon {
                visible: root.downloadGlyph && !root.downloadActionLoading
                anchors.centerIn: parent
                name: "cloud-download"
                width: 14
                height: 14
                tintName: downloadArea.containsMouse && root.downloadActionEnabled
                    ? "accent" : "muted"
            }
            QbzSpinner {
                visible: root.downloadActionLoading
                anchors.centerIn: parent
                size: 14
            }
            MouseArea {
                id: downloadArea
                anchors.fill: parent
                enabled: root.downloadGlyph && root.downloadActionEnabled
                    && !root.downloadActionLoading
                hoverEnabled: enabled
                cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                onClicked: root.downloadClicked()
            }
        }
        // ⋯ column.
        Item {
            visible: root.showMenu
            width: cols.colMenu
            height: root.bandH
        }
        // Whatever the row draws outside the shared TrackRow.
        Item {
            visible: root.trailingReserve > 0
            // The gap before it is already spent by the Row, so the slot
            // itself is the reserve minus that gap.
            width: Math.max(0, root.trailingReserve - cols.gap)
            height: root.bandH
        }
    }
}
