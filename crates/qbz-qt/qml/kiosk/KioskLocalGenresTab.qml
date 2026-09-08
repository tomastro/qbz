// Local Library > Genres, kiosk body.
//
// The desktop tab is the Library Explorer: three or four chained facet columns
// (genre / year / artist / album) with their own search boxes. That is a
// pointer instrument and it does not survive 800×480.
//
// The kiosk keeps the FUNCTION — "show me this genre" — with a single facet: a
// bounded, horizontally scrolling chip rail over the genre names, and the
// matching albums in the same grid the other tabs use. Multi-facet chaining
// stays a Full UI capability; nothing is silently removed, because the album
// set a chip produces is the same set the desktop's first column produces.
//
// DOCUMENT. This is the ONE local surface that legitimately reads the legacy
// `localAlbumsJson`: `load_tab("genres")` explicitly routes to
// `load_albums_legacy()` precisely so the browser has a bounded logical-album
// document even while the Albums SURFACE is on the native model. So the
// stale-document rule the other tabs enforce does not apply here — this
// document is republished for this tab by design.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    property var view: null

    QbzTheme { id: theme }

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    readonly property var albums: root.view ? root.view.albums : []
    /// "" is the explicit All state.
    readonly property string selected: root.view ? root.view.selectedGenre : ""

    /// The genre names present in the document, de-duplicated case-insensitively
    /// and sorted. One O(n) pass per document change, never per frame.
    readonly property var genres: {
        var display = ({})
        var rows = root.albums
        for (var i = 0; i < rows.length; i++) {
            var values = rows[i].genres || []
            for (var j = 0; j < values.length; j++) {
                var name = (values[j] || "").trim()
                var key = name.toLowerCase()
                if (name !== "" && display[key] === undefined)
                    display[key] = name
            }
        }
        var out = []
        for (var k in display)
            out.push({ "key": k, "label": display[k] })
        out.sort(function (a, b) { return a.label.localeCompare(b.label) })
        return out
    }

    readonly property var matching: {
        var key = root.selected
        var rows = root.albums
        if (key === "")
            return rows
        var out = []
        for (var i = 0; i < rows.length; i++) {
            var values = rows[i].genres || []
            for (var j = 0; j < values.length; j++) {
                if ((values[j] || "").toLowerCase() === key) {
                    out.push(rows[i])
                    break
                }
            }
        }
        return out
    }

    function pick(key) {
        if (root.view)
            root.view.selectGenre(root.selected === key ? "" : key)
    }

    // The focus ring reaches the RESULT grid; the chip rail is driven by tap
    // and by the horizontal move the shell already owns.
    function publishNav() {
        if (root.view)
            root.view.publishNav(Math.max(1, grid.columns), root.matching.length)
    }
    onMatchingChanged: root.publishNav()
    Component.onCompleted: root.publishNav()
    Connections {
        target: grid
        function onColumnsChanged() { root.publishNav() }
    }

    // ---------------------------------------------------------------------
    // The chip rail. A ListView, not a Repeater: a large library reaches
    // hundreds of genres and only the chips on screen may be mounted.
    // ---------------------------------------------------------------------
    ListView {
        id: chips
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.leftMargin: root.view ? root.view.pad : 16
        anchors.rightMargin: root.view ? root.view.pad : 16
        height: 64
        visible: root.genres.length > 0
        orientation: ListView.Horizontal
        clip: true
        spacing: 10
        cacheBuffer: 480
        reuseItems: true
        boundsBehavior: Flickable.StopAtBounds
        // The "All" chip is index 0; the genres follow.
        model: root.genres.length + 1

        delegate: Rectangle {
            id: chip
            required property int index

            readonly property bool isAll: chip.index === 0
            readonly property var entry: chip.isAll ? null : root.genres[chip.index - 1]
            readonly property string key: chip.isAll ? "" : (chip.entry ? chip.entry.key : "")
            readonly property string label: chip.isAll
                ? root.t("All") : (chip.entry ? chip.entry.label : "")
            readonly property bool on: root.selected === chip.key

            width: Math.max(88, chipText.implicitWidth + 32)
            height: 48
            // A horizontal ListView owns `x` and leaves `y` to the delegate.
            y: Math.max(0, (chips.height - chip.height) / 2)
            radius: 24
            color: chip.on
                ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.18)
                : theme.surfaceElevated
            border.width: chip.on ? 2 : 1
            border.color: chip.on ? theme.accent : theme.borderSubtle

            Text {
                id: chipText
                anchors.centerIn: parent
                text: chip.label
                color: chip.on ? theme.textPrimary : theme.textSecondary
                font.pixelSize: 15
                font.weight: theme.weightSemibold
            }

            MouseArea {
                anchors.fill: parent
                // The rail is 64px tall and the chip 48; the negative margin
                // spends the remaining band on the touch target.
                anchors.topMargin: -8
                anchors.bottomMargin: -8
                onClicked: root.pick(chip.key)
            }
        }
    }

    KioskLocalAlbumGrid {
        id: grid
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: chips.visible ? chips.bottom : parent.top
        anchors.bottom: parent.bottom
        visible: !QbzLocal.localAlbumsLoading && root.matching.length > 0
        view: root.view
        surface: "genres"
        scrollScope: "local:genres"
        pad: root.view ? root.view.pad : 16
        nativeActive: false
        rows: root.matching
        onOpen: function (id) {
            if (root.view)
                root.view.openAlbum(id)
        }
    }

    KioskSkeleton {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: chips.visible ? chips.bottom : parent.top
        anchors.bottom: parent.bottom
        kind: "grid"
        columns: Math.max(2, grid.columns)
        pad: grid.pad
        gap: grid.gap
        loading: QbzLocal.localAlbumsLoading
        error: QbzLocal.localAlbumsError
        empty: !QbzLocal.localAlbumsLoading && QbzLocal.localAlbumsError === ""
            && root.matching.length === 0
        emptyText: root.albums.length === 0
            ? root.t("No albums in your local library yet.")
            : root.t("No albums match your search.")
    }
    // NO ScrollMemory on the chip rail: the shared component restores
    // `contentY`, and this rail scrolls on X. The grid below owns the tab's
    // scroll scope, which is the offset a Back/Forward has to bring back.
}
