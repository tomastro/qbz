// RoundedImage — an Image with TRUE rounded corners (PreserveAspectCrop).
//
// Why this exists: QML's `clip` is rectangular — a Rectangle with
// `radius` + `clip: true` does NOT clip children to the rounded shape
// (proven with an isolated scene on this Qt build: the child paints
// square over the rounded fill). So the curve has to come from a mask.
//
// ── HOW THE ROUNDING IS DONE (and the doctrine this file used to carry) ────
// Effects need shaders. This port runs on the GPU (OpenGL RHI, measured
// 2026-07-29 with QSG_INFO=1 in a real windowed session); the earlier
// "renders nothing" note in this header came from an offscreen session, which
// forces the software renderer by definition. Where a software path is
// genuinely possible, detect it with `GraphicsInfo.api` rather than assuming
// it.
//
// So there are two arms, chosen per item, at runtime:
//
//   FAST ARM (default)  one `Image` + ONE `layer.enabled` whose `layer.effect`
//                       is a `MultiEffect` masked by a rounded `Rectangle`.
//                       The layer FBO and the mask FBO are pure functions of
//                       (width, height, radius) — they are rendered once and
//                       cached, so a static cover costs ZERO CPU raster and
//                       ZERO texture upload per frame. `radius <= 0` takes
//                       neither the layer nor the mask, i.e. a plain Image.
//   CANVAS ARM          the previous CPU raster (QPainter into a QImage,
//                       `renderTarget: Canvas.Image`), kept verbatim for the
//                       two cases that genuinely need it:
//                         * `fit: "pad"` — its band colours come from
//                           `Context2D.getImageData` on the pixels the canvas
//                           just drew, and there is no non-Canvas way to read
//                           those back;
//                         * a software / Null renderer (offscreen, and any
//                           platform `qt-diag.sh` shows on the software path)
//                           — no shaders, so no mask.
//                       `QbzShell.forceCanvasArt` (env `QBZ_QT_ROUND_MODE=
//                       canvas`, default OFF) pins this arm by hand.
//
// Usage is unchanged: `RoundedImage { anchors.fill: parent; source: artPath;
// radius: theme.radiusSm }` inside the same rounded placeholder Rectangle
// (the placeholder's OWN fill rounds fine — only children needed a mask).
//
// ── READINESS CONTRACT (`ready`) ───────────────────────────────────────────
// A host that draws a loading placeholder over this item MUST gate it on
// `ready`, never on "the path is non-empty".
//
// The path landing means NOTHING here: once `source` is assigned the file
// still has to be decoded (and, on the canvas arm, rasterized). A placeholder
// gated on the path therefore vanishes the instant the path arrives and leaves
// an EMPTY tile on screen until the pixels exist — the reported bug ("the
// skeleton disappears before the art is rendered").
//
// `ready` is true only when the CURRENT source is on screen:
//   fast arm    `Image.Ready` — the pixmap is decoded and its texture uploads
//               on the next render pass; QbzSkeleton's 180ms fade covers that
//               one frame (QbzSkeleton.qml:51-61 documents the same thing for
//               its own probe arm, implemented `handedOver` :184 -> `retired`
//               :214 -> `Behavior on opacity` :216).
//   canvas arm  `_paintedSource` — a paint pass actually drew THAT source.
// Both are MONOTONE WITHIN ONE SOURCE and cleared only by `onSourceChanged`,
// so a recycled list delegate cannot inherit the previous row's readiness and
// a mid-life derivative swap cannot un-retire a placeholder that has already
// handed over.
//
// Hosts that cannot reach this item (the image lives inside AlbumCard,
// PlaylistCard, …) use the equivalent self-probing arm of the placeholder
// itself: `QbzSkeleton { coverSource: <path> }` — see QbzSkeleton.qml.
//
// ── THE CANVAS ARM RASTERS IN DEVICE PIXELS ────────────────────────────────
// `onPaint` lays everything out in DEVICE pixels and pre-divides the context
// by the device pixel ratio, instead of drawing in the item's logical pixels.
//
// Measured on Qt 6.11 (200x200 item, 600x600 cover, xcb, QT_SCALE_FACTOR=2,
// error vs a Lanczos 600->400 reference): drawing in logical pixels lands at
// RMSE 0.100 and its content is bit-for-bit the DPR-1 200x200 raster blown up
// 2x (RMSE 0.0026 against exactly that upscale) — half the detail the screen
// can show. The same paint in device pixels lands at RMSE 0.061 with a real
// 400x400 raster. At DPR 1 `scale(1,1)` makes this byte-identical to the old
// body, so it costs nothing on an unscaled screen; the backing store is
// device-sized either way, so it costs no extra memory on a scaled one.
//
// `_dpr` is a property, not a local, so a window dragged from a 1x screen to
// a 2x one repaints instead of leaving the old raster stretched.
//
// ── WHY THE SCALED DERIVATIVE EXISTS ───────────────────────────────────────
// `Context2D.drawImage` does NOT filter when it scales: measured against
// ImageMagick references, a 600 -> 200 draw is EXACTLY nearest-neighbour
// (RMSE 0.000 vs -filter Point) and so is a mild 230 -> 200 draw. The Canvas
// is loss-FREE only at 1:1 (RMSE 0.000 with a 200px source). Qt gives no way
// to ask `Canvas.loadImage()` for a decode size — `sourceSize` on the probe
// does not leak into it (the QQuickPixmap cache keys on the requested size),
// `drawImage(<Image item>)` draws nothing, and `grabToImage` refuses a hidden
// item. The cure is therefore a SOURCE FILE already at the drawn device size,
// which comes from the Rust artwork pipeline (`artwork_qt::scaled_path`).
//
// That derivative shipped, and then was never drawn: `onPaint` still passed
// `source` — the untouched 600px original — to `drawImage`, so every card was
// point-sampled while a correctly-downscaled file sat unused in the cache
// next to it. That is the whole of the "art in the *Cards looks bad but the
// hover zoom of the SAME image looks fine" report: the hover preview
// (`ArtPreviewOverlay`) is a plain `Image` with `smooth` + `mipmap`, which
// filters. Grabbed side by side at 200px against a Lanczos reference:
//
//   card as shipped (drawImage(source))     RMSE 4.277   == nearest, exactly
//   card drawing the derivative             RMSE 1.303
//   ArtPreviewOverlay's plain Image         RMSE 2.832
//
// The derivative is kept on BOTH arms: the fast arm's `Image` filters, but a
// file already at the drawn size is still one decode of 200x200 instead of
// 600x600, and it is what makes `pad` 1:1.
//
// ── NON-SQUARE SOURCES: `fit: "pad"` / `fit: "auto"` ───────────────────────
// Qobuz playlist artwork (`image_rectangle`) is 800x380 — a 2.11:1 banner,
// verified on every such file in the local cache (179 non-square covers, all
// but three of them exactly 800x380). Dropping that into a square cell has
// three possible answers and the first two are both wrong:
//
//   crop     what Slint does (`playlist/PlaylistView.slint:481` and its
//            collage single-tile arm are `image-fit: cover`) — it keeps the
//            centre 380 of 800 px, i.e. it THROWS AWAY 53% of the width and
//            reads on screen as a huge zoom. This is a case where the
//            reference is not the target: the owner reported it on the Qt
//            surfaces, but Slint gets it wrong the same way.
//   contain  aspect kept, but the two uncovered bands are a flat theme
//            colour, so the tile reads as a small picture floating in a grey
//            box that does not match it.
//   pad      aspect kept, and the uncovered bands are a gradient TAKEN FROM
//            THE IMAGE — this file's mode, and what the owner asked for.
//
// `pad` samples the strip of the DRAWN art adjacent to each seam, spreads
// those samples as an 8-stop gradient along the band's long axis, and lays a
// transparent -> black gradient over it running outward from the seam. The
// colour at the seam is therefore the image's own edge pixels: there is no
// visible join, and the picture dissolves into the frame instead of ending at
// a hard rectangle.
//
// The colours come from `Context2D.getImageData` on the pixels THIS canvas
// just drew, so no second decode, no extra cache dir and no Rust round-trip.
// Measured against a PIL average of the same source strip: (11,18,27) read
// back vs (9,17,25) — faithful. That read-back is the reason `pad` keeps the
// Canvas: it is not a renderer limitation, it is that nothing else can hand
// back the pixels it just drew.
//
// `getImageData` here addresses the DEVICE-pixel backing store and IGNORES
// the active transform — the same space `onPaint` already lays everything out
// in, so the sampler needs no conversion at all. Both halves of that were
// measured on this Qt build rather than taken from the HTML spec, and both
// are surprising:
//   * transform ignored — with `scale(0.5)` applied, reads at y=60 and y=90
//     both landed in the band drawn at USER-space y=100..200, which only an
//     untransformed read explains;
//   * device, not logical — under QT_SCALE_FACTOR=2 on a 200px item,
//     `canvasSize` still reports 200x200, yet a read at y=300 returned the
//     fill and a read at y=150 returned nothing.
// The first version of this file divided by `_dpr` on the strength of the
// spec, and produced NO band at all on any scaled screen while looking
// perfect at 1x. Clamp bounds are device extents for the same reason.
//
// `fit: "auto"` picks `pad` or `crop` from the source's MEASURED aspect
// ratio. It exists because the flag that says "this is the playlist's own
// graphic" (`playlistOwnImage`) is only published by `library_qt.rs`: the
// Home / For You / Search / Browse rails map their playlist art from
// `image.rectangle` (`home_qt.rs::map_playlist`) with no such field, so a
// flag-driven fix would have left every Discover surface cropping. Measuring
// the ratio needs no producer change and cannot go stale.

import QtQuick
import QtQuick.Window
import QtQuick.Effects
import com.blitzfc.qbz

Item {
    id: root
    property string source: ""
    property real radius: 8
    // "crop"    PreserveAspectCrop — album art, the default.
    // "contain" PreserveAspectFit, flat uncovered area — label wordmarks
    //           (LabelCard.slint), which are transparent PNGs whose edge
    //           pixels carry no colour to derive a gradient from.
    // "pad"     PreserveAspectFit + the image-derived gradient bands.
    // "auto"    "pad" when the source measures non-square, else "crop".
    property string fit: "crop"
    /// How dark the `pad` bands go at the OUTER edge, as the alpha of a black
    /// overlay (0 = the edge colour continues flat to the frame). 0.55 keeps
    /// the band unmistakably the image's own hue while still reading as a
    /// vignette, matching the 0.20 vignette the atmosphere pipeline puts on
    /// the album/artist headers.
    property real padFalloff: 0.55
    /// Aspect tolerance for `fit: "auto"`. 1.10 = "more than 10% off square".
    /// Qobuz playlist banners are 2.105; real album covers measure 1.000 and
    /// the handful of near-square outliers in the cache (550x540 = 1.019) stay
    /// on the crop path where they belong.
    property real padAspectTolerance: 1.10

    /// The fit actually used, after `auto` resolves against the MEASURED
    /// source ratio (`_srcW`/`_srcH`, latched below). Falls back to "crop"
    /// until the size is knowable, which is the pre-existing behaviour and
    /// costs nothing: the latch is set by the same probe pass that makes the
    /// first paint possible, so no cell ever draws cropped and then re-lays.
    readonly property string _fit: root.fit !== "auto"
        ? root.fit
        : ((root._srcW > 0 && root._srcH > 0
            && (root._srcW > root._srcH * root.padAspectTolerance
                || root._srcH > root._srcW * root.padAspectTolerance))
            ? "pad" : "crop")
    /// Both fit-inside modes. The derivative request and the draw geometry
    /// must agree on this or the scaled file comes back the wrong shape.
    readonly property bool _contains: root._fit === "contain" || root._fit === "pad"

    /// ── ARM SELECTION — detected, not configured ───────────────────────────
    /// Shaders are unavailable on the software/Null renderer (and that is the
    /// ONLY case the retired "effects render nothing" doctrine was ever true
    /// for: it was observed under QT_QPA_PLATFORM=offscreen, which forces
    /// software by definition). GraphicsInfo.api answers it exactly, per
    /// window, at runtime — so the offscreen gate keeps drawing covers the way
    /// it does today and macOS gets the mask the moment it reports Metal.
    ///
    /// `api` carries `notify: "apiChanged"`, so this IS a live binding: before
    /// the item has a window the value is `Unknown` (fast arm) and it
    /// re-evaluates when the window arrives. Under the RHI an OpenGL backend
    /// reports `GraphicsInfo.OpenGL` and the `*Rhi` values are Qt5-era
    /// aliases, so testing Software/Null NEGATIVELY is the correct test — do
    /// not test for OpenGL positively.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null
    /// `pad` samples the pixels it just drew (Context2D.getImageData); there
    /// is no non-Canvas way to read them back, so that mode keeps the raster.
    /// It is confined to Qobuz playlist banners (fit "pad" / a resolved
    /// "auto"), i.e. 179 covers in the local cache, not every album cover.
    ///
    /// `typeof QbzShell !== "undefined"` mirrors the guard `_requestScaled`
    /// already uses for QbzSession: a registered singleton is always defined
    /// in the built binary, but the guard keeps this file loadable in an
    /// isolated qml6 scene, which is how the "clip does not round" finding in
    /// the header was obtained.
    readonly property bool _useCanvas: root._fit === "pad" || root._noShaders
        || (typeof QbzShell !== "undefined" && QbzShell.forceCanvasArt)

    /// PreserveAspectCrop overflows the item. A layer confines it (the layer
    /// texture IS the item rect); with no layer and a crop fit, nothing does —
    /// and a cropped tile would paint over its neighbour (PlaylistCollage's
    /// mosaic, whose Canvas clipped per tile). Cheapest correct rule: scissor
    /// exactly in the one case that has neither. NOT `clip: true`
    /// unconditionally — that would put a scissor under every layered cover
    /// for nothing.
    clip: !root._useCanvas && root.radius <= 0 && !root._contains

    /// THE handover signal — see the READINESS CONTRACT above. True only once
    /// the current `source` is on screen, not merely known.
    ///
    /// MONOTONE WITHIN A SOURCE, and that is not cosmetic: `QbzSkeleton`'s
    /// `retired` is a live binding, so anything that drops `ready` back to
    /// false brings the placeholder back over drawn art. Five hosts bind this
    /// directly — FolderSubcard.qml:62, LocalAlbumRow.qml:115,
    /// LocalAlbumHeader.qml:63, LocalEphemeralPane.qml:168,
    /// LocalArtistRow.qml:70 — and every `coverSource` host observes the same
    /// shape through its own probe.
    readonly property bool ready: root.source !== ""
        && (root._useCanvas ? (root._paintedSource === root.source)
                            : root._imgReady)
    /// Fast-arm latch: set true on `Image.Ready`, cleared ONLY by
    /// `onSourceChanged`. See the display Image's `onStatusChanged` for why
    /// this must not mirror `status`.
    property bool _imgReady: false
    /// Canvas-arm equivalent: the source of the last paint that actually drew
    /// pixels. Internal; it exists as a property (not a bool flag) so that
    /// `ready` invalidates itself on every source change.
    property string _paintedSource: ""

    /// ── THE SCALED SOURCE ──────────────────────────────────────────────────
    /// drawImage does not filter when it scales, so a 600px cover in a 200px
    /// cell is point-sampled — measured RMSE 0.000 against an ImageMagick
    /// `-filter Point` reference. The cure is a source file ALREADY at the
    /// drawn device size, so the draw is 1:1 and there is no resample at all.
    /// Rust produces the derivative once per (cover, size) and answers on
    /// `artScaledReady`, keyed by the WHOLE request — cover AND size — so a
    /// recycled delegate cannot take the previous row's cover and a 44px
    /// thumbnail cannot take the 200px card's file for the same cover.
    property string _scaled: ""
    readonly property string _effectiveSource: root._scaled !== "" ? root._scaled : root.source

    /// Intrinsic size of the ORIGINAL cover, captured from the probe on the
    /// passes where it is still showing `source` (`_scaled` is "" until the
    /// derivative answers). The request has to be the size the art is
    /// actually DRAWN at, and that depends on the aspect ratio and on `fit` —
    /// asking for the cell box instead is what produced the squashed
    /// derivatives on disk (square covers written as 32x28) and would ask for
    /// a square file to satisfy `LabelCard`'s `fit: "contain"` wordmarks.
    /// Held per-source; a recycled delegate cannot inherit the previous row's
    /// dimensions because `onSourceChanged` clears them.
    property int _srcW: 0
    property int _srcH: 0
    /// The last request actually sent, so width/height/dpr churn during layout
    /// does not re-enter the bridge (and its blocking decode) for a size that
    /// is already pending or on disk.
    property string _reqKey: ""

    /// Accept a derivative outside the Image binding's evaluation stack.
    /// A warm-cache hit is synchronous: assigning `_scaled` from
    /// `probe.onSourceSizeChanged` changes `_effectiveSource`, which changes
    /// that same probe's `source` while Qt is still evaluating it. Qt reports
    /// the cycle and may discard the binding. One event-loop turn preserves
    /// the pre-scenegraph fast path while making the handoff non-reentrant.
    function _acceptScaledLater(scaled, key) {
        if (scaled === "") return
        Qt.callLater(function() {
            if (root._reqKey === key && root._scaled !== scaled)
                root._scaled = scaled
        })
    }

    /// Latch the original's intrinsic size the first time it is knowable for
    /// the CURRENT source, then ask for the derivative.
    ///
    /// Called from several places on purpose. `probe.onStatusChanged` is not
    /// enough by itself: a recycled delegate that moves from one cached cover
    /// to another can go Ready -> Ready with no status change at all, and
    /// `onSourceSizeChanged` is equally silent when both covers are 600x600 —
    /// which is nearly all of them. Re-checking from `onImageLoaded` and from
    /// `onPaint` makes the latch self-healing: `_srcW` is cleared on every
    /// source change, so whichever event does fire performs the capture, and
    /// after it every later call is one integer compare.
    function _captureSrc() {
        if (root._srcW > 0 || root._scaled !== "") return
        if (probe.status !== Image.Ready || probe.sourceSize.width <= 0) return
        root._srcW = probe.sourceSize.width
        root._srcH = probe.sourceSize.height
        root._requestScaled()
    }

    function _requestScaled() {
        if (root.source === "" || root._srcW <= 0 || root._srcH <= 0) return
        if (typeof QbzSession === "undefined") return
        // Device pixels the art occupies, rounded up so a fractional DPR never
        // asks for less than the screen shows.
        var w = Math.ceil(root.width * root._dpr)
        var h = Math.ceil(root.height * root._dpr)
        if (w <= 0 || h <= 0) return
        // The SAME geometry the draw uses, so the answer is the drawn size.
        // `_contains`, not `fit === "contain"`: "pad" and a resolved "auto"
        // fit INSIDE the cell, and asking for the crop (max) scale there
        // would hand back a file wider than the band the art occupies —
        // the draw would resample it and lose the 1:1 blit this whole
        // mechanism exists for.
        var s = root._contains ? Math.min(w / root._srcW, h / root._srcH)
                               : Math.max(w / root._srcW, h / root._srcH)
        // Already at or below the drawn size: a derivative could only be an
        // upscale, and `scaled_path` refuses those anyway.
        if (s >= 1) return
        var rw = Math.max(1, Math.round(root._srcW * s))
        var rh = Math.max(1, Math.round(root._srcH * s))
        var key = root.source + "|" + rw + "x" + rh
        if (key === root._reqKey) return
        root._reqKey = key

        // Warm-cache fast path. Previously even a derivative already on disk
        // took the async Rust -> Tokio -> Qt-thread round trip. The original
        // pixmap therefore reached the scene graph first, then every card
        // replaced a large texture with its derivative a few frames later — a
        // visible burst of work when a cold ListView band mounted. The exact
        // path lookup is one local stat and performs no decode/write; on a
        // miss, generation remains off-thread through artScaled().
        var cached = QbzSession.artScaledCached(root.source, rw, rh)
        if (cached !== "") {
            root._acceptScaledLater(cached, key)
            return
        }
        QbzSession.artScaled(root.source, rw, rh)
    }
    Connections {
        target: typeof QbzSession !== "undefined" ? QbzSession : null
        function onArtScaledReady(path, scaled, w, h) {
            // Keyed on the WHOLE request — path AND size. A recycled delegate
            // that has already moved on ignores the answer meant for its
            // previous row, and (the case the path-only key got wrong) a 44px
            // slim thumbnail no longer accepts the 200px card's derivative for
            // the same cover, which point-sampled it right back to where this
            // whole mechanism started.
            var key = path + "|" + w + "x" + h
            if (root._reqKey === key)
                root._acceptScaledLater(scaled, key)
        }
    }

    // Dimension probe: the ORIGINAL's intrinsic size is what `_requestScaled`
    // and `fit: "auto"` need, and neither a Canvas nor a cropping Image can be
    // asked for it. A hidden Image doubles as the async loader notification.
    //
    // It follows `_effectiveSource`, not `source`: once the derivative lands
    // both this probe and the display Image reference the SAME file, so the
    // original drops to refcount 0 and becomes evictable — which is the other
    // half of the "RoundedImage keeps both the original and the derivative
    // loaded" report. (Releasing the probe outright by binding
    // `source: _srcW > 0 ? "" : source` is possible but requires reordering
    // `_captureSrc` to read into locals first, because assigning `_srcW`
    // would zero `sourceSize` mid-function. Deliberately NOT done here.)
    Image {
        id: probe
        source: root._effectiveSource
        visible: false
        asynchronous: true
        cache: true
        // NO sourceSize: it is part of the QQuickPixmapCache key, so a
        // decode-size request here would fork a SECOND decode of the file the
        // derivative pipeline already sized (QbzSkeleton.qml:178-180 documents
        // the same trap for its own probe).
        onStatusChanged: {
            // While `_scaled` is "" this IS the original, so this is where its
            // intrinsic size becomes knowable — and the derivative cannot be
            // requested before it (see `_srcW`).
            root._captureSrc()
            if (status === Image.Ready || status === Image.Error) root._repaint()
        }
        onSourceSizeChanged: root._captureSrc()
    }

    /// The rounded shape, rendered to its own FBO ONCE (it changes only with
    /// width/height/radius, never per frame). `visible: false` + layered is
    /// the documented MultiEffect mask idiom — the layer renders regardless of
    /// visibility, which is the whole point.
    Item {
        id: maskSrc
        anchors.fill: parent
        visible: false
        layer.enabled: root.radius > 0 && !root._useCanvas
        layer.smooth: true
        Rectangle {
            anchors.fill: parent
            radius: root.radius        // Rectangle clamps internally, exactly
            color: "#ffffff"           // as onPaint's Math.min(r, w/2, h/2) did
        }
    }

    /// ── THE ARRIVAL FADE ───────────────────────────────────────────────────
    /// Owner, 2026-08-21: "una vez que la imagen esta lista, esta aparece de
    /// golpe, parece un glitch... quiero evitar la sensación de salpicado."
    ///
    /// Every cover in the app comes through this file, so the fade belongs
    /// HERE and not in the cards: AlbumCard, PlaylistCard, TrackCard,
    /// ArtistCard, SlimCard, LabelCard and RadioCard all mount a RoundedImage,
    /// and 81 files in the tree do. A per-card animation would be seven copies
    /// that drift, and it would still miss the detail headers and the rows.
    ///
    /// WHAT TRIGGERS IT is `ready`, not the path and not `status`. `ready` is
    /// the readiness contract at the top of this file — MONOTONE WITHIN ONE
    /// SOURCE — which is exactly the property this fade needs: the derivative
    /// swap (`_effectiveSource`) sends `status` back to `Image.Loading`
    /// mid-life, and fading again there would flash a cover that is already on
    /// screen. A recycled delegate DOES re-fade, because `onSourceChanged`
    /// clears the latch, and that is correct: it is a different cover.
    ///
    /// GPU DOCTRINE (qt-frontend/2026-08-11-scenegraph-batches §9). The rule
    /// is that CONTINUOUS animation rides the shared shell pulse, because any
    /// dirty item presents the WHOLE window at ~1.2% GPU. This one is not
    /// continuous: it runs once per cover, for `fadeMs`, and then writes
    /// nothing forever. Nor does a grid multiply the bill — Qt advances every
    /// running animation on one animation tick and the window presents ONCE
    /// per frame regardless of how many items are dirty, so fifty covers
    /// arriving together cost the same presents as one. What extends the
    /// window is covers STREAMING in, and that is bounded by the load itself.
    ///
    /// OpacityAnimator, not a Behavior: an Animator runs on the render thread,
    /// so the fade keeps advancing while the GUI thread is still paying off a
    /// page mount — which is precisely when covers land. Same reasoning, and
    /// the same measurement, as ContentRouter's page fade.
    ///
    /// reduceMotion skips it: the art simply appears.
    property int fadeMs: 200

    // ── The two arms ────────────────────────────────────────────────────────
    // One Loader swap at startup (when GraphicsInfo.api resolves), not per
    // frame. `active` is a live binding on `_useCanvas`, which also follows
    // `_fit` — a `fit: "auto"` cell that resolves to "pad" moves to the canvas
    // arm on the same pass that resolves it.
    //
    // BOTH arms live inside `art` so the fade is ONE animator on ONE item
    // whichever arm is mounted. Only one is ever active, so per-node alpha on
    // this Item is group opacity in practice and no layer FBO is needed.
    Item {
        id: art
        anchors.fill: parent
        // Starts hidden; `_fade` reveals it. With reduceMotion on, the
        // Component.onCompleted below puts it straight to 1.
        opacity: 0.0

        Loader {
            id: fastArm
            anchors.fill: parent
            active: !root._useCanvas
            sourceComponent: imageComp
        }
        Loader {
            id: canvasArm
            anchors.fill: parent
            active: root._useCanvas
            sourceComponent: canvasComp
            // A newly created Canvas has loaded no image yet (the root-level
            // handlers already fired), so prime it here as well as there.
            onLoaded: root._repaint()
        }
    }

    OpacityAnimator {
        id: fadeIn
        target: art
        from: 0.0
        to: 1.0
        duration: root.fadeMs
        easing.type: Easing.OutCubic
    }

    /// Drive the fade off the readiness contract. A `ready` that goes false
    /// (a new source on a recycled delegate) re-arms by snapping back to 0,
    /// which is what stops the OLD cover from being visible under the new
    /// one's load.
    onReadyChanged: {
        fadeIn.stop()
        if (!root.ready) {
            art.opacity = 0.0
        } else if (QbzShell.reduceMotion) {
            art.opacity = 1.0
        } else {
            fadeIn.restart()
        }
    }


    Component {
        id: imageComp
        Image {
            anchors.fill: parent
            source: root._effectiveSource
            // "crop" = PreserveAspectCrop; "contain"/"pad" fit INSIDE. Same
            // predicate the derivative request uses, so the file that comes
            // back is the size this fillMode draws (see _requestScaled).
            fillMode: root._contains ? Image.PreserveAspectFit
                                     : Image.PreserveAspectCrop
            asynchronous: true
            cache: true
            // NO sourceSize: it is part of the QQuickPixmapCache key, so a
            // decode-size request here would fork a SECOND decode of the file
            // the derivative pipeline already sized.
            smooth: true
            mipmap: false
            // The derivative swaps `source` mid-life. Without this the item
            // blanks for the frames the new file takes to decode — the Canvas
            // never did (it kept the previous raster), so this is what keeps
            // the swap invisible. Qt >= 6.8.
            retainWhileLoading: true
            // Called from onStatusChanged AND from Component.onCompleted: an
            // already-cached pixmap can reach Image.Ready DURING the evaluation
            // of the `source` binding, i.e. possibly before the handler is
            // connected, and a missed latch would leave `ready` false forever —
            // a QbzSkeleton shimmering on top of drawn art at five direct
            // hosts. Same self-healing discipline as `_captureSrc`.
            function _sync() {
                // LATCH, do not mirror. `status` goes back to Image.Loading
                // when the derivative swaps `_effectiveSource` mid-life
                // (retainWhileLoading keeps the PIXELS, not the status), and a
                // mirrored `_imgReady` would drop `ready` to false for those
                // frames. `ready` false un-retires QbzSkeleton (`handedOver`
                // :184 -> `retired` :214 -> opacity 1.0), i.e. the placeholder
                // FADES BACK IN over art that is already on screen. The Canvas
                // never did that: `_paintedSource` is monotone within one
                // source. So: set true on Ready, and clear ONLY from
                // onSourceChanged.
                if (status === Image.Ready)
                    root._imgReady = true
                // FALLBACK TO THE ORIGINAL, which the Canvas has (`onPaint`
                // picks `source` when `_effectiveSource` is not loadable) and
                // this arm would otherwise lose. Reachable in production
                // because of the derivative-cache eviction:
                // `artwork_qt::evict_scaled` can unlink a derivative a live or
                // later-recycled delegate still names in `_scaled`, and
                // `_reqKey` would block the re-request, leaving the cover
                // permanently blank. Dropping both re-arms the pipeline.
                if (status === Image.Error && root._scaled !== "") {
                    root._scaled = ""
                    root._reqKey = ""
                }
            }
            onStatusChanged: _sync()
            Component.onCompleted: _sync()
            // Rounding: ONE layer + ONE mask, both static for this item's
            // life. radius 0 (LabelCard, LabelView, LabelReleasesView) takes
            // neither — those three used to pay a full CPU raster to round
            // nothing.
            layer.enabled: root.radius > 0
            layer.smooth: true
            layer.effect: MultiEffect {
                maskEnabled: true
                maskSource: maskSrc
                // ── 0.5 / 1.0, and BOTH numbers were MEASURED, not reasoned.
                // Do not "restore the defaults" here.
                //
                // Qt's mask maths is a smoothstep CENTRED on
                // `maskThresholdMin` whose width is `maskSpreadAtMin`, i.e.
                // roughly smoothstep(min - spread/2, min + spread/2, maskAlpha).
                // So (0.5, 1.0) is the only pair that maps the mask's alpha
                // 0..1 monotonically onto 0..1 — the straight alpha multiply
                // `ctx.clip()` gave. Grabbed a 200x200 cover under a
                // radius-40 mask on this Qt build (6.11.1, OpenGL RHI) and
                // counted the ANTIALIASED pixels (0 < alpha < 255):
                //
                //   Canvas arm (the baseline this replaces)   292 fringe px
                //   maskThresholdMin 0.0 / maskSpreadAtMin 0.0     0 fringe px
                //   maskThresholdMin 0.5 / maskSpreadAtMin 1.0   308 fringe px
                //   Qt5Compat OpacityMask (the sanctioned fallback) 308
                //
                // and on a FULL CIRCLE (radius 100 on 200px — ArtistCard 95,
                // ArtistView 100, HomeView's Spotlight hero 70):
                //
                //   Canvas 752 fringe px | 0.0/0.0 -> 0 | 0.5/1.0 -> 740
                //
                // (0.0, 0.0) collapses the smoothstep to `step(0, alpha)`:
                // every mask pixel with any alpha at all snaps to fully
                // opaque, so the arc comes back HARD and half a pixel fat —
                // a visibly stair-stepped circle at the six circle sites.
                // (0.0, anything > 0) is worse still: the low edge goes
                // negative, every pixel clamps to 1 and the mask is disabled
                // outright (measured: 40000/40000 px opaque, square corners).
                maskThresholdMin: 0.5
                maskSpreadAtMin: 1.0
            }
        }
    }

    // ── Root-level source/geometry handlers ─────────────────────────────────
    // The canvas arm needs an explicit loadImage + requestPaint from each of
    // these; the fast arm needs neither (its `Image` re-binds itself), so they
    // funnel through one guarded helper.

    /// No-op on the fast arm, by construction: `canvasArm.item` is null there.
    function _repaint() {
        if (canvasArm.item) {
            canvasArm.item.loadImage(root._effectiveSource)
            canvasArm.item.requestPaint()
        }
    }

    // requestPaint() as well as loadImage(): a delegate recycled onto a row
    // with NO cover assigns source = "", which emits no imageLoaded, so
    // without an explicit repaint the canvas would keep showing the previous
    // row's art under a placeholder that thinks it has nothing to cover.
    // A new cover invalidates the derivative: showing the previous row's
    // scaled file while the new one resolves is the stale-content bug again.
    onSourceChanged: {
        root._scaled = ""
        // The previous row's intrinsic size must not survive into this one, or
        // the request is computed against the wrong aspect ratio.
        root._srcW = 0
        root._srcH = 0
        root._reqKey = ""
        // THE ONLY place the fast-arm latch is cleared — see `_imgReady`.
        root._imgReady = false
        root._repaint()
    }
    on_EffectiveSourceChanged: root._repaint()
    Component.onCompleted: {
        root._repaint()
        // THE ARRIVAL FADE's self-heal: an already-cached pixmap can reach
        // Image.Ready DURING creation, i.e. before `onReadyChanged` is
        // connected — the same discipline the display Image's `_sync()`
        // documents. Without this a cover that was already in
        // QQuickPixmapCache would sit at opacity 0 forever.
        if (root.ready)
            art.opacity = 1.0
    }
    onWidthChanged: { root._repaint(); root._requestScaled() }
    onHeightChanged: { root._repaint(); root._requestScaled() }

    /// Device pixels per logical pixel for the screen this item is on. Held as
    /// a property so moving the window between screens of different scales
    /// re-rasterizes (see the DEVICE PIXELS note above).
    readonly property real _dpr: Screen.devicePixelRatio > 0 ? Screen.devicePixelRatio : 1
    on_DprChanged: { root._repaint(); root._requestScaled() }

    Component {
        id: canvasComp
        Canvas {
            id: canvas
            anchors.fill: parent
            renderTarget: Canvas.Image
            // GUI-thread raster. Cooperative/Threaded rasterize on the SCENE
            // GRAPH RENDER THREAD, and this item is created and destroyed by
            // list recycling while async artwork callbacks schedule repaints —
            // the render thread can be mid-draw on a pixmap whose Canvas is
            // already gone. Immediate keeps paint and destruction on one
            // thread. renderTarget: Image already meant CPU raster, so this
            // costs no visual change.
            renderStrategy: Canvas.Immediate

            onImageLoaded: { root._captureSrc(); canvas.requestPaint() }

            onPaint: {
                // Last of the self-healing capture hooks (see `_captureSrc`):
                // a paint means the cell is on screen, so if the size is
                // knowable at all it is knowable now. One integer compare once
                // the latch is set.
                root._captureSrc()
                var ctx = canvas.getContext("2d")
                ctx.reset()
                // Everything below is in DEVICE pixels; the context is
                // pre-divided so the item still occupies `width` x `height`
                // logical pixels.
                var dpr = root._dpr
                var w = Math.round(canvas.width * dpr)
                var h = Math.round(canvas.height * dpr)
                if (w <= 0 || h <= 0) return
                if (dpr !== 1) ctx.scale(1 / dpr, 1 / dpr)
                var r = Math.max(0, Math.min(root.radius * dpr, w / 2, h / 2))
                if (r > 0) {
                    ctx.beginPath()
                    ctx.moveTo(r, 0)
                    ctx.lineTo(w - r, 0)
                    ctx.arcTo(w, 0, w, r, r)
                    ctx.lineTo(w, h - r)
                    ctx.arcTo(w, h, w - r, h, r)
                    ctx.lineTo(r, h)
                    ctx.arcTo(0, h, 0, h - r, r)
                    ctx.lineTo(0, r)
                    ctx.arcTo(0, 0, r, 0, r)
                    ctx.closePath()
                    ctx.clip()
                }
                // Geometry comes from the ORIGINAL's intrinsic size, not from
                // the probe. The derivative preserves the aspect ratio, so
                // `dw`/`dh` are identical either way (they depend only on the
                // cell and the ratio) — but `_srcW`/`_srcH` do not go stale
                // for the frame in which the probe is re-loading after
                // `_scaled` lands, which would otherwise blank every card
                // once, mid-rail, the moment its derivative arrived.
                var iw = root._srcW > 0 ? root._srcW : probe.sourceSize.width
                var ih = root._srcH > 0 ? root._srcH : probe.sourceSize.height
                // Prefer the derivative; fall back to the original while it
                // resolves (or if it never does) so a cell is never empty for
                // want of it.
                var drawSrc = canvas.isImageLoaded(root._effectiveSource) ? root._effectiveSource
                            : (canvas.isImageLoaded(root.source) ? root.source : "")
                if (iw > 0 && ih > 0 && drawSrc !== "") {
                    // "crop" = uniform scale covering the rect;
                    // "contain"/"pad" = fitting inside it. All centered. Once
                    // the derivative is in play it is already `dw` x `dh`, so
                    // the blit is 1:1 — which is the entire point of it, since
                    // drawImage does not filter when it scales.
                    var scale = root._contains ? Math.min(w / iw, h / ih) : Math.max(w / iw, h / ih)
                    var dw = iw * scale
                    var dh = ih * scale
                    var ox = (w - dw) / 2
                    var oy = (h - dh) / 2
                    // `drawSrc`, NOT `source`: drawing the original here is
                    // what made the derivative dead weight and every card a
                    // nearest-neighbour downscale.
                    ctx.drawImage(drawSrc, ox, oy, dw, dh)
                    // The uncovered area, filled from the image itself. AFTER
                    // the draw on purpose: the sampler reads the pixels that
                    // were just laid down, so it needs no second decode and
                    // cannot disagree with what is on screen.
                    if (root._fit === "pad")
                        root._paintPad(ctx, dpr, w, h, ox, oy, dw, dh)
                    // ONLY here: pixels for `source` are now on the canvas.
                    // Anything earlier (the path arriving, the probe going
                    // Ready) would hand the cell over to an empty tile.
                    // Assigning the same string twice is a no-op in QML, so
                    // repeated paints cost no notifications.
                    root._paintedSource = root.source
                }
            }
        }
    }

    // ── The image-derived padding ─────────────────────────────────────────
    // Everything below runs in DEVICE pixels, like the rest of `onPaint` —
    // `getImageData` included (see the header note; this was measured, not
    // assumed, and the HTML-spec answer is the wrong one here). It stays on
    // ROOT rather than inside the canvas component: the context is passed in,
    // and keeping it here is what makes the moved body a pure relocation.

    /// Average the strip of drawn art next to a seam into `K` buckets along
    /// its long axis, so the band can carry the image's colour VARIATION and
    /// not just one flat mean. Returns an array of colours, or null when the
    /// strip is off-canvas or fully transparent (a logo with clear edges) —
    /// in which case the caller paints nothing and the host's placeholder
    /// Rectangle shows through, exactly as `fit: "contain"` behaves today.
    ///
    /// `cw`/`ch` are the DEVICE-pixel canvas extents, not `root.width`:
    /// clamping against the logical size silently produced an empty strip on
    /// every scaled screen (verified under QT_SCALE_FACTOR=2 — the band came
    /// back as the bare placeholder).
    function _stripStops(ctx, cw, ch, x, y, sw, sh, alongX) {
        var lx = Math.max(0, Math.min(Math.round(x), cw - 1))
        var ly = Math.max(0, Math.min(Math.round(y), ch - 1))
        var lw = Math.min(Math.max(1, Math.round(sw)), cw - lx)
        var lh = Math.min(Math.max(1, Math.round(sh)), ch - ly)
        if (lw < 1 || lh < 1)
            return null
        var img
        try {
            img = ctx.getImageData(lx, ly, lw, lh)
        } catch (e) {
            return null
        }
        var data = img.data
        var span = alongX ? lw : lh
        var K = Math.max(2, Math.min(8, span))
        var sr = [], sg = [], sb = [], n = []
        var k
        for (k = 0; k < K; k++) { sr[k] = 0; sg[k] = 0; sb[k] = 0; n[k] = 0 }
        // Sub-sampled along the long axis: every index of `data` crosses the
        // JS/C++ boundary, and a ~200x6 strip is 1200 pixels. Every third
        // pixel is the same average to well under one 8-bit step.
        var stepX = alongX ? 3 : 1
        var stepY = alongX ? 1 : 3
        for (var yy = 0; yy < lh; yy += stepY) {
            for (var xx = 0; xx < lw; xx += stepX) {
                var i = (yy * lw + xx) * 4
                if (data[i + 3] < 8)
                    continue
                var t = (alongX ? xx : yy) / span
                var b = Math.min(K - 1, Math.floor(t * K))
                sr[b] += data[i]; sg[b] += data[i + 1]; sb[b] += data[i + 2]; n[b]++
            }
        }
        // Buckets with no opaque pixel borrow their nearest filled neighbour
        // rather than collapsing to black — a partly transparent edge must
        // not punch a dark notch into the band.
        var out = []
        var any = false
        for (k = 0; k < K; k++) {
            if (n[k] > 0) { any = true; break }
        }
        if (!any)
            return null
        for (k = 0; k < K; k++) {
            var j = k
            if (n[j] === 0) {
                var d = 1
                while (d < K) {
                    if (k - d >= 0 && n[k - d] > 0) { j = k - d; break }
                    if (k + d < K && n[k + d] > 0) { j = k + d; break }
                    d++
                }
            }
            out.push(Qt.rgba(sr[j] / n[j] / 255, sg[j] / n[j] / 255, sb[j] / n[j] / 255, 1))
        }
        return out
    }

    /// One uncovered band. `bx,by,bw,bh` is the band; `ox,oy,dw,dh` the drawn
    /// art; `alongX` true for the top/bottom bands (their colour runs across,
    /// their darkening runs up/down); `seamAtEnd` true when the art is on the
    /// band's HIGH-coordinate side (the band above the art, the band left of
    /// it).
    function _padBand(ctx, dpr, cw, ch, bx, by, bw, bh, ox, oy, dw, dh, alongX, seamAtEnd) {
        if (bw <= 0.5 || bh <= 0.5)
            return
        // How deep into the art the colour is read. Shallow enough to stay
        // the EDGE (the seam has to be invisible), deep enough that one row
        // of JPEG noise cannot decide the whole band. Measured on an 800x380
        // banner in a 200px cell: at 8% of the drawn height the band arrives
        // at the seam 8/255 brighter than the art's first row, because the
        // sample reaches content the seam cannot see; at 4% the step is 3.
        var depth = Math.max(2 * dpr, Math.round((alongX ? dh : dw) * 0.04))
        // Keep the sample clear of the rounded corners, whose pixels the clip
        // has already discarded.
        var inset = Math.min(root.radius * dpr, (alongX ? dw : dh) * 0.1)
        var sx, sy, sw, sh
        if (alongX) {
            sx = ox + inset
            sw = dw - 2 * inset
            sy = seamAtEnd ? oy : (oy + dh - depth)
            sh = depth
        } else {
            sy = oy + inset
            sh = dh - 2 * inset
            sx = seamAtEnd ? ox : (ox + dw - depth)
            sw = depth
        }
        var stops = root._stripStops(ctx, cw, ch, sx, sy, sw, sh, alongX)
        if (!stops)
            return

        // 1. Colour, along the band's LONG axis. The axis spans the SAMPLED
        // STRIP — not the band, and not the art either: the strip is inset
        // from the art by `inset`, and running the axis over the art instead
        // slides every stop off the pixels it was averaged from. Measured on
        // the same banner (inset 8 of a 200px art), a highlight sitting at
        // x=140..160 in the art came out centred at x=165 in the band, a
        // ~20px drag. A Canvas gradient clamps to its terminal colour past
        // the ends, which is what carries the edge hue out into the corners.
        var g = alongX ? ctx.createLinearGradient(sx, by, sx + sw, by)
                       : ctx.createLinearGradient(bx, sy, bx, sy + sh)
        // Stop i sits at the CENTRE of the slice it was averaged from,
        // (i + 0.5) / K — not at i / (K - 1). The obvious spacing stretches
        // eight bucket means across the full axis and slides every one of
        // them off its own pixels: measured on the same banner, the band
        // 2px above the seam disagreed with the art's first row by up to 35
        // of 255 at x=150, where a bright slice had been dragged left over a
        // dark one. Centred stops bring the same column to 3.
        var K = stops.length
        for (var i = 0; i < K; i++)
            g.addColorStop((i + 0.5) / K, stops[i])
        ctx.fillStyle = g
        ctx.fillRect(bx, by, bw, bh)

        // 2. Darkening, along the SHORT axis, transparent AT THE SEAM so the
        // join is invisible and the art appears to bleed out and fade.
        var d2
        if (alongX)
            d2 = seamAtEnd ? ctx.createLinearGradient(bx, by + bh, bx, by)
                           : ctx.createLinearGradient(bx, by, bx, by + bh)
        else
            d2 = seamAtEnd ? ctx.createLinearGradient(bx + bw, by, bx, by)
                           : ctx.createLinearGradient(bx, by, bx + bw, by)
        d2.addColorStop(0, Qt.rgba(0, 0, 0, 0))
        d2.addColorStop(1, Qt.rgba(0, 0, 0, root.padFalloff))
        ctx.fillStyle = d2
        ctx.fillRect(bx, by, bw, bh)
    }

    /// Fill whatever the contained art does not cover. `contain` geometry
    /// leaves bands on ONE axis only, so at most two of these four fire.
    /// `w`/`h` are the device-pixel canvas extents and double as the
    /// getImageData clamp.
    function _paintPad(ctx, dpr, w, h, ox, oy, dw, dh) {
        if (oy > 0.5) {
            root._padBand(ctx, dpr, w, h, 0, 0, w, oy, ox, oy, dw, dh, true, true)
            root._padBand(ctx, dpr, w, h, 0, oy + dh, w, h - oy - dh, ox, oy, dw, dh, true, false)
        }
        if (ox > 0.5) {
            root._padBand(ctx, dpr, w, h, 0, 0, ox, h, ox, oy, dw, dh, false, true)
            root._padBand(ctx, dpr, w, h, ox + dw, 0, w - ox - dw, h, ox, oy, dw, dh, false, false)
        }
    }
}
