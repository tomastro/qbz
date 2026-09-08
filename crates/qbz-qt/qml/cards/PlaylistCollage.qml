// PlaylistCollage — the playlist cover mosaic (discover/PlaylistCollage.slint,
// itself the port of Tauri's PlaylistCollage.svelte).
//
// Qobuz hands back up to four MEMBER-ALBUM covers per playlist
// (images300 / images150 / images); the layout is picked by how many there
// are — single, dual (side by side), triple (one large left + two stacked
// right), quad (three stacked left + one large right), and a list-music
// glyph when the playlist has none. Tiles are always crop-fitted; the
// playlist's OWN Qobuz graphic is NOT drawn here (PlaylistCard and
// PlaylistView render that one through RoundedImage's `pad` fit).
//
// The crop is deliberate and stays: these tiles are member-ALBUM covers,
// which Qobuz ships SQUARE, and from two tiles up the cells are not square
// anyway — a mosaic cell is meant to be filled edge to edge, so letterboxing
// one would put gaps inside the grid. The image-derived padding added for
// the 800x380 playlist banner therefore has no case to answer here, and
// duplicating it into this file would only fork the one implementation.
//
// ── HOW THE TILES ARE DRAWN (this file used to be a Canvas) ────────────────
// Each tile is a `RoundedImage { radius: 0; fit: "crop" }`, i.e. a plain
// filtered `Image` that requests its own scaled derivative at its own device
// size through the existing `QbzSession.artScaled` seam. The previous body was
// a per-card `Canvas` whose `Context2D.drawImage` does NOT filter when it
// scales, so a 600px member cover in a ~100px mosaic cell was EXACTLY
// nearest-neighbour (measured RMSE 0.000 against `-filter Point`) — the
// artwork fix that reached RoundedImage never reached this file. Reuse over a
// second copy: RoundedImage already owns the intrinsic-size latch, the
// derivative request, the crop fit, the readiness latch and the
// recycled-delegate invalidation.
//
// Only the collage's OUTER shape is rounded; the tiles are square-cornered.
// That rounding is one `layer.enabled` + one `MultiEffect` mask on the mosaic
// (NOT one per tile), the same mechanism theme/RoundedImage.qml documents.
// Effects need shaders: this port runs on the GPU (OpenGL RHI, measured
// 2026-07-29), and the earlier "shader masks render nothing" note in this
// header came from an offscreen session, which forces the software renderer by
// definition. Where a software path is genuinely possible it is DETECTED
// (`GraphicsInfo.api`) — there the mask is skipped and the mosaic reads
// square-cornered over the already-rounded placeholder fill, rather than
// blank.
//
// PARITY NOTE (pre-existing gap, not introduced here): Slint's `n == 1` arm
// (PlaylistCollage.slint:39-49) contain-fits the single cover over a
// `playlist.dominant-color` letterbox; this port crops it, because the Qt feed
// publishes no `dominantColor` field. Logged for the UX pass — a perf fix must
// not invent a producer field.
//
// Artwork is URL-keyed, not artKey-keyed (a playlist row has ONE artKey but
// up to four covers): the component hands its urls to the shared url-keyed
// resolver — `QbzShell.sidebarArtworkWindow`, the same entry point the
// sidebar tree's micro-collage uses — and listens for the
// `libraryArtworkReady` echo. Disk hits come back synchronously; misses are
// downloaded once and cached. The dispatch is debounced so flinging through
// a windowed grid never fetches covers for delegates that are already gone.

import QtQuick
import QtQuick.Effects
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    // Remote cover urls (feed row `covers`); only the first four distinct
    // entries are drawn.
    property var urls: []
    property real radius: 8
    // Mosaic seam, 1:1 with PlaylistCollage.slint.
    property real gap: 2

    /// `"mosaic"` (discover/PlaylistCollage.slint — the default everywhere) or
    /// `"pm"` (primitives/PmCollage.slint, the Playlist Manager's thumbnail).
    ///
    /// They are genuinely different layout FUNCTIONS, which is the whole delta:
    /// the mosaic splits 2 and 3 covers into cells, while the manager collapses
    /// anything under four to ONE full-bleed cover and only splits at four, with
    /// a 1 px seam instead of 2. Everything else — the url → path resolution
    /// through QbzShell.sidebarArtworkWindow + QbzLibrary.libraryArtworkReady,
    /// the 120 ms debounce, the MultiEffect rounding with the measured
    /// maskThresholdMin 0.5 / maskSpreadAtMin 1.0, the three-way `_noShaders`
    /// test and the recycled-delegate invalidation — is identical and must not
    /// be forked into a second file.
    property string layout: "mosaic"

    /// The manager pins its seam at 1 px (PmCollage.slint:27).
    readonly property real effGap: root.layout === "pm" ? 1 : root.gap

    /// How many of the (up to four) tiles this layout actually draws. The
    /// mosaic draws every cover it has; `"pm"` draws ONE below four.
    readonly property int drawnTiles: root.layout === "pm"
        ? (root.tiles.length >= 4 ? 4 : (root.tiles.length >= 1 ? 1 : 0))
        : root.tiles.length

    QbzTheme { id: theme }

    // De-duplicated + capped at four (Tauri dedupes the same way: repeated
    // albums in a playlist must not paint the same tile twice).
    readonly property var tiles: root.dedup(root.urls)
    // url -> local cached path, filled by the artwork signal.
    property var pathMap: ({})

    /// See theme/RoundedImage.qml: the ONLY case the retired "effects render
    /// nothing" doctrine was ever true for is the software/Null renderer, and
    /// `GraphicsInfo.api` answers it per window at runtime (`api` carries
    /// `notify: "apiChanged"`, so this re-evaluates when the window arrives).
    /// Test Software/Null negatively — under the RHI an OpenGL backend reports
    /// `GraphicsInfo.OpenGL`.
    ///
    /// `QbzShell.forceCanvasArt` (env `QBZ_QT_ROUND_MODE=canvas`, default OFF)
    /// is honoured HERE TOO, and that is not cosmetic symmetry: the override
    /// exists for a machine where `GraphicsInfo` reports a GPU and the mask
    /// still misbehaves. If the mask draws nothing there, this file's mask
    /// draws nothing either and the whole mosaic disappears — pinning the
    /// tiles onto RoundedImage's canvas arm while leaving the mosaic masked
    /// would defeat the override on exactly the surface it was added for.
    /// Square outer corners (the same degrade the software path already takes,
    /// see the header) is the correct fallback. `typeof` guard mirrors
    /// RoundedImage: eager binding, and the file must stay loadable in an
    /// isolated qml6 scene.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null
        || (typeof QbzShell !== "undefined" && QbzShell.forceCanvasArt)

    function dedup(list) {
        var out = []
        var seen = ({})
        var src = list || []
        for (var i = 0; i < src.length && out.length < 4; i++) {
            var u = src[i]
            if (!u || seen[u] === true)
                continue
            seen[u] = true
            out.push(u)
        }
        return out
    }

    function pathAt(index) {
        var u = root.tiles[index]
        return u ? (root.pathMap[u] || "") : ""
    }

    /// The mosaic layout, transcribed from the deleted `onPaint` and confirmed
    /// against discover/PlaylistCollage.slint line by line: `gap: 2px` (:19),
    /// n==2 halves `(size - gap) / 2` (:57), n==3 `0.62` / `0.38` with
    /// `(size - gap) / 2` rows (:74, :82-89), n>=4 `0.34` / `0.66` with
    /// `(size - 2*gap) / 3` rows (:102-115, :121). These numbers do not
    /// change. Returns an empty rect for a tile this playlist does not have.
    function cellOf(index) {
        var w = root.width
        var h = root.height
        var n = root.tiles.length
        if (w <= 0 || h <= 0 || n <= 0 || index >= root.drawnTiles)
            return Qt.rect(0, 0, 0, 0)
        var g = root.effGap
        // Playlist Manager: 1-3 covers -> one full-bleed tile, 4+ -> a 2×2 grid
        // (PmCollage.slint:39-80).
        if (root.layout === "pm") {
            if (n < 4)
                return Qt.rect(0, 0, w, h)
            var cwPm = (w - g) / 2
            var chPm = (h - g) / 2
            return Qt.rect((index % 2) * (cwPm + g),
                           Math.floor(index / 2) * (chPm + g), cwPm, chPm)
        }
        if (n === 1)
            return Qt.rect(0, 0, w, h)
        if (n === 2) {
            // Side-by-side halves.
            var half = (w - g) / 2
            return index === 0 ? Qt.rect(0, 0, half, h)
                               : Qt.rect(half + g, 0, half, h)
        }
        if (n === 3) {
            // One large left (62%), two stacked right (38%).
            var big3 = (w - g) * 0.62
            var small3 = (w - g) * 0.38
            var half3 = (h - g) / 2
            if (index === 0) return Qt.rect(0, 0, big3, h)
            if (index === 1) return Qt.rect(big3 + g, 0, small3, half3)
            return Qt.rect(big3 + g, half3 + g, small3, half3)
        }
        // Three stacked left (34%), one large right (66%).
        var col4 = (w - g) * 0.34
        var big4 = (w - g) * 0.66
        var row4 = (h - 2 * g) / 3
        if (index === 0) return Qt.rect(0, 0, col4, row4)
        if (index === 1) return Qt.rect(0, row4 + g, col4, row4)
        if (index === 2) return Qt.rect(0, 2 * (row4 + g), col4, row4)
        return Qt.rect(col4 + g, 0, big4, h)
    }

    // Placeholder surface. A Rectangle's OWN rounded fill paints correctly —
    // only its children need a mask.
    Rectangle {
        anchors.fill: parent
        radius: root.radius
        color: theme.surfaceElevated
    }

    // No covers at all — the list-music glyph (PlaylistCollage.slint n <= 0).
    QbzIcon {
        visible: root.tiles.length === 0
        name: "list-music"
        width: Math.round(root.width * 0.32)
        height: width
        anchors.centerIn: parent
        tintName: "muted"
    }

    // Debounced url dispatch: one call per settled delegate, only for tiles
    // whose path is still unknown.
    Timer {
        id: dispatchDelay
        interval: 120
        onTriggered: {
            var pending = []
            for (var i = 0; i < root.tiles.length; i++) {
                var u = root.tiles[i]
                if (!root.pathMap[u])
                    pending.push(u)
            }
            if (pending.length > 0)
                QbzShell.sidebarArtworkWindow(JSON.stringify(pending))
        }
    }

    onTilesChanged: {
        if (root.tiles.length > 0)
            dispatchDelay.restart()
    }
    Component.onCompleted: {
        if (root.tiles.length > 0)
            dispatchDelay.restart()
    }

    Connections {
        target: QbzLibrary
        // Shared with the feed's artKey-keyed emissions — ignore anything
        // that is not one of OUR urls.
        function onLibraryArtworkReady(key, path) {
            if (root.pathMap[key] === path)
                return
            var mine = false
            for (var i = 0; i < root.tiles.length; i++) {
                if (root.tiles[i] === key) {
                    mine = true
                    break
                }
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
        // Also gated on `tiles.length` — a layer renders even when its item is
        // invisible (that is the whole mask idiom), so a coverless card would
        // otherwise pay two FBOs to mask a glyph that is not inside them.
        layer.enabled: !root._noShaders && root.radius > 0 && root.tiles.length > 0
        layer.smooth: true
        Rectangle {
            anchors.fill: parent
            radius: root.radius
            color: "#ffffff"
        }
    }

    // ONE layer + ONE mask for the whole mosaic, not one per tile. `clip: true`
    // bounds the outer rect; each tile's own crop overflow is scissored by
    // RoundedImage itself (`clip` there covers exactly the radius-0 crop case
    // this file creates — the deleted Canvas clipped per tile).
    Item {
        id: mosaic
        anchors.fill: parent
        // No clip: cellOf() tiles the parent rect exactly, and each tile is a
        // RoundedImage that scissors itself when it needs to.
        visible: root.tiles.length > 0
        layer.enabled: !root._noShaders && root.radius > 0 && root.tiles.length > 0
        layer.smooth: true
        layer.effect: MultiEffect {
            maskEnabled: true
            maskSource: maskSrc
            // 0.5 / 1.0 — MEASURED, see theme/RoundedImage.qml for the full
            // table. Qt's mask is a smoothstep CENTRED on maskThresholdMin of
            // width maskSpreadAtMin, so (0.5, 1.0) is the only pair that maps
            // mask alpha 0..1 onto 0..1 and therefore the only one that
            // reproduces `ctx.clip()`'s antialiased corner. (0.0, 0.0)
            // collapses to step(0, alpha) and the outer corners come back
            // HARD (0 antialiased pixels vs the Canvas's 292); (0.0, > 0)
            // clamps everything opaque and disables the mask outright. Do not
            // "restore the defaults" here.
            maskThresholdMin: 0.5
            maskSpreadAtMin: 1.0
        }

        Repeater {
            model: 4
            delegate: RoundedImage {
                required property int index
                readonly property rect cell: root.cellOf(index)
                visible: index < root.drawnTiles
                x: cell.x
                y: cell.y
                width: cell.width
                height: cell.height
                source: root.pathAt(index)
                // Square-cornered: only the mosaic's outer shape is rounded.
                // radius 0 also means this RoundedImage mounts a plain Image
                // with NO layer and NO mask of its own.
                radius: 0
                fit: "crop"
            }
        }
    }
}
