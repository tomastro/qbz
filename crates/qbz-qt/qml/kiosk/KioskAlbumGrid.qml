// KioskAlbumGrid — the wrapping card grid behind the kiosk Search / Library /
// Local Library album, folder and artist tabs.
//
// ── WHY THIS IS NOW A GridView ────────────────────────────────────────────
// It used to be a Flickable holding a Repeater of CHEAP EMPTY SLOTS, one per
// album, each with a Loader that mounted the heavy KioskCard only inside a
// 240 px band around the viewport. Cards and images were bounded that way, and
// the file's own header argued a GridView could not drive the focus writes.
//
// K0 measured what the slots themselves cost, on the real component, at
// 800x480 (03-K0-INITIAL-EVIDENCE.md and 04-INTEGRATION-SEAMS.md):
//
//   rows      visual items   Loaders (top/mid)   cards/images   assign→settle
//   20               237          44 / 52           12 / 16        168 ms
//   2 000          4 197       2 024 / 2 032        12 / 16        116 ms
//   20 000        40 197      20 024 / 20 032       12 / 16      1 043 ms
//
// and on the Pi itself (Debian 13 aarch64, Qt 6.8.2, software renderer,
// identical fixture) the 20 000-row case took 4 815.99 ms with 40 197 visual
// items. Cards and images were indeed bounded at 12/16 — and the mount still
// cost nearly five seconds on the target hardware, before a single cover.
//
// So the pattern was doing its job and the job was not enough: the O(n) part
// was never the cards, it was the 2n objects the windowing itself needed. That
// is the growth contract §5.1 requires removed and §3.3 answers with
// "ListView/GridView es la primera opción cuando conserva navegación".
//
// A GridView creates delegates for the viewport plus `cacheBuffer` and NOTHING
// for the rest of the model, so 20 rows and 20 000 rows mount the same tree.
//
// ── AND THE FOCUS OBJECTION ───────────────────────────────────────────────
// The retired header was right that hand-written `contentY` arithmetic does
// not survive a view that owns its own origin and margins. The answer is not
// to keep the Repeater, it is to stop doing the arithmetic:
// `positionViewAtIndex(i, GridView.Contain)` scrolls the MINIMUM amount to
// bring an item inside the viewport and does nothing when it is already there
// — which is exactly what the old `scrollFocusIntoView` computed by hand, and
// it is correct through margins, origin and a partially-filled last row.
//
// The Qt.callLater discipline is kept verbatim: the scroll is issued on a
// SETTLED layout, from a function, never from a binding that feeds layout —
// the AlbumCollectionView "Recursion detected" panic class.
//
// ── PUBLIC API (unchanged) ────────────────────────────────────────────────
// `albums`, `columns`, `gap`, `pad`, `open(id)`, plus everything a Flickable
// exposes (`contentY`, `contentHeight`, `flick`…), which a GridView still is.
// `cardW` / `cardH` / `rows` keep their meaning. Callers are unmodified:
// KioskLibrary.qml:437, KioskSearch.qml:696, KioskLocalLibrary.qml:375 and
// :389.
//
// ── NEW: THE VISIBLE WINDOW ───────────────────────────────────────────────
// `firstVisible` / `lastVisible` and the `visibleWindow(first, last)` signal
// exist for the hosts that already own an artwork-window API and could not
// drive it: KioskLocalLibrary reports its window ONCE per tab entry and never
// on scroll (KioskLocalLibrary.qml:161-164), so only the first band of covers
// ever resolves. That limit was inherited from the Slint, whose grid had no
// window hook to fire. This grid has one.
//
// The bounds are DERIVED FROM GEOMETRY (contentY, originY, cellHeight), not
// from walking delegates: it is a handful of integer operations, it is exact
// for a uniform grid, and it does not depend on when the view happens to have
// created a delegate. Emission is coalesced through Qt.callLater and gated on
// an actual change, so a flick emits once per row crossed, not once per frame.
//
// ── OVERSCAN ──────────────────────────────────────────────────────────────
// `overscanRows` (default 1) becomes `cacheBuffer`, i.e. one row of delegates
// kept live above and below the viewport. Contract §5.1 caps grids at two
// rows; the default is deliberately the smaller number because the Pi is the
// floor, and a host with a measurement may raise it to 2.
//
// The content index space skips no entries here; the HOSTING VIEW publishes
// the grid geometry (tabs / columns / count) into QbzKioskNav and this grid
// only mirrors the focus index (ring + scroll-into-view) and answers the Enter
// pulse. `focusedItem` subtracts the view's leading tab entries.

import QtQuick
import com.blitzfc.qbz

GridView {
    id: root

    property var albums: []
    property int columns: 6
    property real gap: 16
    property real pad: 16
    /// Rows of delegates kept alive beyond the viewport, each side. See
    /// OVERSCAN above; the contract's ceiling is 2.
    property int overscanRows: 1
    signal open(string id)

    /// Emitted when the range of album indices inside the viewport changes.
    /// `first`/`last` are inclusive; both are -1 when the model is empty.
    /// Coalesced — see THE VISIBLE WINDOW above.
    signal visibleWindow(int first, int last)
    signal windowChanged(int first, int last)

    // ── pitch ──────────────────────────────────────────────────────────────
    // A GridView has no column count: it fits `floor(available / cellWidth)`
    // cells per row. So the pitch is derived to make that floor land on
    // `columns` exactly.
    //
    // The old layout was `pad | card | gap | card | … | card | pad`. Read as a
    // uniform pitch of `cardW + gap` that is `pad` from the left, the trailing
    // cell's own gap covers `gap` of the right padding, so the view only needs
    // `pad - gap` of right margin. Substituting back gives exactly the old
    // `cardW = (width - 2*pad - (columns-1)*gap) / columns`.
    //
    // Math.floor, because equality is not safe: `columns * cellWidth` landing a
    // float hair ABOVE the available width would fit one column fewer and
    // reflow the whole grid. Flooring loses under a pixel per row and cannot.
    readonly property real _rightPad: Math.max(0, root.pad - root.gap)
    readonly property real _avail: Math.max(0, root.width - root.pad - root._rightPad)
    readonly property real cardW: Math.max(0, root.cellWidth - root.gap)
    readonly property real cardH: root.cardW + 46
    readonly property int rows: root.columns > 0
        ? Math.ceil(root.albums.length / root.columns)
        : 0

    model: root.albums
    // Never 0: a zero cell size makes the view's own row/column arithmetic
    // divide by zero before the item has been given a width.
    cellWidth: root.columns > 0 ? Math.max(1, Math.floor(root._avail / root.columns)) : 1
    cellHeight: Math.max(1, root.cardH + root.gap)

    leftMargin: root.pad
    rightMargin: root._rightPad
    topMargin: root.pad
    bottomMargin: root.pad

    clip: true
    boundsBehavior: Flickable.StopAtBounds
    // THE bound on live delegates. Everything outside viewport + this is not
    // instantiated at all — no card, no image, no slot, no Loader.
    cacheBuffer: Math.max(0, Math.ceil(root.overscanRows * root.cellHeight))
    // Recycle rather than destroy/recreate on every row crossed: a flick over
    // a 20 000-row model then costs a fixed number of delegates for its whole
    // length. `index`/`modelData` re-evaluate on reuse, so the focus ring and
    // the card contents follow.
    reuseItems: true
    // The focus ring is the kiosk's own (QbzKioskNav), not the view's cursor;
    // a highlighted current item would draw a second, contradictory ring.
    highlight: null
    currentIndex: -1
    // Keyboard reaches this grid through QbzKioskNav, never through the view's
    // own key handling — two cursors would fight over one ring.
    keyNavigationEnabled: false

    // ── focus ring + scroll-into-view ──────────────────────────────────────
    readonly property int focusedItem: QbzKioskNav.index - QbzKioskNav.tabs
    readonly property bool itemFocused: QbzKioskNav.navActive
        && QbzKioskNav.zone === "content"
        && root.focusedItem >= 0
        && root.focusedItem < root.albums.length

    /// Scroll the focused card into view. Called through Qt.callLater on a
    /// settled layout — never from a binding that feeds layout.
    ///
    /// `GridView.Contain` is the whole of the old hand-written body: scroll the
    /// minimum amount to make the item visible, do nothing if it already is.
    /// Unlike the arithmetic it replaces it is correct through the view's
    /// margins and origin, which is what the previous header was worried about.
    function scrollFocusIntoView() {
        if (!root.itemFocused)
            return
        root.positionViewAtIndex(root.focusedItem, GridView.Contain)
    }

    Connections {
        target: QbzKioskNav

        function onIndexChanged() {
            Qt.callLater(root.scrollFocusIntoView)
        }

        // Enter pulse → open the focused album.
        function onActivateSeqChanged() {
            if (root.itemFocused)
                root.open(root.albums[root.focusedItem].id)
        }
    }

    // ── the visible window ─────────────────────────────────────────────────
    // `originY` is read rather than assumed: a view with a top margin does not
    // put its first row at content y 0, and hardcoding either convention is
    // how this kind of arithmetic goes quietly wrong.
    function _rowAt(y) {
        if (root.cellHeight <= 0)
            return 0
        return Math.floor((y - root.originY) / root.cellHeight)
    }

    readonly property int firstVisible: (root.albums.length === 0 || root.columns <= 0)
        ? -1
        : Math.max(0, Math.min(root.albums.length - 1,
            Math.max(0, root._rowAt(root.contentY)) * root.columns))
    readonly property int lastVisible: (root.albums.length === 0 || root.columns <= 0)
        ? -1
        : Math.max(root.firstVisible, Math.min(root.albums.length - 1,
            (Math.max(0, root._rowAt(root.contentY + root.height - 1)) + 1) * root.columns - 1))

    /// The band that actually HAS delegates: the visible window widened by
    /// `overscanRows` on each side, which is what `cacheBuffer` mounts.
    ///
    /// AN ARTWORK CONSUMER WANTS THIS ONE, not `firstVisible`/`lastVisible`. A
    /// card in the overscan is already mounted and already asking for a cover;
    /// reporting only the viewport would leave exactly those rows blank at the
    /// moment they scroll in, which is the visible half of the "only the first
    /// band of covers ever resolves" bug this grid is fixing. The signal
    /// carries the viewport pair because that is what "visible window" means;
    /// widen with these when reporting keys.
    readonly property int firstMounted: root.firstVisible < 0 ? -1
        : Math.max(0, root.firstVisible - root.overscanRows * root.columns)
    readonly property int lastMounted: root.lastVisible < 0 ? -1
        : Math.min(root.albums.length - 1, root.lastVisible + root.overscanRows * root.columns)

    property int _emittedFirst: -2
    property int _emittedLast: -2

    function _emitVisibleWindow() {
        if (root.firstVisible === root._emittedFirst && root.lastVisible === root._emittedLast)
            return
        root._emittedFirst = root.firstVisible
        root._emittedLast = root.lastVisible
        root.visibleWindow(root.firstVisible, root.lastVisible)
        root.windowChanged(root.firstVisible, root.lastVisible)
    }

    // Coalesced: several of these fire on one flick frame (contentY moves,
    // then both bounds recompute), and the host should see one call.
    onFirstVisibleChanged: Qt.callLater(root._emitVisibleWindow)
    onLastVisibleChanged: Qt.callLater(root._emitVisibleWindow)
    // A new model over the same indices is still a new window for the host's
    // artwork keys, so the change gate is re-armed rather than skipped.
    onAlbumsChanged: {
        root._emittedFirst = -2
        root._emittedLast = -2
        Qt.callLater(root._emitVisibleWindow)
    }
    Component.onCompleted: Qt.callLater(root._emitVisibleWindow)

    delegate: Item {
        id: cell

        required property var modelData
        required property int index

        // The cell is the PITCH; the card is the pitch minus the gap, laid at
        // the origin, so the gap falls on the right and bottom exactly as the
        // absolute layout placed it.
        width: root.cellWidth
        height: root.cellHeight

        KioskCard {
            artSize: root.cardW
            album: cell.modelData
            navFocused: root.itemFocused && root.focusedItem === cell.index
            onClicked: function (id) {
                root.open(id)
            }
        }
    }
}
