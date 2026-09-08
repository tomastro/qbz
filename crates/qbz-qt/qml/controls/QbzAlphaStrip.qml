// Vertical A-Z jump strip (favorites/FavoritesView.slint:398 `AlphaStrip`;
// LocalLibraryView.slint:577 is a 1:1 lift of the same component). 18px
// gutter, 15px rows, 9px semibold muted letters that go accent on hover.
// Emits jump(ordinal, index); the host scrolls its own view — exactly like
// the Slint, which scrolls proportionally on the grouped grids (:1996) and by
// row index on the uniform track list (:1687).
//
// PROMOTED from views/local/AlphaStrip.qml, unchanged except for the import
// depth — same move QbzMultiSelectBar made and for the same reason: the Slint
// component was always shared (Local Library albums/artists/tracks AND the
// Library favorites tabs), so the "local" folder was a naming accident. The
// old views/local/AlphaStrip.qml is GONE; this file is the only copy.
// Call sites: views/local/{LocalAlbumsTab,LocalArtistsTab,LocalTracksTab}.qml,
// views/library/{LibraryAlbumsList}.qml and views/LibraryView.qml.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    /// [{ letter, index }] — index = the row ordinal in the flat model.
    property var jumps: []
    /// Paint the stable # A-Z affordance even when a query has no row for a
    /// letter. Missing buckets are deliberately inert and dimmed; present
    /// buckets retain their exact native/global row index.
    property bool completeAlphabet: false
    signal jump(int ordinal, int index)

    function entryAt(position) {
        if (!root.completeAlphabet)
            return (root.jumps || [])[position] || ({ "letter": "", "index": -1 })
        var targets = ({})
        var source = root.jumps || []
        for (var i = 0; i < source.length; i++) {
            var letter = String(source[i].letter || "#").toUpperCase()
            if (letter < "A" || letter > "Z") letter = "#"
            if (targets[letter] === undefined)
                targets[letter] = Number(source[i].index)
        }
        var letter = "#ABCDEFGHIJKLMNOPQRSTUVWXYZ".charAt(position)
        return { "letter": letter,
                 "index": targets[letter] === undefined ? -1 : targets[letter] }
    }

    QbzTheme { id: theme }

    width: 18

    Column {
        anchors.centerIn: parent
        spacing: 0
        Repeater {
            // An integer model makes the full rail structural: it always
            // creates exactly 27 delegates, regardless of when the async
            // jump document lands or how a JS-array binding is invalidated.
            model: root.completeAlphabet ? 27 : (root.jumps || []).length
            delegate: Item {
                id: cell
                required property int index
                readonly property var entry: root.entryAt(index)
                width: 18
                // A complete rail is 27 entries (# + A-Z). Keep the reference
                // 15px row when it fits, then compress uniformly on short
                // windows instead of painting letters outside the viewport.
                height: Math.max(1, Math.min(15,
                    root.height / Math.max(1, root.completeAlphabet
                        ? 27 : (root.jumps || []).length)))
                Text {
                    anchors.fill: parent
                    text: cell.entry.letter
                    color: cell.entry.index < 0 ? theme.alphaTier(24)
                        : cellArea.containsMouse ? theme.accent : theme.textMuted
                    font.pixelSize: Math.min(9, Math.max(7, parent.height - 1))
                    font.weight: theme.weightSemibold
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                MouseArea {
                    id: cellArea
                    anchors.fill: parent
                    enabled: cell.entry.index >= 0
                    hoverEnabled: true
                    cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                    onClicked: root.jump(cell.index, cell.entry.index)
                }
            }
        }
    }
}
