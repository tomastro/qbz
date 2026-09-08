// ArtistScene — the "artists from this scene" browser, reached from the artist
// page's Origin row ("Born in / Founded in") and from the header ⋯ menu.
//
// THE REFERENCE IS TAURI, NOT SLINT. For this module only, the owner suspended
// TRACK-RULES' "Slint is the reference" rule: `legacy-tauri/src/lib/components/
// views/ArtistsByLocationView.svelte` (1685 lines) is the target, and Slint's
// `location/ArtistsByLocationView.slint` (173 lines) is a heavy simplification
// with no nav bar, no sidepanel, no alpha rail and no error state — it is NOT
// consulted for anything visual here. Every number below is cited to the
// Svelte as `V:<line>` (markup) or `S:<line>` (its <style> block); the geometry
// table is 01-tauri-artist-scene.md §17 and the contract is
// qbz-nix-docs/qt-frontend/2026-08-14-artist-scene-musician/00-CONTRACT.md.
//
// ANATOMY (V:422-819), top to bottom:
//   root padding 0 8 0 18 (S:823-833) · [top bar — DROPPED, see DV-15] ·
//   compact sticky header (V:433-443) · nav bar (V:447-601) · content, five
//   mutually exclusive states in Tauri's own branch order (V:606,633,640,645,
//   713): loading -> error -> empty -> grid -> sidepanel.
// The hero (140px flag + 28px title) is NOT a sibling of the nav bar: it is the
// grid's scroll HEADER (V:655-676), so the nav bar stays pinned while the hero
// scrolls away. In sidepanel mode the hero does not render at all.
//
// DELIBERATE DIVERGENCES FROM TAURI (contract §6 — none may be "corrected
// back" without reopening the contract):
//  - DV-15  NO per-view back button. Tauri's `.top-bar` holds one and nothing
//           else (V:425-430), so the whole region is gone: Qt's back/forward
//           live once in the shell HeaderBar (QbzNavButton, HeaderBar.qml:300-
//           311). What replaces it here is the 22px spacer every detail view
//           that dropped its Slint NavButtons row keeps (MixView.qml:123,
//           LabelReleasesView.qml:92). Tauri's row measured ~40px tall
//           (8 margin-top + the 14px line + 12 margin-bottom), so the nav bar
//           sits ~18px higher here than in Tauri. Stated because it is the one
//           place this file's vertical rhythm departs from the reference.
//  - DV-4   the group dropdown closes on an outside click. Tauri's does not
//           (only its sibling genre popup does, V:240-246) — which is what
//           makes it a defect and not a design.
//  - DV-8   `heroScrolledPast` is reset by a grid<->sidepanel switch. In Tauri
//           only the grid mounts the sentinel observer, so the flag keeps
//           whatever value it had and the compact header can stick on or off.
//           Here it is `mode === "grid" && <scrolled>`, i.e. false by
//           construction while the sidepanel is up.
//  - DV-10  the results count does NOT `toLowerCase()` a translated noun
//           (V:451 does, which breaks German "Künstler"). Two msgids carry the
//           whole phrase instead.
//  - DV-11  the text search matches on the TRIMMED query. Tauri tests
//           `searchQuery.trim()` for truthiness but matches with the untrimmed
//           string (V:263), so a trailing space filters everything out.
//  - DV-12  "Browse view" / "Grid view" / "Group: A-Z" / "Group: Off" / "Off" /
//           "Alphabetical (A-Z)" are hardcoded English in Tauri (V:564, :577,
//           :587, :594). They go through tr() here. Because a QBZ msgid IS its
//           English source string (ADR-012), the English output is
//           byte-identical — DV-12 costs nothing visually.
//  - DV-13  the flag is a BUNDLED asset (qml/assets/flags/<cc>.svg, 266 files
//           already in the qrc), not the hatscripts circle-flags CDN Tauri
//           fetches per view (V:161-165). A missing code or a missing file
//           renders NO flag element at all — never a broken-image box.
//  - DV-17  a failed "Load More" does NOT replace the populated view with a
//           full-screen error. Tauri's catch sets `error` unconditionally
//           (V:386), `loadMore` never clears it (V:390-399) and the chain
//           tests error BEFORE grid (V:633 vs :645), so one failed page
//           destroys the grid, the hero and the nav bar and Retry restarts at
//           offset 0. Here the grid stays and the failure is a line in the
//           load-more footer.
//  - DV-18  filtering to zero shows a no-results line. Tauri's empty branch
//           tests the UNFILTERED list (V:640), so a search that matches
//           nothing leaves the hero over a blank viewport with the count
//           reading "0 / N artists".
//  - §4.3   Retry and Load More have NO BORDER. Their CSS says `1px solid
//           var(--border-primary)` (S:1605, S:1641) and `--border-primary` is
//           defined NOWHERE in the Tauri tree, which invalidates the whole
//           shorthand at computed-value time and leaves `border-style: none`.
//           Tauri paints no border on either button; neither does this.
//
// COLOURS — tokens, never literals (contract §4.3): --bg-primary =
// surfaceMain, --bg-secondary = surfaceCard, --bg-tertiary = surfaceElevated,
// --bg-hover = bgHover, --accent-primary = accent, --border-subtle =
// borderSubtle. The three `rgba()` literals Tauri authors (the flag shadow, the
// dropdown shadow, the alpha rail's backplate) are written `Qt.rgba(r,g,b,a)`
// with 0..1 floats: transcribed as 8-digit hex, Qt reads #AARRGGBB where CSS
// wrote #RRGGBBAA, so `rgba(0,0,0,0.4)` -> `#00000066` is FULLY TRANSPARENT.
// That trap has landed twice in this port.
//
// THE PULSE LAW (contract §4.5). Qt Quick has no partial redraws: any dirty
// item presents the WHOLE window at ~1.2% GPU, flat. The loading screen is the
// only continuous animation on this page and it is BESPOKE and pulse-driven —
// neither QbzSpinner (QbzSpinner.qml:19-25, an infinite RotationAnimation) nor
// QbzSkeleton (QbzSkeleton.qml:229-243, a repeating 900ms Timer plus a
// `Behavior on breathe`) may be used, both break the same law. The progress bar
// is data-fed, so it carries NO Behavior; its width is written on each publish.
// While the view is not loading, the whole subtree writes nothing. Hover
// transitions are gesture-bounded and therefore allowed (ArtistCard.qml:170).
//
// WHAT THIS FILE DOES NOT OWN. Three components are a sibling lane's and are
// mounted here against their frozen APIs: scene/SceneGenreFilter.qml,
// scene/AlphaIndexRail.qml, scene/ScenePanelMode.qml. The document shape is
// frozen in contract §13.1 (QbzScene.sceneJson / sceneRev; open, loadMore,
// retry, openArtist, selectArtist, close).

import QtQuick
import QtQuick.Effects
import QtQuick.Window
import com.blitzfc.qbz
import "../cards"
import "../controls"
import "../theme"
import "scene"

Rectangle {
    id: root
    property bool kioskHost: false

    color: ambientOn ? "transparent" : theme.surfaceMain
    readonly property bool ambientOn: theme.ambientOn
    radius: 12

    QbzTheme { id: theme }

    // ONE document, re-parsed on every publish — a fresh string is a fresh
    // object, so the Repeaters/ListView below always see a NEW reference
    // ("Rebind requires a NEW object reference", ArtistReleasesView.qml:65-68).
    // `sceneRev` is read for its dependency alone: the contract has Rust bump
    // it on every publish so a document that happens to serialise to the same
    // string still forces a re-read.
    readonly property var doc: {
        QbzScene.sceneRev
        try {
            return JSON.parse(QbzScene.sceneJson)
        } catch (e) {
            return {}
        }
    }

    // ---- Artist portraits ------------------------------------------------
    // ArtistCard paints `RoundedImage { source: artSource }` — a LOCAL cache
    // path and nothing else (ArtistCard.qml:159-161). It does NOT fetch
    // `artworkUrl`; that property is only carried so a pin snapshot can store
    // the remote url. So the HOST owns the download, and without this block
    // every card in the grid drew the person-glyph placeholder forever, with
    // no error anywhere — which is exactly what shipped in the first pass.
    //
    // Same machinery as the sibling panel (scene/ScenePanelMode.qml:104-135),
    // deliberately duplicated rather than hoisted: the two have different
    // request WINDOWS (a row-band here, an album section there) and sharing a
    // map would make one view's scrolling evict the other's covers.
    //
    // `_asked` is never pruned. A resolved path costs one map entry; dropping
    // it would only make the portrait flicker back to the placeholder on the
    // next scroll.
    property var coverMap: ({})
    property var _coverInbox: ({})
    property var _asked: ({})
    readonly property int coverBatchCap: 48
    // Batched on a 16 ms one-shot: `libraryArtworkReady` fires once PER COVER,
    // and assigning `coverMap` per signal would republish the whole map to
    // every delegate 48 times for one screenful. One dirty frame instead of
    // 48 — the pulse law's concern is presents, and this is the same argument.
    Timer {
        id: coverFlush
        interval: 16
        repeat: false
        onTriggered: {
            var m = Object.assign({}, root.coverMap, root._coverInbox)
            root._coverInbox = ({})
            root.coverMap = m
        }
    }
    Connections {
        target: QbzLibrary
        function onLibraryArtworkReady(key, path) {
            root._coverInbox[key] = path
            if (!coverFlush.running)
                coverFlush.start()
        }
    }
    function requestCovers(urls) {
        var out = []
        for (var i = 0; i < urls.length && out.length < root.coverBatchCap; i++) {
            var u = urls[i]
            if (u === "" || root._asked[u] === true || root.coverMap[u])
                continue
            root._asked[u] = true
            out.push(u)
        }
        if (out.length > 0)
            QbzShell.sidebarArtworkWindow(JSON.stringify(out))
    }
    /// Ask for every portrait the current model wants, capped per call.
    /// Driven by the document landing and by scrolling — `requestCovers`
    /// self-dedupes, so an over-eager caller costs a loop, not a fetch.
    function requestVisibleCovers() {
        if (!root.isReady)
            return
        var urls = []
        var rows = root.rowModel
        for (var i = 0; i < rows.length; i++) {
            if (rows[i].type === "header")
                continue
            var arts = rows[i].artists || []
            for (var j = 0; j < arts.length; j++)
                urls.push(String(arts[j].artUrl || ""))
        }
        root.requestCovers(urls)
    }

    readonly property string sceneState: doc.state || "loading"
    readonly property bool isLoading: root.sceneState === "loading"
    readonly property bool isReady: root.sceneState === "ready"
    // Refuse entry offline (contract B7): the document's own class first, the
    // session flag as the belt-and-braces every other view already wears
    // (ArtistReleasesView.qml:100-111).
    readonly property bool isOffline: root.sceneState === "offline" || QbzSession.offline

    // The rows the bridge published. Candidates with no `qobuzId` are dropped
    // in RUST (contract §13.1) — Tauri filters them in candidatesToFavoriteArtists
    // (V:170) and the Slint port's failure to do so is defect S3. Nothing here
    // re-filters them; a row with an empty id arriving would be a bridge bug.
    readonly property var allArtists: doc.artists || []
    readonly property var progress: doc.progress || ({})

    // Cancel any in-flight discovery when the router unmounts us (contract B8:
    // "invalidated on view deactivation"). One page can run up to 100 sequential
    // Qobuz searches; a generation guard would drop the publish but not the
    // traffic.
    Component.onDestruction: QbzScene.close()

    // ======================= view state (all client-side) ==================
    // Every control in the nav bar filters the ALREADY-FETCHED list, exactly as
    // Tauri does (V:250-268) — none of them talks to the bridge.
    property string mode: "grid"              // V:88  'grid' | 'sidepanel'
    property string searchQuery: ""           // V:90
    property bool groupingEnabled: false      // V:92
    property bool groupMenuOpen: false        // V:93
    property var activeGenres: []             // V:95  Tauri's Set<string>
    property bool genrePopupOpen: false       // V:96
    property string selectedArtistId: ""      // V:102 (sidepanel only)

    // --- Filter chain, in Tauri's order (V:250-268) ------------------------
    //  1. genre filter — AND semantics, CASE-SENSITIVE exact equality on the
    //     strings that came back on the row (V:255-259).
    //  2. text search — DV-11, trimmed on both sides of the comparison.
    // No fuzzy matching, no diacritic folding, no debounce (V:250 is a plain
    // derived value recomputed on every keystroke).
    readonly property var filteredArtists: {
        var out = root.allArtists
        if (root.activeGenres.length > 0) {
            var want = root.activeGenres
            out = out.filter(function (a) {
                var g = a.genres || []
                for (var i = 0; i < want.length; ++i)
                    if (g.indexOf(want[i]) === -1)
                        return false
                return true
            })
        }
        var q = (root.searchQuery || "").trim().toLowerCase()
        if (q !== "")
            out = out.filter(function (a) {
                return root.artistNameOf(a).toLowerCase().indexOf(q) !== -1
            })
        return out
    }
    readonly property bool filteringActive:
        (root.searchQuery || "").trim() !== "" || root.activeGenres.length > 0
    // DV-18 — Tauri has no state for this and shows a blank viewport.
    readonly property bool filteredToZero:
        root.isReady && root.allArtists.length > 0 && root.filteredArtists.length === 0

    // --- The genre inventory the popup filters on (V:207-217) --------------
    // All unique genres across the UNFILTERED rows, sorted by frequency
    // descending with no secondary tiebreak (Tauri lets Map insertion order
    // decide ties; a stable sort over an insertion-ordered array does the same).
    readonly property var availableGenres: {
        var counts = ({})
        var order = []
        for (var i = 0; i < root.allArtists.length; ++i) {
            var g = root.allArtists[i].genres || []
            for (var j = 0; j < g.length; ++j) {
                if (counts[g[j]] === undefined) {
                    counts[g[j]] = 0
                    order.push(g[j])
                }
                counts[g[j]] += 1
            }
        }
        var rows = order.map(function (n) { return { "name": n, "count": counts[n] } })
        rows.sort(function (a, b) { return b.count - a.count })
        // NAMES ONLY. SceneGenreFilter's frozen mount API (contract §13.3)
        // takes plain strings — it filters, labels, selection-tests and emits
        // with the bare value, and it displays no count. Handing it the
        // {name, count} objects the sort needs would break the filter
        // silently: nothing in this tree type-checks a QML property, so the
        // cards would label themselves "[object Object]" and no selection
        // would ever match. The counts exist only to establish Tauri's
        // frequency-descending order (01 §7 `availableGenres`).
        return rows.map(function (r) { return r.name })
    }

    // ======================= grouping (V:271-301) ==========================
    // The key is the initial upper-cased, kept only when it is A-Z — so
    // accented and non-Latin initials all collapse into '#' (V:274-276).
    // Deliberately NOT "improved": it is on contract §6's kept-quirk list.
    // The 27-letter alphabet itself ('#' first, V:272) belongs to
    // scene/AlphaIndexRail.qml; this view only tells it which keys have members.
    function alphaKeyOf(name) {
        var c = (name || "").charAt(0).toUpperCase()
        return (c >= "A" && c <= "Z") ? c : "#"
    }

    /// The artist's display name, whatever the producer called the field.
    ///
    /// THE SEAM THAT BIT: the row used to carry `name`, and the swap to the
    /// standard `cards/ArtistCard.qml` (owner ruling R8) moved it to the house
    /// `title`. Grouping, sorting and search all still read `name`, so
    /// `(a.name || "")` yielded "", `alphaKeyOf("")` answered "#" for EVERY
    /// artist, and the A-Z sort compared empty strings — the rail rendered
    /// perfectly over one giant unsorted "#" bucket. Nothing errored, because
    /// every step is a legal operation on an empty string.
    ///
    /// Reading BOTH names is the same one-token insurance the sibling panel
    /// keeps for its album fields (scene/ScenePanelMode.qml:138-147), and it is
    /// what makes this class of rename survivable instead of silent.
    function artistNameOf(a) { return String((a && (a.title || a.name)) || "") }

    // V:278-296 — sort a COPY by localeCompare, bucket, then order the keys
    // with '#' forced first and the rest by localeCompare.
    function groupArtists(list) {
        var sorted = list.slice().sort(function (a, b) {
            return root.artistNameOf(a).localeCompare(root.artistNameOf(b))
        })
        var buckets = ({})
        var keys = []
        for (var i = 0; i < sorted.length; ++i) {
            var k = root.alphaKeyOf(root.artistNameOf(sorted[i]))
            if (buckets[k] === undefined) {
                buckets[k] = []
                keys.push(k)
            }
            buckets[k].push(sorted[i])
        }
        keys.sort(function (a, b) {
            if (a === "#") return -1
            if (b === "#") return 1
            return a.localeCompare(b)
        })
        return keys.map(function (k) { return { "key": k, "artists": buckets[k] } })
    }

    // V:298-301 — grouping OFF is a single anonymous group that keeps the
    // BACKEND's score-descending order. That is the default view, and the one
    // an "obvious" alphabetical sort would silently destroy.
    readonly property var groups: root.groupingEnabled
        ? root.groupArtists(root.filteredArtists)
        : [{ "key": "", "artists": root.filteredArtists }]

    readonly property var alphaActiveKeys: {
        var out = []
        for (var i = 0; i < root.groups.length; ++i)
            if (root.groups[i].key !== "")
                out.push(root.groups[i].key)
        return out
    }

    // ======================= the virtual row model =========================
    // Tauri virtualises a FLAT list of header|row items (Grid:68-100) with
    // CARD_WIDTH 160, CARD_HEIGHT 250, GAP 24, ROW_GAP 11, HEADER_HEIGHT 44
    // (Grid:48-53). A ListView over the same flat model gives Qt the identical
    // windowing for free — a GridView could not carry the group headers.
    // OWNER RULING R8 (2026-08-14): the grid draws the STANDARD
    // `cards/ArtistCard.qml` — hover overlay, follow chip, pin and context
    // menu — not the bare tile Tauri has. So the card DICTATES these two, and
    // they are its `implicitWidth`/`implicitHeight` (ArtistCard.qml:84-85),
    // not Tauri's 160x250 (Grid:48-49). The gap and row-gap stay Tauri's,
    // which is what keeps the grid's rhythm recognisably the reference's.
    readonly property int cardW: 200
    readonly property int cardH: 246
    readonly property int cardGap: 24
    readonly property int rowGap: 11
    readonly property int headerH: 44
    // Grid:62-65 — from a ResizeObserver in Tauri, from the item width here.
    // No media queries anywhere in the reference.
    readonly property int columns: Math.max(1,
        Math.floor((gridPane.width + root.cardGap) / (root.cardW + root.cardGap)))

    // Ask for the portraits whenever the row model settles. `rowModel` is
    // rebuilt by a new document, a page appended, a filter, a search keystroke
    // and a resize — every one of which can bring artists into view that were
    // not there before, and `requestCovers` self-dedupes, so a redundant call
    // costs a loop over the model and no fetch.
    //
    // NOT a `Component.onCompleted`: the view mounts BEFORE the first document
    // lands (the route is recorded by `QbzScene.open` and discovery is async),
    // so at completion the model is empty and the one shot would be wasted.
    onRowModelChanged: root.requestVisibleCovers()

    readonly property var rowModel: {
        var out = []
        var showHeaders = root.groupingEnabled
        for (var g = 0; g < root.groups.length; ++g) {
            var grp = root.groups[g]
            if (showHeaders && grp.key !== "")
                out.push({ "type": "header", "key": grp.key, "count": grp.artists.length })
            for (var i = 0; i < grp.artists.length; i += root.columns)
                out.push({ "type": "row", "key": grp.key,
                           "artists": grp.artists.slice(i, i + root.columns) })
        }
        return out
    }

    /// Top of the content in the ListView's own content coordinates. Read off
    /// the header ITEM rather than `originY`, which means different things with
    /// and without a header and is the kind of detail a silent 40px offset
    /// hides in.
    function contentTop() {
        return list.headerItem ? list.headerItem.y : 0
    }

    /// Content-Y of row `idx`, accumulated exactly the way Tauri accumulates
    /// its virtual tops (Grid:68-100): headers 44, rows 250 + 11.
    function rowTop(idx) {
        var y = root.contentTop() + (list.headerItem ? list.headerItem.height : 0)
        for (var i = 0; i < idx; ++i)
            y += root.rowModel[i].type === "header" ? root.headerH : (root.cardH + root.rowGap)
        return y
    }

    function jumpToLetter(letter) {
        for (var i = 0; i < root.rowModel.length; ++i) {
            if (root.rowModel[i].type === "header" && root.rowModel[i].key === letter) {
                var top = root.contentTop()
                var maxY = Math.max(top, top + list.contentHeight - list.height)
                // Tauri smooth-scrolls (Grid:224-231, `behavior: 'smooth'`).
                // ONE-SHOT and gesture-bounded, like the tail fades this tree
                // already runs (SearchView.qml:905-911) — not a continuous
                // animator and not a Behavior on a data-fed property, so the
                // pulse law is untouched.
                jumpAnim.stop()
                jumpAnim.from = list.contentY
                jumpAnim.to = Math.max(top, Math.min(maxY, root.rowTop(i)))
                jumpAnim.start()
                return
            }
        }
    }

    // ======================= scene identity ================================
    // `sceneLabel` is already `country.unwrap_or(display_name)` on the Rust
    // side (core.rs:2655-2657), so the view has ONE title source and does not
    // re-derive Tauri's three-way `sceneLabel || country || displayName`.
    readonly property string sceneTitle: doc.sceneLabel || ""
    readonly property string genreSummary: doc.genreSummary || ""
    readonly property var seedGenres: doc.seedGenres || []
    // V:664-673 — the subtitle renders when EITHER source has something.
    readonly property string subtitleGenres:
        root.genreSummary !== "" ? root.genreSummary : root.seedGenres.slice(0, 3).join(" / ")
    readonly property bool hasSubtitle: root.subtitleGenres !== ""

    // DV-13 — the bundled asset, lowercased ISO code (qml/assets/flags/, 266
    // files, already swept into the qrc by build.rs:177). Both mounts gate the
    // ELEMENT on `Image.Ready`, not on "the code is non-empty": an ISO code
    // MusicBrainz can emit and circle-flags lacks must render NOTHING — never a
    // broken-image box — and an Item that is `visible: false` is skipped by
    // Row/Column layout, so the 20px hero gap and the 10px compact gap close
    // with it.
    readonly property string flagCode: (doc.countryCode || "").toLowerCase()
    readonly property string flagSource: root.flagCode === ""
        ? "" : "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/flags/" + root.flagCode + ".svg"

    function tr(s) { return QbzSession.tr(s, QbzSession.trRev) }

    // ============================ offline gate =============================
    QbzOfflinePlaceholder {
        visible: root.isOffline
        anchors.centerIn: parent
        showSettingsAction: true
        onSettingsClicked: QbzShell.navigateTo("settings")
    }

    // ===================== STATE 1 — loading (V:606-632) ===================
    // The loading screen OWNS the viewport: top bar, compact header and nav bar
    // are all suppressed while it is up (V:423, V:447).
    //
    // BESPOKE AND PULSE-DRIVEN (DV-7). The reference is a 96px breathing circle
    // + an icon carousel + a 280x4 bar + phase text + a percentage, which no
    // stock Qt component resembles; and the two that come closest both break
    // the pulse law (see the header). One `Connections` on QbzShell.pulseMs
    // drives all of it, and it writes NOTHING unless `isLoading`.
    Item {
        id: loadingLayer
        anchors.fill: parent
        visible: root.isLoading && !root.isOffline

        // Local accumulated milliseconds. `pulseMs` is a WRAPPING counter whose
        // absolute value means nothing (shell_bridge.rs:295-302) — consumers
        // accumulate their own tick off its notify edge, which is also what
        // lets this one reset to 0 without touching the shared clock.
        property int ms: 0
        property int lastPulse: 0

        // S:1551-1554 — `@keyframes pulse`, 2s ease-in-out, 0%/100% scale(1)
        // opacity .8, 50% scale(1.06) opacity 1. A raised cosine is that curve.
        readonly property real breathe: (1 - Math.cos(ms / 2000 * 2 * Math.PI)) / 2
        // V:116-123 — Music -> Globe -> Share2 every 1500ms.
        readonly property int iconIndex: Math.floor(ms / 1500) % 3
        // S:1546-1549 — `iconFadeIn` 400ms ease-out, opacity 0->1,
        // scale .8->1, replayed on EVERY icon change by Tauri's `{#key}` block.
        readonly property real fadeT: Math.min(1, (ms % 1500) / 400)

        Connections {
            target: QbzShell
            function onPulseMsChanged() {
                if (!root.isLoading)
                    return          // a handler that writes nothing costs no frame
                var d = QbzShell.pulseMs - loadingLayer.lastPulse
                if (d < 0 || d > 500)
                    d = 33          // wrap, or a stall: one nominal period
                loadingLayer.lastPulse = QbzShell.pulseMs
                loadingLayer.ms += d
            }
        }
        onVisibleChanged: {
            if (visible) {
                ms = 0
                lastPulse = QbzShell.pulseMs
            }
        }

        Column {
            anchors.centerIn: parent
            spacing: 24                                     // S:1511-1517

            // S:1527-1537 — 96px disc, bg-secondary, accent glyph 42px.
            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 96
                height: 96
                radius: 48
                color: theme.surfaceCard
                scale: 1 + 0.06 * loadingLayer.breathe
                opacity: 0.8 + 0.2 * loadingLayer.breathe

                QbzIcon {
                    anchors.centerIn: parent
                    // Tauri's LOADING_ICONS is ['music','globe','network']
                    // (V:113) rendered as Music / Globe / Share2; `network` is
                    // this tree's name for that third glyph.
                    name: loadingLayer.iconIndex === 0 ? "music"
                        : (loadingLayer.iconIndex === 1 ? "globe" : "network")
                    width: 42
                    height: 42
                    tintName: "accent"
                    opacity: loadingLayer.fadeT
                    scale: 0.8 + 0.2 * loadingLayer.fadeT
                }
            }

            // S:1556-1569 — 280x4 track, radius 2, accent fill. The width is
            // DATA-FED, so no Behavior: progress events arrive at human rate
            // and a Behavior would animate every step at display rate, which is
            // one full-window present per tween frame.
            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 280
                height: 4
                radius: 2
                color: theme.surfaceElevated
                Rectangle {
                    width: parent.width * Math.max(0, Math.min(100, root.progress.pct || 0)) / 100
                    height: parent.height
                    radius: 2
                    color: theme.accent
                }
            }

            // S:1571-1589 — phase label 14 / text-secondary, percent 12 muted.
            Column {
                anchors.horizontalCenter: parent.horizontalCenter
                spacing: 4
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    // V:147-158 — three phases; anything else falls back to
                    // the 'searching' label with the literal "music".
                    // `sceneStep2` exists in Tauri's catalogue and is never
                    // used by this view (contract §7): do not port it.
                    text: {
                        var ph = root.progress.phase || "searching"
                        if (ph === "validating")
                            return root.tr("Verifying availability on Qobuz...")
                        if (ph === "done")
                            return root.tr("Preparing results...")
                        var g = root.progress.detail || "music"
                        return root.tr("Searching MusicBrainz for {genre} artists...")
                            .replace("{genre}", g)
                    }
                    color: theme.textSecondary
                    font.pixelSize: 14
                    horizontalAlignment: Text.AlignHCenter
                }
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: Math.round(root.progress.pct || 0) + "%"
                    color: theme.textMuted
                    font.pixelSize: 12
                }
            }
        }
    }

    // ======================= everything that is not loading ================
    Item {
        id: page
        anchors.fill: parent
        visible: !root.isLoading && !root.isOffline

        // S:823-833 — `.scene-view { padding: 0 8px 0 18px }`.
        readonly property int padL: 18
        readonly property int padR: 8

        // DV-15's replacement for the dropped `.top-bar`.
        Item { id: topSpacer; y: 0; width: 1; height: root.kioskHost ? 0 : 22 }

        // ---- Compact sticky header (V:433-443, S:862-895) -----------------
        // It lives OUTSIDE the scroll container, so when it appears it pushes
        // the nav bar and the grid down by its own height — sticky in intent,
        // static in layout, exactly as the reference is.
        Item {
            id: compact
            x: page.padL
            y: topSpacer.height
            width: page.width - page.padL - page.padR
            // 8 padding-top + content + 8 padding-bottom + 1 border + 8 margin
            height: visible ? 45 : 0
            visible: root.heroScrolledPast && root.isReady

            Row {
                id: compactRow
                y: 8
                width: parent.width
                spacing: 10                                   // S:864
                Image {
                    id: compactFlag
                    // 28x28 (S:872-878). circle-flags ship round, so no mask.
                    width: 28
                    height: 28
                    sourceSize.width: 56
                    sourceSize.height: 56
                    fillMode: Image.PreserveAspectCrop
                    source: root.flagSource
                    visible: root.flagSource !== "" && status === Image.Ready
                    anchors.verticalCenter: parent.verticalCenter
                }
                Text {
                    text: root.sceneTitle
                    color: theme.textPrimary
                    font.pixelSize: 16
                    font.weight: theme.weightBold
                    elide: Text.ElideRight
                    anchors.verticalCenter: parent.verticalCenter
                    width: Math.max(0, Math.min(implicitWidth,
                        parent.width
                            - (compactFlag.visible ? compactFlag.width + 10 : 0)
                            - (compactSub.visible ? compactSub.width + 10 : 0)))
                }
                Text {
                    id: compactSub
                    text: root.genreSummary
                    visible: root.genreSummary !== ""
                    color: theme.textMuted
                    font.pixelSize: 12
                    elide: Text.ElideRight
                    anchors.verticalCenter: parent.verticalCenter
                    width: Math.min(implicitWidth, parent.width * 0.4)
                }
            }
            // S:866 — 1px bottom border, then an 8px margin.
            Rectangle {
                y: 37
                width: parent.width
                height: 1
                color: theme.borderSubtle
            }
        }

        // ---- Nav bar (V:447-601, S:943-950) -------------------------------
        // Gated exactly as Tauri gates it: not while loading, not on an error,
        // and not before there is anything to count.
        Item {
            id: navBar
            x: page.padL
            y: compact.y + compact.height
            width: page.width - page.padL - page.padR
            height: visible ? (root.kioskHost ? 64 : 48) : 0        // 36px controls + S:947 margin 12
            visible: root.isReady && root.allArtists.length > 0

            // --- left: the results count (V:449-453, S:964-968) ------------
            // DV-10: Tauri prints `{n}{ / {total}}? ` + t('search.artists')
            // .toLowerCase(). Lower-casing a translated noun is locale-naive,
            // so the whole phrase is one msgid here and the English output is
            // identical.
            Text {
                anchors.verticalCenter: countAnchor.verticalCenter
                text: root.filteringActive
                    ? root.tr("{} / {} artists")
                        .replace("{}", root.filteredArtists.length)
                        .replace("{}", root.allArtists.length)
                    : root.tr("{} artists").replace("{}", root.allArtists.length)
                color: theme.textMuted
                font.pixelSize: 12
            }
            Item { id: countAnchor; width: 1; height: root.kioskHost ? 44 : 36 }

            // --- right: search · genre · view toggle · group (V:456-599) ---
            Row {
                id: navRight
                x: parent.width - width
                spacing: 8                                    // S:960
                height: root.kioskHost ? 44 : 36

                // (a) Search — QbzLineEdit's expandable arm IS this control:
                // a magnifier slot that opens a right-anchored field growing
                // LEFT, live `edited`, and an X that clears AND closes, which
                // is V:472 verbatim. DELIBERATE 2px DEVIATION: the shared
                // control's collapsed slot is 34px (ExpandableSearch.slint's
                // own number) where Tauri's `.search-icon-btn` is 36 — worth it
                // to not grow a second search box in this tree.
                QbzLineEdit {
                    id: searchBox
                    kioskHost: root.kioskHost
                    anchors.verticalCenter: parent.verticalCenter
                    expandable: true
                    openWidth: 240                            // S:997
                    placeholder: root.tr("Search…")
                    onEdited: function (v) { root.searchQuery = v }
                }

                // (b) Genre filter trigger — only when there is more than one
                // genre to choose between (V:480).
                Rectangle {
                    id: genreBtn
                    visible: root.availableGenres.length > 1
                    width: root.kioskHost ? 44 : 36
                    height: root.kioskHost ? 44 : 36                                // S:1056-1062
                    radius: 8
                    color: genreArea.containsMouse ? theme.bgHover : theme.surfaceElevated
                    border.width: 1
                    border.color: root.activeGenres.length > 0
                        ? theme.accent : theme.borderSubtle   // S:1107-1110
                    Behavior on color { ColorAnimation { duration: 150 } }

                    QbzIcon {
                        anchors.centerIn: parent
                        // Tauri's lucide Funnel; `list-filter` is this tree's
                        // funnel glyph (BrowseGenreButton.qml uses the same).
                        name: "list-filter"
                        width: 16
                        height: 16
                        tintName: root.activeGenres.length > 0 ? "accent"
                            : (genreArea.containsMouse ? "textPrimary" : "secondary")
                    }
                    // S:1112-1126 — 16px accent disc at -4/-4, 10/700, the
                    // count of active filters, in the page background colour.
                    Rectangle {
                        visible: root.activeGenres.length > 0
                        x: parent.width - width + 4
                        y: -4
                        width: 16
                        height: 16
                        radius: 8
                        color: theme.accent
                        Text {
                            anchors.centerIn: parent
                            text: root.activeGenres.length
                            color: theme.surfaceMain
                            font.pixelSize: 10
                            font.weight: theme.weightBold
                        }
                    }
                    MouseArea {
                        id: genreArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            root.groupMenuOpen = false
                            root.genrePopupOpen = !root.genrePopupOpen
                        }
                        // With genres ON, say WHICH — the badge only ever
                        // showed a number. This view already owns a QbzTooltip
                        // instance (`tips`), so it calls the summary mode
                        // directly instead of going through the shell channel
                        // that toolbars without one have to use.
                        onContainsMouseChanged: {
                            if (!containsMouse) {
                                tips.hide("scene-genre")
                            } else if (root.activeGenres.length > 0) {
                                tips.showSummary(genreBtn, "scene-genre", [{
                                    group: root.tr("Genre"),
                                    values: root.activeGenres
                                }])
                            } else {
                                tips.showAbove(genreBtn, "scene-genre",
                                               root.tr("Filter by genre"))
                            }
                        }
                    }
                }

                // (c) View toggle (V:554-571) ---------------------------------
                Rectangle {
                    id: modeBtn
                    width: root.kioskHost ? 44 : 36
                    height: root.kioskHost ? 44 : 36
                    radius: 8
                    color: modeArea.containsMouse ? theme.bgHover : theme.surfaceElevated
                    border.width: 1
                    border.color: theme.borderSubtle
                    Behavior on color { ColorAnimation { duration: 150 } }

                    QbzIcon {
                        anchors.centerIn: parent
                        // lucide PanelLeftClose while in grid mode, LayoutGrid
                        // while in sidepanel mode (V:565-569).
                        name: root.mode === "grid" ? "panel-left" : "layout-grid"
                        width: 16
                        height: 16
                        tintName: modeArea.containsMouse ? "textPrimary" : "secondary"
                    }
                    MouseArea {
                        id: modeArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            if (root.mode === "grid") {
                                root.mode = "sidepanel"
                            } else {
                                // V:559-562 — leaving the sidepanel clears the
                                // selection.
                                root.mode = "grid"
                                root.selectedArtistId = ""
                            }
                            root.groupMenuOpen = false
                            root.genrePopupOpen = false
                        }
                        // DV-12 — hardcoded English in Tauri (V:564).
                        onContainsMouseChanged: containsMouse
                            ? tips.showAbove(modeBtn, "scene-mode",
                                root.mode === "grid" ? root.tr("Browse view") : root.tr("Grid view"))
                            : tips.hide("scene-mode")
                    }
                }

                // (d) Group dropdown — grid mode only (V:574-599) -------------
                // Written out rather than reusing QbzSelect: this control is a
                // labelled button ("Group: A-Z") with a 2-row menu on Tauri's
                // own metrics, where QbzSelect carries its own chrome, its own
                // 30/34px sizing and a value-shaped label.
                Rectangle {
                    id: groupBtn
                    visible: root.mode === "grid"
                    width: groupRow.width + 24                // S:1043 padding 8px 12px
                    height: root.kioskHost ? 44 : 36
                    radius: 8
                    color: groupArea.containsMouse ? theme.bgHover : theme.surfaceElevated
                    border.width: 1
                    border.color: theme.borderSubtle
                    Behavior on color { ColorAnimation { duration: 150 } }

                    Row {
                        id: groupRow
                        anchors.centerIn: parent
                        spacing: 8                            // S:1039
                        Text {
                            // DV-12 — both labels are hardcoded English at
                            // V:577.
                            text: root.groupingEnabled
                                ? root.tr("Group: A-Z") : root.tr("Group: Off")
                            color: groupArea.containsMouse ? theme.textPrimary : theme.textSecondary
                            font.pixelSize: 12
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        QbzIcon {
                            name: "chevron-down"
                            width: 14
                            height: 14
                            anchors.verticalCenter: parent.verticalCenter
                            tintName: groupArea.containsMouse ? "textPrimary" : "secondary"
                        }
                    }
                    MouseArea {
                        id: groupArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            root.genrePopupOpen = false
                            root.groupMenuOpen = !root.groupMenuOpen
                        }
                    }
                }
            }
        }

        // ---- Body (V:605-819) ---------------------------------------------
        Item {
            id: body
            x: page.padL
            y: navBar.y + navBar.height
            width: page.width - page.padL - page.padR
            height: Math.max(0, page.height - y)

            // === STATE 2 — error (V:633-639, S:1592-1615) ==================
            // Only a FIRST-page failure reaches here; a failed pagination is
            // DV-17 and stays in the footer.
            Column {
                visible: root.sceneState === "error"
                anchors.horizontalCenter: parent.horizontalCenter
                y: 80                                          // S:1596 padding
                spacing: 16
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    // The raw error string, untranslated — Tauri renders
                    // `String(err)` verbatim (V:635).
                    text: root.doc.error || ""
                    color: theme.textMuted
                    font.pixelSize: 14
                    width: Math.min(implicitWidth, body.width - 64)
                    wrapMode: Text.WordWrap
                    horizontalAlignment: Text.AlignHCenter
                }
                // S:1602-1615 — padding 8px 20px, radius 8, bg-secondary,
                // text-primary 13. NO BORDER (see the header, §4.3).
                Rectangle {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: retryLabel.implicitWidth + 40
                    height: root.kioskHost ? 64 : retryLabel.implicitHeight + 16
                    radius: 8
                    color: retryArea.containsMouse ? theme.surfaceElevated : theme.surfaceCard
                    Behavior on color { ColorAnimation { duration: 150 } }
                    Text {
                        id: retryLabel
                        anchors.centerIn: parent
                        text: root.tr("Retry")
                        color: theme.textPrimary
                        font.pixelSize: 13
                    }
                    MouseArea {
                        id: retryArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        // DV-5 lives in Rust: `retry()` re-arms the progress
                        // stream AND the cancellation token before restarting
                        // at offset 0 (contract §13.1). Tauri's retry re-runs
                        // discovery without re-attaching its listener, so its
                        // bar stays frozen at whatever value it held.
                        onClicked: QbzScene.retry()
                    }
                }
            }

            // === STATE 3 — empty (V:640-644, S:1617-1626) ==================
            // Reached when the query SUCCEEDED and returned nothing — i.e.
            // MusicBrainz found artists but none of them exist on Qobuz.
            Column {
                visible: root.sceneState === "empty"
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.verticalCenter: parent.verticalCenter
                spacing: 16
                QbzIcon {
                    anchors.horizontalCenter: parent.horizontalCenter
                    name: "music"
                    width: 48
                    height: 48
                    tintName: "muted"
                }
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: root.tr("No matching artists from this scene are currently available on Qobuz.")
                    color: theme.textMuted
                    font.pixelSize: 14
                    width: Math.min(implicitWidth, body.width - 64)
                    wrapMode: Text.WordWrap
                    horizontalAlignment: Text.AlignHCenter
                }
            }

            // === STATE 4 — grid mode (V:645-712) ===========================
            Item {
                id: gridMode
                anchors.fill: parent
                visible: root.isReady && root.mode === "grid"

                // S:1359-1370 — `.grid-with-alpha` is a flex ROW: the grid
                // takes the remaining width and the rail sits beside it. The
                // rail is NOT sticky, whatever its CSS says: its scroller is a
                // descendant of its SIBLING, so `position:sticky` never
                // engages and it is simply top-aligned (08-critique-findings).
                Item {
                    id: gridPane
                    width: parent.width - (rail.visible ? rail.width + 4 : 0)
                    height: parent.height

                    ListView {
                        id: list
                        anchors.fill: parent
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds
                        model: root.rowModel
                        spacing: 0
                        cacheBuffer: 2 * (root.cardH + root.rowGap)   // Grid:150 ±5 items

                        NumberAnimation {
                            id: jumpAnim
                            target: list
                            property: "contentY"
                            duration: 250
                            easing.type: Easing.OutCubic
                        }

                        // --- the hero, as the scroll header (V:655-676) -----
                        header: Item {
                            width: list.width
                            height: heroRow.height + 20            // S:900 padding-bottom
                            Row {
                                id: heroRow
                                width: parent.width
                                spacing: 20                        // S:899
                                // S:905-918 — 140px round flag with a
                                // `0 8px 32px rgba(0,0,0,0.4)` shadow. Written
                                // Qt.rgba(): #00000066 would be TRANSPARENT
                                // black under Qt's #AARRGGBB parse.
                                Item {
                                    id: flagWrap
                                    width: 140
                                    height: 140
                                    visible: heroFlag.status === Image.Ready
                                    anchors.verticalCenter: parent.verticalCenter

                                    Rectangle {
                                        // The Cortinilla.qml:188-201 shadow
                                        // recipe: a blurred offset disc, gated
                                        // off on the software renderer where
                                        // there are no shaders.
                                        visible: !root.noShaders
                                        x: 0
                                        y: 8
                                        width: 140
                                        height: 140
                                        radius: 70
                                        color: Qt.rgba(0, 0, 0, 0.4)
                                        layer.enabled: true
                                        layer.effect: MultiEffect {
                                            blurEnabled: true
                                            blurMax: 32
                                            blur: 1.0
                                        }
                                    }
                                    Image {
                                        id: heroFlag
                                        anchors.fill: parent
                                        source: root.flagSource
                                        sourceSize.width: 280
                                        sourceSize.height: 280
                                        fillMode: Image.PreserveAspectCrop
                                    }
                                }
                                Column {
                                    spacing: 8                     // S:920-925
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: parent.width
                                        - (flagWrap.visible ? flagWrap.width + 20 : 0)
                                    Text {
                                        width: parent.width
                                        text: root.sceneTitle
                                        color: theme.textPrimary
                                        font.pixelSize: 28         // S:927-933
                                        font.weight: theme.weightBold
                                        lineHeight: 1.2
                                        lineHeightMode: Text.ProportionalHeight
                                        elide: Text.ElideRight
                                    }
                                    Text {
                                        visible: root.hasSubtitle
                                        width: parent.width
                                        // V:666 — "Based on {artist} · {genres}",
                                        // where {artist} is the MUSICBRAINZ name
                                        // (ArtistDetailView.svelte:2915), not the
                                        // Qobuz one, and the separator is a
                                        // U+00B7 MIDDLE DOT.
                                        text: root.tr("Based on {artist} · {genres}")
                                            .replace("{artist}", root.doc.artistName || "")
                                            .replace("{genres}", root.subtitleGenres)
                                        color: theme.textMuted
                                        font.pixelSize: 14         // S:935-940
                                        lineHeight: 1.4
                                        lineHeightMode: Text.ProportionalHeight
                                        wrapMode: Text.WordWrap
                                    }
                                }
                            }
                        }

                        // --- rows and group headers ------------------------
                        // The model is FLAT (header | row of up to `columns`
                        // cards), which is what lets one ListView give the
                        // grouped grid the same windowing Tauri hand-rolls.
                        // Every reference below goes through `del`, never a
                        // `parent.parent` chain: parent-chain bindings are
                        // invisible to qmllint and are a documented trap in
                        // this port (Cortinilla.qml:441-448).
                        delegate: Item {
                            id: del
                            required property var modelData
                            readonly property bool isHeader: modelData.type === "header"
                            width: list.width
                            height: del.isHeader
                                ? root.headerH : (root.cardH + root.rowGap)

                            // Grid:333-350 — 12px uppercase muted, letter-
                            // spacing 0.08em (= 0.96px at 12px), padding
                            // 12px 0 4px, count on the right.
                            Item {
                                visible: del.isHeader
                                width: del.width
                                height: root.headerH
                                Text {
                                    y: 12
                                    text: (del.modelData.key || "").toUpperCase()
                                    color: theme.textMuted
                                    font.pixelSize: 12
                                    font.weight: theme.weightSemibold
                                    font.letterSpacing: 0.96
                                }
                                Text {
                                    y: 12
                                    x: parent.width - width
                                    text: del.modelData.count || ""
                                    color: theme.textMuted
                                    font.pixelSize: 12
                                }
                            }

                            // Grid:352-356 — `.artist-row { display:flex;
                            // gap:24 }`.
                            Row {
                                visible: !del.isHeader
                                spacing: root.cardGap
                                Repeater {
                                    model: del.isHeader ? [] : (del.modelData.artists || [])
                                    delegate: ArtistCard {
                                        kioskHost: root.kioskHost
                                        // The row is published in the house
                                        // card shape by artist_scene_qt.rs
                                        // (id/title/subtitle/artUrl +
                                        // following/isPinned), so there is no
                                        // translation layer here — same mount
                                        // as LabelView.qml:247 and
                                        // HomeView.qml:1176.
                                        item: modelData
                                        artSource: root.coverMap[String(modelData.artUrl || "")] || ""
                                        artworkUrl: modelData.artUrl || ""
                                        isPinned: modelData.isPinned === true
                                    }
                                }
                            }
                        }

                        // --- footer: Load More / the DV-17 inline failure ---
                        footer: Item {
                            width: list.width
                            // Grid:432-434 `.virtual-footer` padding 16px 0 8px
                            // plus S:1629-1633 `.load-more-inline` 16px 0 24px,
                            // then the tail room every Qt view leaves for the
                            // now-playing bar (ArtistReleasesView.qml:230).
                            height: footerCol.height + 100
                            Column {
                                id: footerCol
                                width: parent.width
                                topPadding: 16
                                spacing: 12

                                // DV-17 — a failed pagination keeps the grid and
                                // says so here. The seam: `state` stays "ready"
                                // and `error` is non-empty.
                                Text {
                                    visible: root.isReady && (root.doc.error || "") !== ""
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    text: root.doc.error || ""
                                    color: theme.textMuted
                                    font.pixelSize: 12
                                    width: Math.min(implicitWidth, parent.width - 64)
                                    elide: Text.ElideRight
                                    horizontalAlignment: Text.AlignHCenter
                                }

                                // Idle: S:1635-1651 — padding 10px 24px,
                                // radius 8, bg-secondary, 13px. NO BORDER
                                // (§4.3). The msgid is the catalogue's existing
                                // "Load more"; Tauri's "Load More" would cost
                                // eight translations for one capital letter
                                // (contract §7).
                                Rectangle {
                                    visible: root.doc.hasMore === true
                                        && root.doc.loadingMore !== true
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    width: moreLabel.implicitWidth + 48
                                    height: root.kioskHost ? 64 : moreLabel.implicitHeight + 20
                                    radius: 8
                                    color: moreArea.containsMouse
                                        ? theme.surfaceElevated : theme.surfaceCard
                                    Behavior on color { ColorAnimation { duration: 150 } }
                                    Text {
                                        id: moreLabel
                                        anchors.centerIn: parent
                                        text: root.tr("Load more")
                                        color: theme.textPrimary
                                        font.pixelSize: 13
                                    }
                                    MouseArea {
                                        id: moreArea
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: QbzScene.loadMore()
                                    }
                                }

                                // Busy: S:1653-1679 — a 200x4 bar and a
                                // tabular percentage, replacing the button.
                                // Data-fed width, so again no Behavior.
                                Row {
                                    visible: root.doc.loadingMore === true
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    spacing: 12
                                    Rectangle {
                                        width: 200
                                        height: 4
                                        radius: 2
                                        color: theme.surfaceElevated
                                        anchors.verticalCenter: parent.verticalCenter
                                        Rectangle {
                                            width: parent.width
                                                * Math.max(0, Math.min(100, root.progress.pct || 0)) / 100
                                            height: parent.height
                                            radius: 2
                                            color: theme.accent
                                        }
                                    }
                                    Text {
                                        text: Math.round(root.progress.pct || 0) + "%"
                                        color: theme.textMuted
                                        font.pixelSize: 12
                                        anchors.verticalCenter: parent.verticalCenter
                                    }
                                }
                            }
                        }
                    }

                    // Back/forward scroll memory (controls/ScrollMemory.qml): reports
                    // this container's offset while it is the live page, and restores it
                    // when a back/forward step arms this route.
                    ScrollMemory { target: list; scope: "scene" }
                    QbzScrollBar {
                        anchors.right: parent.right
                        anchors.rightMargin: 4
                        anchors.top: parent.top
                        anchors.bottom: parent.bottom
                        target: list
                    }

                    // DV-18 — Tauri leaves the hero over a blank viewport here.
                    // The list is unscrollable in this state, so a static line
                    // under the hero is enough.
                    Text {
                        visible: root.filteredToZero
                        anchors.horizontalCenter: parent.horizontalCenter
                        y: (list.headerItem ? list.headerItem.height : 0) + 40
                        text: root.tr("No results found.")
                        color: theme.textMuted
                        font.pixelSize: 14
                    }
                }

                // The 27-letter jump rail — a SIBLING lane's component
                // (scene/AlphaIndexRail.qml). Rendered only while grouping is
                // on (V:699), top-aligned, 4px off the grid (S:1381).
                AlphaIndexRail {
                    id: rail
                    visible: root.groupingEnabled
                    x: gridPane.width + 4
                    y: 0
                    activeKeys: root.alphaActiveKeys
                    onJumped: function (letter) { root.jumpToLetter(letter) }
                }
            }

            // === STATE 5 — sidepanel mode (V:713-817) ======================
            // `.artist-two-column-layout` carries `margin: 0 -8px 0 -18px`
            // (S:1419), which exactly cancels `.scene-view`'s padding so the
            // two columns go edge to edge. Here that is the negative x plus the
            // full root width.
            ScenePanelMode {
                id: panel
                visible: root.isReady && root.mode === "sidepanel"
                x: -page.padL
                y: 0
                width: root.width
                height: parent.height
                doc: root.doc.panel || ({})
                // ALWAYS grouped A-Z in this mode, regardless of the nav bar's
                // grouping toggle (V:718) — the rail's own contract.
                artists: root.filteredArtists
                selectedId: root.selectedArtistId
                onArtistSelected: function (qobuzId) {
                    root.selectedArtistId = qobuzId
                    QbzScene.selectArtist(qobuzId)
                }
                // V:762-765 — the sidepanel's album cards forward to the host's
                // `onAlbumClick` / `onAlbumPlay`, which are the shell's normal
                // album open and album play (+page.svelte:6989-6991).
                onAlbumActivated: function (albumId) { QbzAlbum.openAlbum(albumId) }
                onAlbumPlayed: function (albumId) { QbzPlayer.playAlbum(albumId) }
            }
        }

        // ---- Popover layer -------------------------------------------------
        // DV-4: ONE scrim closes both menus on an outside click. Tauri gives
        // the genre popup a document-level handler (V:240-246) and the group
        // menu none at all, which is a plain defect and not a design.
        MouseArea {
            id: outsideScrim
            anchors.fill: parent
            visible: root.genrePopupOpen || root.groupMenuOpen
            z: 90
            onPressed: {
                root.genrePopupOpen = false
                root.groupMenuOpen = false
            }
        }

        // Group menu (V:578-598, S:1069-1104) — absolute, right-aligned under
        // its trigger, 4px gap, min-width 160, z-index 100. ADR-009's "modals
        // >= 3000" does not apply: this is a popover inside a view.
        Rectangle {
            id: groupMenu
            visible: root.groupMenuOpen && groupBtn.visible
            z: 100
            width: Math.max(160, offItem.implicitTextWidth + 24)
            height: 4 + offItem.height + alphaItem.height + 4
            radius: 8
            color: theme.surfaceCard
            border.width: 1
            border.color: theme.borderSubtle
            // Position: the trigger's right edge, 4px below it.
            x: navBar.x + navRight.x + groupBtn.x + groupBtn.width - width
            y: navBar.y + groupBtn.y + groupBtn.height + 4

            // S:1079 — `box-shadow: 0 4px 16px rgba(0,0,0,0.3)`.
            Rectangle {
                visible: !root.noShaders
                z: -1
                x: 0
                y: 4
                width: parent.width
                height: parent.height
                radius: parent.radius
                color: Qt.rgba(0, 0, 0, 0.3)
                layer.enabled: true
                layer.effect: MultiEffect {
                    blurEnabled: true
                    blurMax: 16
                    blur: 1.0
                }
            }

            Column {
                y: 4
                width: parent.width
                DropdownItem {
                    id: offItem
                    // DV-12 — "Off" is hardcoded English at V:587, and the
                    // msgid already exists in all eight catalogues.
                    label: root.tr("Off")
                    selected: !root.groupingEnabled
                    onPicked: {
                        root.groupingEnabled = false
                        root.groupMenuOpen = false
                    }
                }
                DropdownItem {
                    id: alphaItem
                    label: root.tr("Alphabetical (A-Z)")      // DV-12, V:594
                    selected: root.groupingEnabled
                    onPicked: {
                        root.groupingEnabled = true
                        root.groupMenuOpen = false
                    }
                }
            }
        }

        // Genre popup — a SIBLING lane's component (scene/SceneGenreFilter.qml).
        // S:1133-1147 anchors it to the 36x36 funnel button:
        // `top: calc(100% + 6px); right: 0` against `.genre-filter-container`
        // (S:1129-1131), NOT against the nav bar.
        SceneGenreFilter {
            id: genrePopup
            z: 200
            open: root.genrePopupOpen && genreBtn.visible
            visible: open
            x: Math.max(0, navBar.x + navRight.x + genreBtn.x + genreBtn.width - width)
            y: navBar.y + genreBtn.y + genreBtn.height + 6
            genres: root.availableGenres
            selected: root.activeGenres
            onToggled: function (genre) {
                var next = root.activeGenres.slice()
                var i = next.indexOf(genre)
                if (i === -1)
                    next.push(genre)
                else
                    next.splice(i, 1)
                root.activeGenres = next
            }
            onCleared: {
                root.activeGenres = []
                root.genrePopupOpen = false            // V:542 closes with it
            }
            onCloseRequested: root.genrePopupOpen = false
        }
    }

    // Hover tooltips for the three icon-only controls — Tauri's `title=`
    // attributes (V:458, :487, :564), which the browser renders for free and
    // QML does not. QbzTooltip is the shared overlay; it takes no pointer and
    // owns no animator, so an idle one costs nothing.
    QbzTooltip {
        id: tips
        anchors.fill: parent
        z: 900
    }

    // MultiEffect needs shaders. Where there are none (offscreen smoke runs,
    // and any platform on the software path) the two shadows are dropped
    // rather than faked — the same detection RoundedImage.qml and
    // Cortinilla.qml:186-187 do.
    readonly property bool noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null

    // Scroll sentinel — Tauri watches a 1px element pinned to the bottom of the
    // hero with an IntersectionObserver (Grid:200-216) and hands the result up
    // as `heroScrolledPast` (V:653). The header item's own bottom edge in
    // content coordinates is the same measurement, and unlike `originY` it
    // means one thing whether or not a header exists.
    // DV-8: the `mode === "grid"` term is what resets it on a mode switch — in
    // Tauri the sentinel simply stops existing and the flag sticks at whatever
    // it held, so the compact header can be stuck on or off.
    readonly property bool heroScrolledPast: root.mode === "grid"
        && list.headerItem !== null
        && list.contentY >= list.headerItem.y + list.headerItem.height

    // ---- The two-row group menu's item (S:1082-1104) ----------------------
    component DropdownItem: Item {
        id: ddi
        property string label: ""
        property bool selected: false
        readonly property real implicitTextWidth: ddText.implicitWidth
        signal picked()

        width: parent ? parent.width : 0
        height: root.kioskHost ? 44 : 32                                     // padding 8px + 12px text

        Rectangle {
            anchors.fill: parent
            anchors.leftMargin: 4
            anchors.rightMargin: 4
            radius: 6
            color: ddArea.containsMouse ? theme.surfaceElevated : "transparent"
            Behavior on color { ColorAnimation { duration: 100 } }
        }
        Text {
            id: ddText
            x: 12
            anchors.verticalCenter: parent.verticalCenter
            text: ddi.label
            color: ddi.selected ? theme.accent
                : (ddArea.containsMouse ? theme.textPrimary : theme.textSecondary)
            font.pixelSize: 12
            font.weight: ddi.selected ? theme.weightSemibold : theme.weightRegular
        }
        MouseArea {
            id: ddArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: ddi.picked()
        }
    }

    // ---- The scene's grid card (Grid:267-286, S:358-418) ------------------
    // 160x250 slot, NOT the shared cards/ArtistCard.qml (200x246). The contract
    // (§4.4) settles this: reuse the primitives, write the scene's own
    // delegate. Tauri's scene card is a tile with a 120px round portrait and a
    // NAME — no follow, no play, no pin, no context menu, and NO SUBTITLE.
    //
    // The absent subtitle is deliberate and was verified against the reference
    // rather than inherited: `albums_count` is set on the object (V:179) and
    // rendered NOWHERE (Grid:270-285 is image + name), and the sibling list row
    // carries a comment saying the count was removed on purpose — "albums_count
    // from Qobuz API includes compilations, tributes, etc. — misleadingly high.
    // Removed per #169" (List:269-270). Contract §13.1's note that albumsCount
    // is the card subtitle describes Tauri's DATA, not its UI; putting the count
    // on screen would re-introduce a line Tauri's authors deleted.
}
