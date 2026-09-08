// AlbumCollection — QML port of discover/AlbumCollectionView.slint: the
// rendered album collection (flat grid, flat list, or group-by sections),
// WITHOUT the surrounding toolbar / loading / empty / load-more states. The
// host owns those, exactly as in the .slint, so the column math and the
// list header+rows live in one place and are shared by Discover Browse,
// Label Releases and the two play-history pages.
//
// (views/local/LocalAlbumCollection.qml is the LOCAL twin: same idea, but
// its rows are LocalAlbumRow and it is coupled to LocalLibraryView for the
// skeleton pulse and the per-item artwork gate. Catalog pages have neither,
// so this is the catalog-side collection rather than a fifth arm bolted onto
// that one.)
//
// VIEW MODE: the flat grid mounts on `viewMode !== "list"`. The .slint tests
// `== "grid"` (:294) and therefore renders NOTHING for an empty view-mode —
// a trap the Rust seeding avoids anyway, but there is no reason to reproduce
// a blank page here.
//
// WINDOWING (flat grid/list, opt-in via `flick`): only rows within a bounded
// runway around the visible band exist. The collection keeps the FULL footprint
// as one cheap Item, so its height — and therefore the scroll geometry — never
// changes, but there is no longer one Loader shell per off-screen result.
// Scroll events only run a constant-time COVERAGE GUARD. It rebuilds the slice
// synchronously when the viewport consumes its inner runway; that matters
// because a delayed sampler can leave the viewport outside the mounted slice
// for one or more completely blank frames during a fast wheel/touchpad jump.
// The wider outer runway means the slice still changes roughly once per
// viewport, not once per scroll frame. Grouped sections remain eager on
// DESKTOP — the same call the .slint makes (:265-277); under `kioskHost` they
// are windowed too, against a pixel band, because a label grouped by artist
// otherwise mounts every card of every section (contract §3.3).
//
// KIOSK (`kioskHost`, default false): the flag forces the LIST arm, tightens
// the overscan and asks the artwork bridge for the smallest provider bucket
// that covers the 44px thumb. See the property's own block below.
//
// TAIL FADE (opt-in via armTailFade(), see the block below): the arrival half
// of the Load-more round. OFF unless a host arms it, so the two hosts that do
// not (Discover Browse, Play History) are bit-for-bit unchanged.

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz
import "../assets/kiosk-art.js" as ArtPolicy
import "../cards"
import "../controls"
import "../theme"

Column {
    id: root

    /// Flat list of home_qt::HomeCard rows.
    property var albums: []
    /// [{ title, albums: [...] }] — only read while `isGrouped`.
    property var grouped: []
    property bool isGrouped: false
    /// "grid" | "list".
    property string viewMode: "grid"
    property int cardWidth: 200
    property int cardHeight: 266
    property int cardGap: 24
    property int listRowGap: 4
    /// Render the "{} plays" line (Most Played Albums only — AlbumCard.slint
    /// :508 shows it when the card carries a non-zero count, and every other
    /// surface publishes 0).
    property bool showPlays: false

    /// The scrolling host. Set it to enable grid windowing; leave null and
    /// every card mounts (the right choice for a bounded set like the 24-item
    /// play history).
    property Flickable flick: null
    /// Approximate y of this collection inside the host's content — the
    /// .slint's `content-offset`.
    property real contentOffset: 0

    // --- Kiosk host arm (contract §2.6 / §3.3 / §5.1 / §5.2) --------------
    //
    // DEFAULT FALSE IS DESKTOP, and every branch below is `kioskHost ? … : <the
    // old literal>`, so an unset host is bit-for-bit the file that shipped.
    // Only ContentRouter's kiosk `setSource` map ever sets it.
    //
    // What the flag buys, and why the LIST arm rather than a smaller card:
    // the kiosk content pane is 784 x 256 at the 800x480 Pi floor, and a card
    // is 200 x 266 — one card does not fit VERTICALLY, at any column count.
    // The list arm is already windowed on AlbumListRow (64px rows, a 44px
    // thumb, a 346px title column at this width), already meets the 64px
    // primary touch target, and keeps its ⋯ menu permanently visible instead
    // of behind AlbumCard's hover scrim. So kiosk forces list mode instead of
    // reflowing a card this file does not own.
    property bool kioskHost: false
    /// The mode every arm below reads. `viewMode` stays the HOST's property
    /// (the toolbar toggle still writes it, and Back/Forward still restores
    /// it) — kiosk only overrides what is rendered from it.
    readonly property string _mode: root.kioskHost ? "list" : root.viewMode
    /// Overscan, in viewports, before/after the visible band. Desktop keeps
    /// its 2/3 runway; kiosk uses one row before/after the viewport — contract §5.1 asks for
    /// "viewport + up to two rows" and a 256px viewport under a 5-viewport
    /// runway mounts ~15 rows to show 3.
    readonly property int _overscanBack: root.kioskHost ? 1 : 2
    readonly property int _overscanFwd: root.kioskHost ? 2 : 3
    /// The list arm's row pitch, in one place (AlbumListRow is 64 tall).
    readonly property int _rowPitch: 64 + root.listRowGap

    QbzTheme { id: theme }

    width: parent ? parent.width : 0
    spacing: 0

    // --- Windowing band (row indices) ------------------------------------
    property var band: ({first: 0, last: 0})
    readonly property int bandFirst: band.first
    // Start bounded too: the first model publish may precede
    // Component.onCompleted, and an eager sentinel would briefly instantiate
    // the entire result set before the first sample replaces it.
    readonly property int bandLast: band.last
    // Grouped mode used to be EXEMPT from windowing here ("Grouped sections
    // remain eager — the same call the .slint makes"), which on a label
    // grouped by artist mounts every card of every section. Kiosk cannot
    // afford that (contract §3.3), so under `kioskHost` the grouped arm gets
    // its own pixel band below and joins the windowed set. Desktop keeps the
    // eager sections it always had.
    readonly property bool windowed: flick !== null
        && (!root.isGrouped || root.kioskHost)

    function sampleBand() {
        if (!root.windowed)
            return
        if (root.isGrouped) {
            // kioskHost-only by `windowed` above.
            root.sampleGroupBand()
            return
        }
        var listMode = root._mode === "list"
        var pitch = listMode ? root._rowPitch
                             : root.cardHeight + root.cardGap
        // The list rows begin after their 32px column header. The grid starts
        // at the collection origin.
        var rowsTop = root.contentOffset + (listMode ? 32 : 0)
        var top = root.flick.contentY - rowsTop
        var h = root.flick.height
        if (root.kioskHost && (top + h <= 0 || top >= root.albums.length * pitch)) {
            root.band = {first: 0, last: -1}; return
        }
        // Two viewports on each side (one, forward two, in kiosk).
        // ensureBandCoverage() refreshes when the viewport enters the inner
        // half, leaving a full viewport mounted even after the refresh edge
        // and enough lead for image incubation.
        root.band = {first: Math.max(0, Math.floor((top - (root.kioskHost ? pitch : root._overscanBack * h)) / pitch)),
            last: Math.max(0, Math.ceil((top + (root.kioskHost ? h : root._overscanFwd * h)) / pitch))}
    }

    function refreshBand() {
        if (root.windowed && root.visible) {
            root.sampleBand()
            root.reportArtWindow()
        }
    }

    function ensureBandCoverage() {
        if (root.kioskHost) { root.refreshBand(); return }
        if (!root.windowed || !root.visible)
            return
        if (root.isGrouped) {
            root.ensureGroupCoverage()
            return
        }
        var listMode = root._mode === "list"
        var pitch = listMode ? root._rowPitch
                             : root.cardHeight + root.cardGap
        var rowsTop = root.contentOffset + (listMode ? 32 : 0)
        var top = root.flick.contentY - rowsTop
        var h = Math.max(1, root.flick.height)
        var cols = listMode ? 1 : Math.max(1, Math.floor(
            (root.width + root.cardGap) / (root.cardWidth + root.cardGap)))
        var totalRows = Math.ceil(root.albums.length / cols)
        if (totalRows <= 0 || top + h <= 0 || top >= totalRows * pitch)
            return
        var visibleFirst = Math.max(0, Math.floor(top / pitch))
        var visibleLast = Math.max(0, Math.ceil((top + h) / pitch))
        var innerRunway = root.kioskHost ? 1 : Math.max(1, Math.ceil(h / pitch))
        // A scrollbar seek may jump across the whole slice in one signal;
        // normal wheel motion reaches an inner edge about once per viewport.
        if (visibleFirst < root.bandFirst || visibleLast > root.bandLast
                || (visibleFirst > innerRunway
                    && visibleFirst - root.bandFirst < innerRunway)
                || (visibleLast < totalRows - innerRunway
                    && root.bandLast - visibleLast < innerRunway))
            root.refreshBand()
    }

    // --- Grouped windowing (kiosk) ---------------------------------------
    //
    // Sections have UNEQUAL heights, so there is no single row pitch to index
    // by the way the flat arms do. The band is therefore kept in PIXELS, and
    // `_groupPlan` precomputes one record per section — O(sections), no object
    // per album. Both the sections and, inside each mounted section, its rows
    // are then windowed against that band, so neither a 900-artist grouping
    // nor a 900-album section can mount more than the runway.
    //
    // Sorting, section titles and per-row actions are untouched: this changes
    // WHICH delegates exist, never the model the host derived.
    property var groupBand: ({top: 0, bottom: 0})
    readonly property real groupTop: groupBand.top
    readonly property real groupBottom: groupBand.bottom
    /// Section rows start below the one shared AlbumListHeader (32px).
    readonly property real _groupRowsTop: root.contentOffset + 32
    readonly property int _sectionTitleH: 34
    readonly property int _sectionGap: 20

    function sampleGroupBand() {
        var h = Math.max(1, root.flick.height)
        var top = root.flick.contentY - root._groupRowsTop
        root.groupBand = {top: top - root._rowPitch, bottom: top + h + root._rowPitch}
    }

    function ensureGroupCoverage() {
        var h = Math.max(1, root.flick.height)
        var top = root.flick.contentY - root._groupRowsTop
        // Same inner-runway rule as the flat arm, expressed in pixels. At rest
        // it is self-consistent (a freshly sampled band satisfies neither
        // clause), so this cannot loop on a stationary viewport.
        if (top < root.groupTop + root._rowPitch || top + h > root.groupBottom - root._rowPitch)
            root.refreshBand()
    }

    /// [{ index, title, y, rowsY, count, h }] in the collection's own
    /// coordinates, below the column header. Empty on desktop and whenever the
    /// page is not grouped — the binding then costs one comparison.
    readonly property var _groupPlan: {
        if (!root.kioskHost || !root.isGrouped)
            return []
        var pitch = root._rowPitch
        var out = []
        var y = 0
        for (var i = 0; i < root.grouped.length; i++) {
            var g = root.grouped[i] || ({})
            var n = (g.albums || []).length
            var body = n > 0 ? n * pitch - root.listRowGap : 0
            out.push({ "index": i, "title": g.title || "", "y": y,
                       "rowsY": root._sectionTitleH, "count": n,
                       "h": root._sectionTitleH + body })
            y += root._sectionTitleH + body + root._sectionGap
        }
        return out
    }
    readonly property real _groupHeight: {
        var p = root._groupPlan
        if (p.length === 0)
            return 0
        var last = p[p.length - 1]
        return last.y + last.h
    }

    /// The sections that intersect the band, each with the row slice of its
    /// own list that does. One pass over the plan, no album touched.
    readonly property var _mountedSections: {
        var plan = root._groupPlan
        var out = []
        if (plan.length === 0)
            return out
        var pitch = root._rowPitch
        var lo = root.groupBand.top
        var hi = root.groupBand.bottom
        for (var i = 0; i < plan.length; i++) {
            var s = plan[i]
            if (s.y + s.h < lo || s.y > hi)
                continue
            var base = s.y + s.rowsY
            var from = Math.max(0, Math.min(s.count,
                Math.floor((lo - base) / pitch)))
            var to = Math.max(from, Math.min(s.count,
                Math.ceil((hi - base) / pitch) + 1))
            out.push({ "index": s.index, "title": s.title, "y": s.y,
                       "rowsY": s.rowsY, "from": from, "to": to })
        }
        return out
    }

    Connections {
        target: root.flick
        ignoreUnknownSignals: true
        function onContentYChanged() { root.ensureBandCoverage() }
        function onHeightChanged() { root.refreshBand() }
    }

    Component.onCompleted: root.refreshBand()
    onWidthChanged: root.refreshBand()
    onViewModeChanged: root.refreshBand()
    onVisibleChanged: root.refreshBand()

    // --- Windowed artwork -------------------------------------------------
    //
    // The delegates were already windowed; their COVERS were not. Rust fetched
    // every missing cover of the whole page in one batch and then republished
    // the entire document to attach the paths (`browse_qt::refresh_*_art`), so
    // on a View-all page nothing had artwork until everything did — and the
    // republish rebuilt every delegate on arrival. That is the same shape
    // Library > All was fixed out of: ask for what is near the viewport, and
    // take the answers ONE KEY AT A TIME so the model is never re-handed.
    //
    // Purely ADDITIVE for the hosts that do not window (they pass no `flick`):
    // nothing reports, `artMap` stays empty, and `artOf` falls through to the
    // `artPath` those pages already publish.
    //
    // `QbzShell.sidebarArtworkWindow` is the existing generic seam — it
    // resolves what is already on disk, downloads the rest and emits
    // `libraryArtworkReady` per url. It classifies local paths too, so the
    // local tabs that host this collection are served by the same call.
    property var artMap: ({})
    /// Urls already handed to the bridge. A NON-notifying holder on purpose:
    /// several scroll samples may cover the same window before a download
    /// lands, and none should dispatch the same unresolved cover twice.
    readonly property var _artAsked: ({ seen: ({}) })

    /// The KEY this collection asks the bridge for, and the key `artMap` is
    /// indexed by. Desktop is the raw `artUrl` — unchanged. Kiosk rewrites it
    /// to the smallest provider bucket that covers the 44px thumb (contract
    /// §5.2: "Kiosk solicita el bucket más pequeño que cubra el cuadro
    /// físico"), through the SAME policy KioskArtwork and KioskCoverSource
    /// use, so the three agree on one cache entry. A non-Qobuz or local url
    /// falls through `sizedUrl` untouched.
    readonly property real _dpr: Screen.devicePixelRatio > 0 ? Screen.devicePixelRatio : 1
    readonly property int _artPx: root.kioskHost ? Math.ceil(44 * root._dpr) : 0
    function _artKey(m) {
        var u = m && m.artUrl ? m.artUrl : ""
        if (u === "" || !root.kioskHost)
            return u
        return ArtPolicy.sizedUrl(u, root._artPx)
    }

    function artOf(m) {
        if (!m)
            return ""
        var key = root._artKey(m)
        if (key !== "" && root.artMap[key])
            return root.artMap[key]
        return m.artPath || ""
    }

    /// One album -> at most one pending key. Shared by the flat and grouped
    /// passes so they cannot drift.
    function _collectArt(a, pending, asked) {
        if (!a)
            return
        // Already on disk at publish time, already resolved, or already asked
        // for — all three mean there is nothing to request.
        if ((a.artUrl || "") === "" || (a.artPath || "") !== "")
            return
        var key = root._artKey(a)
        if (key === "" || root.artMap[key] || asked[key] === true)
            return
        asked[key] = true
        pending.push(key)
    }

    function reportArtWindow() {
        if (!root.windowed)
            return
        var pending = []
        var asked = root._artAsked.seen
        var s, r, list
        if (root.isGrouped) {
            // kioskHost-only (see `windowed`): the mounted slice of each
            // mounted section, and nothing else.
            var ms = root._mountedSections || []
            for (s = 0; s < ms.length; s++) {
                list = (root.grouped[ms[s].index] || ({})).albums || []
                for (r = ms[s].from; r < ms[s].to; r++)
                    root._collectArt(list[r], pending, asked)
            }
        } else {
            var listMode = root._mode === "list"
            var cols = listMode ? 1 : flatGrid.columns
            if (cols <= 0)
                return
            var lo = Math.max(0, root.bandFirst * cols)
            var hi = Math.min(root.albums.length - 1,
                              (root.bandLast + 1) * cols - 1)
            for (var i = lo; i <= hi; i++)
                root._collectArt(root.albums[i], pending, asked)
        }
        if (pending.length > 0)
            QbzShell.sidebarArtworkWindow(JSON.stringify(pending))
    }

    Connections {
        target: QbzLibrary
        // Shared with every other consumer of this signal — ignore keys that
        // are not ours.
        function onLibraryArtworkReady(key, path) {
            if (root.artMap[key] === path || root._artAsked.seen[key] !== true)
                return
            var m = root.artMap
            m[key] = path
            // Rebinding needs a NEW object reference; a same-ref assignment is
            // not a change in QML.
            root.artMap = Object.assign({}, m)
        }
    }

    // --- Tail fade: the appended page APPEARS, it does not pop -------------
    // The owner's second ask for the Load-more round (2026-08-02): "que la
    // aparicion de lo que se cargue, sea smooth". controls/QbzLoadMore.qml
    // owns the WAITING half (the skeleton under the button); the ARRIVAL half
    // can only live here — only the collection knows which of its delegates
    // are new. A host asks for it by calling armTailFade() immediately BEFORE
    // the bridge call that fetches the next page.
    //
    // WHY AN ID SET AND NOT "index >= the old count": this collection is
    // SORTED by its host (LabelReleasesView's sort control does newest /
    // oldest / title / artist, src/label_qt.rs::sort_cards). Only under
    // "newest" does a fetched page land as a tail; under any other order the
    // new albums INTERLEAVE, and an index threshold would then re-fade albums
    // that never moved — the very flicker this exists to avoid. Membership in
    // the pre-fetch id set is the honest question, and it costs one JS object
    // of N keys per click. It also lets GROUPED mode fade correctly, where
    // there is no flat index at all: a fetched album is folded into whichever
    // artist section it belongs to, anywhere in the page.
    //
    // WHY BOTH LISTS: `visible` and `grouped` are mutually exclusive in Rust —
    // derive_releases returns `Vec::new()` for the flat list as soon as
    // group-by is on (label_qt.rs:923). Snapshotting only `albums` would
    // capture an EMPTY set in grouped mode and every card on screen would read
    // as "new".
    //
    // INERT BY DEFAULT: a host that never arms leaves `_tailSeen` null, every
    // delegate's opacity short-circuits to 1.0, `_tailReveal` is never read
    // (so it is never even a binding dependency) and no animation exists.
    property var _tailSeen: null
    property real _tailReveal: 1.0

    /// Host-supplied identity of the page on screen (a label id, an artist
    /// id…). Changing it disarms a pending fade, so a click that never landed
    /// cannot colour the NEXT page's first paint. "" = the hosts that do not
    /// use the fade at all.
    property string collectionKey: ""
    onCollectionKeyChanged: root.clearTailFade()

    function _addIds(seen, list) {
        for (var i = 0; i < list.length; i++) {
            var a = list[i]
            if (a && a.id !== undefined && a.id !== null && a.id !== "")
                seen[a.id] = true
        }
    }

    /// Snapshot what is on screen NOW. Call it BEFORE the bridge call: the
    /// bridge may republish synchronously.
    function armTailFade() {
        var seen = {}
        root._addIds(seen, root.albums)
        for (var g = 0; g < root.grouped.length; g++)
            root._addIds(seen, root.grouped[g].albums || [])
        // Arming ALSO parks the reveal at 0. A delegate for a new id can be
        // built by a Repeater's model binding BEFORE onAlbumsChanged runs —
        // both hang off the same change signal and the order between them is
        // not ours to choose — and it must never be painted at full opacity
        // for one frame before the fade starts.
        root._tailReveal = 0.0
        root._tailSeen = seen
    }

    /// Disarm without fading.
    function clearTailFade() {
        root._tailSeen = null
        root._tailReveal = 1.0
    }

    /// 1.0 for everything the arm already saw — and for every host that never
    /// armed, and for a card with no id at all (an unknown id must fail
    /// OPAQUE, never invisible).
    function tailOpacity(id) {
        if (!root._tailSeen || id === undefined || id === null || id === "")
            return 1.0
        return root._tailSeen[id] === true ? 1.0 : root._tailReveal
    }

    function _hasFreshId(list) {
        for (var i = 0; i < list.length; i++) {
            var a = list[i]
            if (a && a.id !== undefined && a.id !== null && a.id !== ""
                    && root._tailSeen[a.id] !== true)
                return true
        }
        return false
    }

    function _maybeStartTailFade() {
        if (!root._tailSeen)
            return
        var fresh = root._hasFreshId(root.albums)
        for (var g = 0; !fresh && g < root.grouped.length; g++)
            fresh = root._hasFreshId(root.grouped[g].albums || [])
        // A republish with no new id — an artwork batch, a favourite toggle, a
        // sort flip, a group-by flip — is NOT the page landing. Stay armed and
        // stay silent; re-fading what is already on screen is the regression.
        if (fresh)
            tailFade.restart()
    }

    onAlbumsChanged: {
        root._maybeStartTailFade()
        // A new page appended -> recompute the exact mounted slice before
        // asking for its covers. ONE handler per signal: declaring a second
        // `onAlbumsChanged` is a hard qmlcachegen error.
        root.refreshBand()
    }
    onGroupedChanged: {
        root._maybeStartTailFade()
        // Grouped is windowed under `kioskHost` (see `windowed`), so a
        // republished grouping has to re-sample before its covers are asked
        // for — the same reason `onAlbumsChanged` does.
        root.refreshBand()
    }

    NumberAnimation {
        id: tailFade
        target: root
        property: "_tailReveal"
        from: 0.0
        to: 1.0
        // 220ms / OutCubic is the content half of the Load-more round (the
        // skeleton half uses the tree's 180ms — QbzSkeleton.qml:216).
        duration: 220
        easing.type: Easing.OutCubic
        // Disarm on the way out, so the NEXT republish finds no arm at all.
        // `finished` is emitted only on a natural end — a restart() stops the
        // animation without it, which is exactly what we want mid-flight.
        onFinished: root.clearTailFade()
    }

    // One group-by section's card grid (FULL mounts — see the header note).
    // Declared before its use for readability; QML registers inline
    // components document-wide either way.
    component SectionGrid: Item {
        id: sg
        property var items: []
        /// Tail fade, PASSED IN rather than read off `root`: an inline
        /// `component` does not share the document's scope the way a plain
        /// Component does (controls/QbzSkeleton.qml:269-270 spells this out),
        /// so the mount site — which is in file scope — hands it the two
        /// values. Null / 1.0 = no fade, which is every other host.
        property var seenIds: null
        property real reveal: 1.0
        width: parent ? parent.width : 0
        readonly property int columns: Math.max(
            1, Math.floor((width + root.cardGap) / (root.cardWidth + root.cardGap)))
        readonly property int rows: Math.ceil(sg.items.length / sg.columns)
        height: sg.rows > 0
            ? sg.rows * root.cardHeight + (sg.rows - 1) * root.cardGap
            : 0

        Repeater {
            model: sg.items
            delegate: Item {
                id: gcell
                required property var modelData
                required property int index
                x: (gcell.index % sg.columns) * (root.cardWidth + root.cardGap)
                y: Math.floor(gcell.index / sg.columns) * (root.cardHeight + root.cardGap)
                width: root.cardWidth
                height: root.cardHeight
                // Same rule as the flat grid, expressed with the passed-in
                // pair (see `seenIds`): an album the arm already saw is
                // opaque, so only a fetched one fades.
                opacity: (sg.seenIds && gcell.modelData && gcell.modelData.id
                          && sg.seenIds[gcell.modelData.id] !== true)
                    ? sg.reveal : 1.0
                AlbumCard {
                    albumId: gcell.modelData.id
                    source: gcell.modelData.source || ""
                    sources: gcell.modelData.sources || []
                    title: gcell.modelData.title
                    artist: gcell.modelData.artist
                    artistId: gcell.modelData.historyArtistLink ? "" : gcell.modelData.artistId
                    hostArtistLink: gcell.modelData.historyArtistLink === true && (gcell.modelData.artist || "").trim() !== ""
                    onArtistRequested: QbzHome.openHistoryAlbumArtist(
                        gcell.modelData.id, gcell.modelData.source || "", gcell.modelData.artist || "", gcell.modelData.artistId || "")
                    genre: gcell.modelData.genre
                    year: gcell.modelData.year
                    qualityTier: gcell.modelData.qualityTier
                    qualityDetail: gcell.modelData.qualityDetail || ""
                    ribbon: gcell.modelData.ribbon || ""
                    ribbonKind: gcell.modelData.ribbonKind || ""
                    artSource: root.artOf(gcell.modelData)
                    isPinned: gcell.modelData.isPinned === true
                    // Snapshot url the pin payload persists (artPath is the
                    // local cache path — see AlbumCard.artworkUrl).
                    artworkUrl: gcell.modelData.artUrl || ""
                    // Stamped on the row (AlbumCardData / HomeCard); false
                    // made the glyph lie and inverted the first click.
                    isFavorite: gcell.modelData.isFavorite === true
                }
            }
        }
    }

    // --- Grouped sections (desktop: EAGER, as the .slint is) --------------
    Column {
        visible: !root.kioskHost && root.isGrouped && root.grouped.length > 0
        width: parent.width
        spacing: 0

        // In list mode the column header sits ONCE above all sections.
        AlbumListHeader { visible: root._mode === "list" }

        Repeater {
            // `visible: false` is not lazy mounting (contract §3.3): the model
            // itself has to go empty, or the kiosk host pays for every section
            // of a grouping it never draws.
            model: root.isGrouped && !root.kioskHost ? root.grouped : []
            delegate: Column {
                id: sectionCol
                required property var modelData
                required property int index
                width: root.width
                spacing: 12

                Item { visible: sectionCol.index > 0; width: 1; height: 24 }
                Text {
                    text: sectionCol.modelData.title || ""
                    color: theme.textPrimary
                    font.pixelSize: theme.fontHeading
                    font.weight: theme.weightSemibold
                }
                // List arm.
                Column {
                    visible: root._mode === "list"
                    width: parent.width
                    spacing: root.listRowGap
                    Repeater {
                        model: root._mode === "list" ? (sectionCol.modelData.albums || []) : []
                        delegate: AlbumListRow {
                            required property var modelData
                            required property int index
                            item: modelData
                            rowIndex: index
                            // The row prefers an explicit source and falls back
                            // to `item.artPath`, so handing it the windowed map
                            // costs nothing when the map is empty.
                            artSource: root.artOf(modelData)
                            // Plain Component, file scope — `root` resolves
                            // here, unlike inside SectionGrid.
                            opacity: root.tailOpacity(modelData ? modelData.id : "")
                        }
                    }
                }
                // Grid arm.
                SectionGrid {
                    visible: root._mode !== "list"
                    items: sectionCol.modelData.albums || []
                    seenIds: root._tailSeen
                    reveal: root._tailReveal
                }
            }
        }
    }

    // --- Grouped sections (kiosk: WINDOWED) -------------------------------
    // Same data, same section titles, same per-row menu — only the mounted
    // set differs. Positions come from `_groupPlan`, so the footprint (and
    // therefore the host's scroll geometry) is stable whatever is mounted.
    Item {
        id: groupedKiosk
        visible: root.kioskHost && root.isGrouped && root.grouped.length > 0
        width: parent.width
        height: groupedKiosk.visible
            ? groupedKioskHeader.height + root._groupHeight : 0

        AlbumListHeader { id: groupedKioskHeader }

        Repeater {
            model: groupedKiosk.visible ? root._mountedSections.length : 0
            delegate: Item {
                id: gsec
                required property int index
                readonly property var plan: root._mountedSections[gsec.index] || ({})
                readonly property var rows: (root.grouped[gsec.plan.index] || ({})).albums || []

                x: 0
                y: groupedKioskHeader.height + (gsec.plan.y || 0)
                width: groupedKiosk.width
                height: root._sectionTitleH + (gsec.rows.length > 0
                    ? gsec.rows.length * root._rowPitch - root.listRowGap : 0)

                Text {
                    width: parent.width
                    height: root._sectionTitleH
                    verticalAlignment: Text.AlignVCenter
                    text: gsec.plan.title || ""
                    color: theme.textPrimary
                    font.pixelSize: theme.fontHeading
                    font.weight: theme.weightSemibold
                    elide: Text.ElideRight
                }

                Repeater {
                    model: Math.max(0, (gsec.plan.to || 0) - (gsec.plan.from || 0))
                    delegate: AlbumListRow {
                        required property int index
                        readonly property int globalRow: (gsec.plan.from || 0) + index
                        readonly property var cardData: gsec.rows[globalRow] || ({})
                        x: 0
                        y: (gsec.plan.rowsY || 0) + globalRow * root._rowPitch
                        width: gsec.width
                        kioskHost: root.kioskHost
                        item: cardData
                        artSource: root.artOf(cardData)
                        rowIndex: globalRow
                        // Plain Component, file scope — `root` resolves here,
                        // unlike inside SectionGrid.
                        opacity: root.tailOpacity(cardData.id || "")
                    }
                }
            }
        }
    }

    // --- Flat list (windowed) ---------------------------------------------
    Item {
        id: flatList
        visible: !root.isGrouped && root._mode === "list" && root.albums.length > 0
        width: parent.width
        readonly property int rowPitch: root._rowPitch
        readonly property int mountedFrom: root.windowed
            ? Math.min(root.albums.length, Math.max(0, root.bandFirst)) : 0
        readonly property int mountedTo: root.windowed
            ? Math.min(root.albums.length, Math.max(mountedFrom, root.bandLast + 1))
            : root.albums.length
        readonly property int mountedCount: root.kioskHost && root.windowed
            ? Math.max(0, Math.min(root.albums.length, root.band.last + 1) - Math.min(root.albums.length, root.band.first))
            : Math.max(0, mountedTo - mountedFrom)
        height: 32 + (root.albums.length > 0
            ? root.albums.length * 64 + (root.albums.length - 1) * root.listRowGap
            : 0)

        AlbumListHeader { id: flatListHeader }
        Repeater {
            model: flatList.visible ? flatList.mountedCount : 0
            delegate: AlbumListRow {
                required property int index
                readonly property int globalIndex: flatList.mountedFrom + index
                readonly property var cardData: root.albums[globalIndex] || ({})
                x: 0
                y: flatListHeader.height + globalIndex * flatList.rowPitch
                width: flatList.width
                kioskHost: root.kioskHost
                item: cardData
                artSource: root.artOf(cardData)
                rowIndex: globalIndex
                opacity: root.tailOpacity(cardData.id || "")
            }
        }
    }

    // --- Flat grid (windowed) --------------------------------------------
    Item {
        id: flatGrid
        visible: !root.isGrouped && root._mode !== "list" && root.albums.length > 0
        width: parent.width
        readonly property int columns: Math.max(
            1, Math.floor((width + root.cardGap) / (root.cardWidth + root.cardGap)))
        readonly property int rows: Math.ceil(root.albums.length / flatGrid.columns)
        readonly property int mountedFrom: root.windowed
            ? Math.min(root.albums.length,
                       Math.max(0, root.bandFirst * flatGrid.columns)) : 0
        readonly property int mountedTo: root.windowed
            ? Math.min(root.albums.length,
                       Math.max(mountedFrom,
                                (root.bandLast + 1) * flatGrid.columns))
            : root.albums.length
        readonly property int mountedCount: root.kioskHost && root.windowed
            ? Math.max(0, Math.min(root.albums.length, root.band.last + 1) - Math.min(root.albums.length, root.band.first))
            : Math.max(0, mountedTo - mountedFrom)
        height: flatGrid.rows > 0
            ? flatGrid.rows * root.cardHeight + (flatGrid.rows - 1) * root.cardGap
            : 0

        Repeater {
            model: flatGrid.visible ? flatGrid.mountedCount : 0
            delegate: Item {
                id: cell
                required property int index
                readonly property int globalIndex: flatGrid.mountedFrom + index
                readonly property var cardData: root.albums[globalIndex] || ({})
                readonly property int rowIndex: Math.floor(globalIndex / flatGrid.columns)
                x: (globalIndex % flatGrid.columns) * (root.cardWidth + root.cardGap)
                y: cell.rowIndex * (root.cardHeight + root.cardGap)
                width: root.cardWidth
                height: root.cardHeight
                // Tail fade (see the block at the top). A LIVE binding, never
                // a Component.onCompleted write: this delegate is destroyed
                // and rebuilt on every republish, and the binding gives the
                // rebuilt cell the right value at creation instead of a frame
                // at the wrong one. Everything the arm saw — and everything,
                // on every host that never arms — reads 1.0 and never even
                // depends on `_tailReveal`.
                opacity: root.tailOpacity(cell.cardData.id || "")

                // Numeric slice windowing reuses this viewport slot for a new
                // global index. AlbumCard's optimistic heart/pin assignments
                // intentionally break their host bindings; explicitly restore
                // those bindings when the slot changes identity so state from
                // the old album can never leak into the new one.
                onCardDataChanged: {
                    if (!mountedCard)
                        return
                    mountedCard.isPinned = Qt.binding(function () {
                        return cell.cardData.isPinned === true
                    })
                    mountedCard.isFavorite = Qt.binding(function () {
                        return cell.cardData.isFavorite === true
                    })
                }

                AlbumCard {
                    id: mountedCard
                    albumId: cell.cardData.id
                    source: cell.cardData.source || ""
                    sources: cell.cardData.sources || []
                    title: cell.cardData.title
                    artist: cell.cardData.artist
                    artistId: cell.cardData.historyArtistLink ? "" : cell.cardData.artistId
                    hostArtistLink: cell.cardData.historyArtistLink === true && (cell.cardData.artist || "").trim() !== ""
                    onArtistRequested: QbzHome.openHistoryAlbumArtist(
                        cell.cardData.id, cell.cardData.source || "", cell.cardData.artist || "", cell.cardData.artistId || "")
                    genre: cell.cardData.genre
                    year: cell.cardData.year
                    qualityTier: cell.cardData.qualityTier
                    qualityDetail: cell.cardData.qualityDetail || ""
                    ribbon: cell.cardData.ribbon || ""
                    ribbonKind: cell.cardData.ribbonKind || ""
                    artSource: root.artOf(cell.cardData)
                    isPinned: cell.cardData.isPinned === true
                    artworkUrl: cell.cardData.artUrl || ""
                    plays: root.showPlays ? (cell.cardData.plays || 0) : 0
                    isFavorite: cell.cardData.isFavorite === true
                }
            }
        }
    }
}
