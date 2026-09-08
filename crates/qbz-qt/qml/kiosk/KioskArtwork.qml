// KioskArtwork — THE image primitive of the kiosk shell.
//
// Kiosk-only by construction: theme/RoundedImage.qml is the DESKTOP artwork
// path and is not touched by this file, by any kiosk card, or by anything in
// this directory. Both can live in the same process because they share the one
// thing that matters — QQuickPixmapCache and the Rust artwork cache on disk.
//
// ── WHY A SECOND IMAGE COMPONENT AT ALL ───────────────────────────────────
// RoundedImage cannot request a decode size. It says so itself, twice, and for
// a good reason on the desktop: `sourceSize` is part of the QQuickPixmapCache
// key, so asking for one there would FORK a second decode of the file its Rust
// derivative pipeline has already sized. That pipeline is what makes the
// desktop correct, and it works like this:
//
//   1. mount a hidden probe Image on the ORIGINAL to learn its intrinsic size,
//   2. wait for that probe to reach Image.Ready — i.e. DECODE THE ORIGINAL,
//   3. ask Rust for a derivative at the drawn device size,
//   4. swap `source` to the derivative when it lands.
//
// Step 2 is the whole problem on a 7" panel. Contract §5.2 is explicit that
// "usa RoundedImage" does not close the artwork gate precisely because the
// cold-cache path can download and decode a 600px original into a 96px cell
// before the correctly-sized file is even requested. On a Pi that is the
// difference between a grid that settles and one that swaps textures for
// seconds. Kiosk therefore does the opposite trade: ONE Image, `sourceSize`
// pinned to the physical box, no probe, no derivative round-trip, no second
// decode to fork because there is no first one.
//
// ── WHAT THIS COMPONENT WILL NOT DO ───────────────────────────────────────
//   - It has NO network pipeline. It never downloads through a path Qt's own
//     Image would not already have taken, never writes a cache, and never
//     calls QbzSession.artScaled / artScaledCached. When the host hands it a
//     resolved local cache path — which is what every kiosk host does today
//     (KioskLocalLibrary.artPathOf, KioskArtist.coverMap, home `artPath`) —
//     that path is used verbatim.
//   - It mounts NO probe Image, so a cell costs one pixmap, not two.
//   - It runs NO animation. No fade, no Behavior, no Timer. Contract §3.3: a
//     dirty item presents the WHOLE window, and the kiosk profile is
//     reduce-motion by default. Art appears.
//
// ── PROVIDER BUCKETS (`source` is a url) ──────────────────────────────────
// Qobuz serves one cover at fixed edges — 50/100/150/230/300/600/max/org — as
// a `_<edge>.jpg` token on the last path segment. `qbz-source`'s
// `QobuzSource::small_url` (sources/qobuz.rs:170-187) is the authority on that
// shape and this file mirrors its token list exactly, including `_org`/`_max`.
//
// Cache requests and direct image inputs share assets/kiosk-art.js. It picks
// the smallest available bucket covering the physical frame, including when
// an incoming remote thumbnail was too small. Local cache files are unchanged.
// resolvedSource/requestedPx expose the budgets for runtime instrumentation.
//
// ── DECODE SIZE ───────────────────────────────────────────────────────────
// `sourceSize` is a square whose edge is the LONGER logical side times the
// screen's device pixel ratio, rounded up to a multiple of 8.
//
//   square, and that is EXACT for both fits without measuring the source,
//     because QQuickImage picks the aspect policy from `fillMode`: a
//     PreserveAspectCrop image expands the decode until the box is COVERED,
//     a PreserveAspectFit one fits it inside. Measured on this Qt build with
//     an 800x380 banner in a 128 px crop cell: the decode came back 269x128,
//     i.e. covering, not 128x61. A square 600x600 cover in a 96 px cell came
//     back 96x96. Non-square crops are an explicit aspect-ratio exception:
//     269px exceeds the 256px two-times edge budget by 13px, the minimum
//     width needed to cover 128px vertically without distorting this banner.
//   quantised to 8, because `sourceSize` is part of the pixmap-cache key: a
//     cell whose width jitters by a pixel during layout would otherwise
//     re-request — and re-decode — on every jitter. Quantising also makes
//     neighbouring cells of nearly equal size SHARE one cache entry.
//
// ── ROUNDING ──────────────────────────────────────────────────────────────
// Same fast arm RoundedImage documents and measured on this Qt build: one
// `layer.enabled` Image + a MultiEffect masked by a rounded Rectangle, with
// maskThresholdMin 0.5 / maskSpreadAtMin 1.0 — the only pair that maps mask
// alpha monotonically onto 0..1 (RoundedImage.qml:592-620 carries the fringe
// counts; do not "restore the defaults").
//
// There is deliberately NO canvas arm. On the software/Null renderer, where
// shaders do not exist, this degrades to an UNROUNDED image inside the host's
// own rounded tile rather than paying a per-cell CPU raster. A kiosk on the
// software path is already the worst case; a Canvas per cell is what would
// make it unusable, and the offscreen harness is the main consumer of that
// path.
//
// ── READINESS ─────────────────────────────────────────────────────────────
// `ready` is the same contract RoundedImage publishes, so a host can gate a
// KioskSkeleton on it: true only when the CURRENT source is decoded, MONOTONE
// within one source, cleared only by `onSourceChanged` so a recycled GridView
// delegate cannot inherit the previous cell's readiness.

import QtQuick
import QtQuick.Window
import "../assets/kiosk-art.js" as ArtPolicy
import QtQuick.Effects

Item {
    id: root

    /// A local cache path, or a remote url. Empty draws nothing and leaves the
    /// host's placeholder tile showing.
    property string source: ""
    /// Corner radius in logical px. `Math.min(w, h) / 2` is a full circle
    /// (KioskArtistCard's avatar). 0 takes neither the layer nor the mask.
    property real radius: 8
    /// "crop"    PreserveAspectCrop — covers and avatars, the default.
    /// "contain" PreserveAspectFit — wordmarks and banners.
    property string fit: "crop"
    /// THE handover signal. True only when the current `source` is decoded.
    readonly property bool ready: root.source !== "" && root._imgReady
    /// True once the current source failed. Hosts that show an error tile read
    /// this; it is cleared by `onSourceChanged` like `_imgReady`.
    readonly property bool failed: root.source !== "" && root._imgFailed

    /// ── EVIDENCE HOOKS (contract §5.2) ────────────────────────────────────
    /// The square decode edge actually requested, in DEVICE pixels, and the
    /// url/path actually handed to the Image after the bucket rewrite. Both
    /// are readable from a harness so the artwork budget can be asserted
    /// rather than asserted-about.
    readonly property int requestedPx: root._px
    readonly property string resolvedSource: root._resolved

    // ── decode size ────────────────────────────────────────────────────────
    /// Device pixels per logical pixel for the screen this item is on. Held as
    /// a property so a window moved between screens of different scales
    /// re-requests, exactly as RoundedImage does.
    readonly property real _dpr: Screen.devicePixelRatio > 0 ? Screen.devicePixelRatio : 1
    /// The longer logical side is what a crop fit has to cover.
    readonly property real _edge: Math.max(root.width, root.height)
    /// Quantised to 8 so layout jitter cannot re-key the pixmap cache.
    readonly property int _px: root._edge > 0 && root._dpr > 0
        ? Math.ceil(root._edge * root._dpr / 8) * 8
        : 0

    // Shared with the Rust cache-request adapter; one provider-size policy.
    function bucketFor(px) { return ArtPolicy.bucketFor(px) }
    readonly property string _resolved: ArtPolicy.sizedUrl(root.source, root._px)

    // ── readiness latches ──────────────────────────────────────────────────
    // LATCH, do not mirror: `status` is not monotone (a cache eviction or a
    // re-layout can send it back to Loading), and a mirrored flag would
    // un-retire a host's skeleton over art that is already on screen — the
    // exact regression RoundedImage.qml:555-567 documents.
    property bool _imgReady: false
    property bool _imgFailed: false
    onSourceChanged: {
        root._imgReady = false
        root._imgFailed = false
    }

    // ── arms ───────────────────────────────────────────────────────────────
    /// Testing Software/Null NEGATIVELY is the correct test under the RHI: an
    /// OpenGL backend reports GraphicsInfo.OpenGL and the `*Rhi` values are
    /// Qt5-era aliases. `api` notifies, so this is a live binding and an item
    /// that has no window yet (Unknown) takes the masked arm.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null
    readonly property bool _masked: root.radius > 0 && !root._noShaders
    readonly property bool _contains: root.fit === "contain"

    /// PreserveAspectCrop overflows the item. The layer confines it (the layer
    /// texture IS the item rect); with no layer and a crop fit nothing does, so
    /// scissor exactly in that one case — not unconditionally, which would put
    /// a scissor under every masked cover for nothing.
    clip: !root._masked && !root._contains

    /// The rounded shape, rendered to its own FBO once — it changes only with
    /// width/height/radius, never per frame. `visible: false` + layered is the
    /// documented MultiEffect mask idiom: the layer renders regardless of
    /// visibility, which is the point.
    Item {
        id: maskSrc
        anchors.fill: parent
        visible: false
        layer.enabled: root._masked
        layer.smooth: true
        Rectangle {
            anchors.fill: parent
            radius: root.radius        // Rectangle clamps internally
            color: "#ffffff"
        }
    }

    Image {
        id: img
        anchors.fill: parent
        source: root._resolved
        // THE decode-size request. Square (see the header) and zero-guarded:
        // Qt reads sourceSize 0 as "intrinsic", which is precisely what must
        // not happen, so an item with no geometry yet shows nothing rather
        // than decoding an original.
        sourceSize.width: root._px
        sourceSize.height: root._px
        visible: root._px > 0
        fillMode: root._contains ? Image.PreserveAspectFit : Image.PreserveAspectCrop
        asynchronous: true
        cache: true
        smooth: true
        mipmap: false
        // Qt >= 6.8. Keeps the PIXELS while a size change reloads, so a cell
        // does not blank for the frames the new decode takes.
        retainWhileLoading: true

        // Called from onStatusChanged AND from Component.onCompleted: an
        // already-cached pixmap can reach Image.Ready DURING the evaluation of
        // the `source` binding, i.e. before the handler is connected, and a
        // missed latch would leave `ready` false forever.
        function _sync() {
            if (status === Image.Ready)
                root._imgReady = true
            if (status === Image.Error)
                root._imgFailed = true
        }
        onStatusChanged: img._sync()
        Component.onCompleted: img._sync()

        layer.enabled: root._masked
        layer.smooth: true
        layer.effect: MultiEffect {
            maskEnabled: true
            maskSource: maskSrc
            // 0.5 / 1.0 were MEASURED on this Qt build, not reasoned — see
            // theme/RoundedImage.qml:592-620 for the fringe-pixel counts of
            // every alternative. Do not "restore the defaults".
            maskThresholdMin: 0.5
            maskSpreadAtMin: 1.0
        }
    }
}
