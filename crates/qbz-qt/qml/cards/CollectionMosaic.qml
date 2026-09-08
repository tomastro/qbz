// CollectionMosaic — the artwork renderer for Mixtape / Collection cards, list
// rows, empty states and the detail hero (myqbz/CollectionMosaic.slint:1-121).
// Four sizes in use: 184 (grid card), 48 (list row), 160 (empty state), 186
// (detail hero).
//
// SIBLING FILE: cards/PlaylistCollage.qml. Read that one first — the tile
// discipline, the URL-keyed artwork resolver, the 120ms dispatch debounce and
// the single outer mask are copied from it deliberately. Two differences:
//   * this grid is UNIFORM (2x2 or 3x3) instead of the playlist collage's
//     single / dual / 62-38 / 34-66 ratios, and
//   * `urls` is INDEX-SIGNIFICANT (Slint's url1..url9), so PlaylistCollage's
//     `dedup()` must NOT be copied: cell N is `urls[N]` and Rust already
//     guarantees distinct covers.
// It is a separate file rather than a `layout` parameter on PlaylistCollage
// because the tiles are static children: nine of them would mount on every
// playlist card on every Home rail, every Library grid cell and every sidebar
// row, app-wide.
//
// ── HOW THE TILES ARE DRAWN (this file used to be a Canvas) ────────────────
// Each cell is a `RoundedImage { radius: 0; fit: "crop" }`, i.e. a plain
// filtered `Image` that requests its own scaled derivative at its own device
// size through the existing `QbzSession.artScaled` seam. The first version of
// this file was a per-card `Canvas` whose `Context2D.drawImage` does NOT
// filter when it scales — a 150px cover drawn into a 23px list-row cell was
// EXACTLY nearest-neighbour (measured RMSE 0.000 against `-filter Point`,
// theme/RoundedImage.qml). That is the same defect perf FIX 1 / FIX 4 removed
// from RoundedImage and PlaylistCollage; writing it a third time here was the
// bug. Reuse over a third copy: RoundedImage already owns the intrinsic-size
// latch, the derivative request, the crop fit, the readiness latch, the
// per-item crop scissor (`clip` at radius 0, spec 04 §1.3) and the
// recycled-delegate invalidation.
//
// Only the mosaic's OUTER shape is rounded; the cells are square-cornered.
// That rounding is one `layer.enabled` + one `MultiEffect` mask on the whole
// mosaic (NOT one per cell), the mechanism theme/RoundedImage.qml documents.
// Effects need shaders: this port runs on the GPU (OpenGL RHI, measured
// 2026-07-29); the earlier "shader masks render nothing" note in this header
// came from an offscreen session, which forces the software renderer by
// definition. Where a software path is genuinely possible it is DETECTED
// (`GraphicsInfo.api`), and `QbzShell.forceCanvasArt` pins the same degrade by
// hand — in either case the mask is skipped and the mosaic reads
// square-cornered over the already-rounded placeholder fill, rather than
// blank.
//
// THE RULES, IN PRECEDENCE ORDER (CollectionMosaic.slint:61-120):
//   1. `hasCustomCover` -> ONE full-bleed crop-fitted cover. The gate is
//      `hasCustomCover` ALONE (:62), NOT "and the path resolved": an
//      unresolved custom cover paints the bare surface-elevated outer, exactly
//      as Slint does. (Rust's card predicate and hero predicate differ on
//      purpose — myqbz.rs:322-331 vs :367 — so a deleted cover file can give
//      glyph-in-hero / mosaic-in-grid. Do not unify them here.)
//   2. else `coverCount == 0` -> the centred kind glyph at round(size * 0.4),
//      muted, opacity 0.6 (:70-82).
//   3. else the collage. `cols = (effKind == "collection" && itemCount >= 9)
//      ? 3 : 2` (:58), gap 2 (:59). Empty cells show the outer
//      surface-elevated through the seam, which is what Slint's empty
//      MosaicCell paints anyway (:32).
//
// EFFECTIVE KIND: `emptyKind !== "" ? emptyKind : kind` (:56). The override is
// NOT empty-state-only — it feeds BOTH the glyph choice and the 3x3 rule, so
// it is computed once and used in both places.
//
// Rust pre-decides `coverCount` (0 / 4 / 9) and pre-downscales each cell URL
// (_50 for the 3x3, _150 for the 2x2 — myqbz_qt.rs:403-417); this component
// only lays out. Never point a cell at a _600 source.

import QtQuick
import QtQuick.Effects
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    /// 0..9 remote cover URLs, INDEX-SIGNIFICANT (Slint's url1..url9).
    property var urls: []
    /// Optional pre-resolved `file://` paths from the document (GridDoc
    /// `cellPaths` / DetailDoc `heroCellPaths`), same indexing; "" = pending,
    /// in which case the URL-keyed resolver below fills it in.
    property var paths: []
    property int coverCount: 0
    /// item.kind — mixtape | collection | artist_collection.
    property string kind: ""
    /// Caller override of the EFFECTIVE kind (glyph AND the 3x3 rule).
    property string emptyKind: ""
    /// Drives the 3x3 rule.
    property int itemCount: 0
    property bool hasCustomCover: false
    /// Resolved local file:// for the custom cover.
    property string customCoverPath: ""
    property real size: 184
    property real gap: 2
    property real radius: 8

    QbzTheme { id: theme }

    width: root.size
    height: root.size

    readonly property string effKind: root.emptyKind !== "" ? root.emptyKind : root.kind
    readonly property int cols: (root.effKind === "collection" && root.itemCount >= 9) ? 3 : 2
    readonly property int cellCount: root.cols === 3 ? 9 : 4
    readonly property bool collage: !root.hasCustomCover && root.coverCount > 0

    /// See theme/RoundedImage.qml: the ONLY case the retired "effects render
    /// nothing" doctrine was ever true for is the software/Null renderer, and
    /// `GraphicsInfo.api` answers it per window at runtime (`api` carries
    /// `notify: "apiChanged"`, so this re-evaluates when the window arrives).
    /// Test Software/Null negatively — under the RHI an OpenGL backend reports
    /// `GraphicsInfo.OpenGL`.
    ///
    /// `QbzShell.forceCanvasArt` (env `QBZ_QT_ROUND_MODE=canvas`, default OFF)
    /// is honoured HERE TOO, for the reason PlaylistCollage.qml:86-96 states:
    /// the override exists for a machine where `GraphicsInfo` reports a GPU and
    /// the mask still misbehaves, and if the mask draws nothing there then THIS
    /// file's mask draws nothing either — the whole mosaic disappears. Pinning
    /// the cells onto RoundedImage's canvas arm while leaving the mosaic masked
    /// would defeat the override on exactly the surface it was added for.
    /// Square outer corners (the same degrade the software path takes) is the
    /// correct fallback. The `typeof` guard mirrors RoundedImage: eager
    /// binding, and the file must stay loadable in an isolated qml6 scene.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null
        || (typeof QbzShell !== "undefined" && QbzShell.forceCanvasArt)

    // url -> local cached path, filled by the artwork signal.
    property var pathMap: ({})

    function urlAt(index) {
        const u = root.urls
        return (u && index < u.length && u[index]) ? u[index] : ""
    }
    function pathAt(index) {
        const seeded = (root.paths && index < root.paths.length) ? root.paths[index] : ""
        if (seeded)
            return seeded
        const u = root.urlAt(index)
        return u ? (root.pathMap[u] || "") : ""
    }

    /// Uniform grid, transcribed from CollectionMosaic.slint: 2x2 cell
    /// `(size - gap) / 2` (:89-95), 3x3 cell `(size - 2 * gap) / 3` (:104-118),
    /// `gap` 2 (:59). Cell N sits at `col * (cell + gap)` / `row * (cell + gap)`
    /// — the same arithmetic the deleted `onPaint` ran. These numbers do not
    /// change.
    function cellOf(index) {
        const w = root.width
        const h = root.height
        const n = root.cols
        if (w <= 0 || h <= 0 || index >= n * n)
            return Qt.rect(0, 0, 0, 0)
        const g = root.gap
        const cell = (w - (n - 1) * g) / n
        return Qt.rect((index % n) * (cell + g),
                       Math.floor(index / n) * (cell + g),
                       cell, cell)
    }

    // --- Outer placeholder. A Rectangle's OWN rounded fill paints correctly;
    // only its children would need the mask.
    Rectangle {
        anchors.fill: parent
        radius: root.radius
        color: theme.surfaceElevated
    }

    // --- Rule 1: custom cover, full-bleed crop. Loader-gated so the mask is
    // not instantiated on the (overwhelmingly common) cards that have no
    // custom cover.
    Loader {
        anchors.fill: parent
        active: root.hasCustomCover
        sourceComponent: customCover
    }
    Component {
        id: customCover
        RoundedImage {
            source: root.customCoverPath
            radius: root.radius
            fit: "crop"
        }
    }

    // --- Rule 2: the centred kind glyph.
    QbzIcon {
        visible: !root.hasCustomCover && root.coverCount === 0
        name: root.effKind === "mixtape" ? "cassette-tape"
            : root.effKind === "artist_collection" ? "user"
            : "library-big"
        width: Math.round(root.size * 0.4)
        height: width
        anchors.centerIn: parent
        tintName: "muted"
        opacity: 0.6
    }

    // --- Artwork: URL-keyed through the SHARED resolver (the sidebar
    // micro-collage channel), debounced so flinging a windowed grid never
    // fetches covers for delegates that are already gone. Only cells whose
    // path is still unknown are asked for. No new bridge member.
    Timer {
        id: dispatchDelay
        interval: 120
        onTriggered: {
            var pending = []
            for (var i = 0; i < root.cellCount; i++) {
                var u = root.urlAt(i)
                if (u !== "" && root.pathAt(i) === "")
                    pending.push(u)
            }
            if (pending.length > 0)
                QbzShell.sidebarArtworkWindow(JSON.stringify(pending))
        }
    }
    function kick() {
        if (root.collage)
            dispatchDelay.restart()
    }
    onUrlsChanged: root.kick()
    Component.onCompleted: root.kick()

    Connections {
        target: QbzLibrary
        // Shared with the feed's artKey-keyed emissions — ignore anything that
        // is not one of OUR urls.
        function onLibraryArtworkReady(key, path) {
            if (root.pathMap[key] === path)
                return
            var mine = false
            for (var i = 0; i < root.cellCount; i++) {
                if (root.urlAt(i) === key) { mine = true; break }
            }
            if (!mine)
                return
            var m = root.pathMap
            m[key] = path
            // Rebind requires a NEW object reference.
            root.pathMap = Object.assign({}, m)
        }
    }

    /// The rounded OUTER shape, rendered to its own FBO once (it changes only
    /// with width/height/radius, never per frame). `visible: false` + layered
    /// is the documented MultiEffect mask idiom — the layer renders regardless
    /// of visibility, which is the whole point.
    Item {
        id: maskSrc
        anchors.fill: parent
        visible: false
        // Also gated on `collage` — a layer renders even when its item is
        // invisible (that is the mask idiom), so a glyph-only card would
        // otherwise pay two FBOs to mask a glyph that is not inside them.
        layer.enabled: !root._noShaders && root.radius > 0 && root.collage
        layer.smooth: true
        Rectangle {
            anchors.fill: parent
            radius: root.radius
            color: "#ffffff"
        }
    }

    // --- Rule 3: the collage. ONE layer + ONE mask for the whole mosaic, not
    // one per cell. `clip: true` bounds the outer rect; each cell's own crop
    // overflow is scissored by RoundedImage itself (`clip` there covers exactly
    // the radius-0 crop case this file creates — the deleted Canvas clipped per
    // cell with `ctx.rect(); ctx.clip()`).
    Item {
        id: mosaic
        anchors.fill: parent
        // No clip: cellOf() tiles the parent rect exactly, and each tile is a
        // RoundedImage that scissors itself when it needs to.
        visible: root.collage
        layer.enabled: !root._noShaders && root.radius > 0 && root.collage
        layer.smooth: true
        layer.effect: MultiEffect {
            maskEnabled: true
            maskSource: maskSrc
            // 0.5 / 1.0 — MEASURED, see theme/RoundedImage.qml:491-519 for the
            // full table. Qt's mask is a smoothstep CENTRED on maskThresholdMin
            // of width maskSpreadAtMin, so (0.5, 1.0) is the only pair that maps
            // mask alpha 0..1 onto 0..1 and therefore the only one that
            // reproduces `ctx.clip()`'s antialiased corner. The comment that
            // used to sit here had it backwards: (0.0, 0.0) does NOT "leave
            // everything alpha-blending", it collapses the smoothstep to
            // `step(0, alpha)` — every mask pixel with any alpha snaps fully
            // opaque, so the corner comes back HARD (measured at radius 40 on
            // this Qt 6.11.1: 0 antialiased pixels against the Canvas's 292,
            // and 308 at 0.5/1.0 = Canvas parity, ramp [0,51,219,255]).
            // (0.0, > 0) is worse: the low edge goes negative, everything
            // clamps opaque and the mask is disabled outright. Do not "restore
            // the defaults" here.
            maskThresholdMin: 0.5
            maskSpreadAtMin: 1.0
        }

        Repeater {
            // 4 for the 2x2, 9 for the 3x3, and ZERO when this card shows a
            // custom cover or the kind glyph — an idle tile is still an Item.
            model: root.collage ? root.cellCount : 0
            delegate: RoundedImage {
                required property int index
                readonly property rect cell: root.cellOf(index)
                x: cell.x
                y: cell.y
                width: cell.width
                height: cell.height
                source: root.pathAt(index)
                // Square-cornered: only the mosaic's outer shape is rounded.
                // radius 0 also means this RoundedImage mounts a plain Image
                // with NO layer and NO mask of its own, and takes the crop
                // scissor instead (spec 04 §1.3).
                radius: 0
                fit: "crop"
            }
        }
    }
}
