// HeaderGradient — the artwork-tinted band behind the album / artist page
// headers. ONE component, mounted by BOTH AlbumView and ArtistView (the
// Slint paints them from identical blocks: AlbumPageView.slint:235-257 and
// ArtistPageView.slint:225-247 are the same four lines twice).
//
// The Slint has TWO routes for this band, and this component carries BOTH:
//   route A  header-atmosphere.width > 0  -> ImmersiveAtmosphere (a blurred,
//            magnified, four-times-overlaid copy of the cover). This is what
//            the app actually shows.
//   route B  header-atmosphere.width <= 0 -> a flat linear gradient built
//            from `header-color` (crates/qbz/src/artwork.rs header_tint), the
//            arm the Slint paints only while the atmosphere is still missing.
// Route A needs NO shader here: the atmosphere is a 128x128 PNG produced in
// Rust (atmosphere_qt.rs, the immersive.rs pipeline ported stage for stage),
// so it is an ordinary picture. `tint` is the header_tint hex the Rust side
// publishes in the page document (album_qt.rs / artist_qt.rs `headerColor`)
// and feeds route B.
//
// Bands, read off the .slint:
//   band 1  linear-gradient(180deg, tint 0%, tint 16%, transparent 100%)
//   band 4  linear-gradient(180deg, transparent 0%, transparent 82%,
//                           surface-main 100%)
// Band 4 lands the fade EXACTLY on the header/content boundary whatever
// height the header resolved to, which is why the host binds `height` to the
// divider's y rather than to a constant.
//
// --- LIGHT-THEME CONTRAST FLOOR (bands 2 + 3) ---------------------------
// The header text is WHITE whenever this band is on — AlbumPageView.slint
// :169-172 pins `hdr-strong: #ffffff` / `hdr-body: #ffffff@88%` the moment
// `header-light` is true, on EVERY theme. That is only safe because the
// backdrop the .slint normally paints is route A, and route A is OPAQUE
// DARK: ImmersiveAtmosphere.slint:18 fills `#0a0a0b` under the artwork
// copies, then :76-79 lay a flat `#000000 @ dim` sheet (the album/artist
// headers pass `dim: 0.24`) and a `linear-gradient(180deg, #00000066 0%,
// transparent 35%, #00000080 100%)` vignette over the whole band. White text
// and the overlay-palette circles sit on that, not on the page colour.
//
// Route B alone does NOT reproduce that: its tint alpha decays from 1.0 at
// 16% to 0.0 at 100%, so below roughly half the band the backdrop IS the
// page. On a dark theme that is invisible (white on dark either way); on a
// LIGHT theme the description, the meta line and the whole CircleAction row
// — which live in the bottom third of the header — were white on white.
// Route A's two constant sheets are therefore replicated verbatim here as
// bands 2 and 3, and band 1 holds the tint solid until the 82% mark where
// band 4 takes over, matching route A's opaque coverage instead of route
// B's early decay. Same numbers, same order as ImmersiveAtmosphere.
//
// Mount it as the FIRST child of the page Flickable (so it scrolls with the
// content, as the Slint's does) at x:0 with the viewport width — the band is
// deliberately full-bleed: the page's own 32px padding must not clip it, or
// a dark gutter strip appears on the right (the .slint comment at
// AlbumPageView.slint:199-202 documents that exact regression).

// --- ROUTE A IS FOUR OVERSIZED COPIES, NOT ONE STRETCHED ONE -------------
// The first version of this file painted the atmosphere as ONE Image at
// `anchors.fill` + `Image.Stretch`, i.e. the 128x128 field mapped 1:1 onto the
// band. That is not what ImmersiveAtmosphere does and it is why the header read
// as "a blob in the middle": the field is VIGNETTED in Rust (atmosphere_qt.rs
// `vignette`, 0.20 from 20% to 70% of the shorter side) and its colour is
// concentrated at the centre, so mapping it 1:1 onto a 1400x300 band puts the
// bright core dead centre and the vignette's dark ring on the edges.
//
// ImmersiveAtmosphere.slint:21-67 lays FOUR copies, each SIGNIFICANTLY LARGER
// than the band (+520/+620/+700/+560 wide, +420/+500/+540/+620 tall) and pulled
// back by a negative offset, `image-fit: cover`. So what the band shows is a
// CROP of a heavily magnified field: the vignette ring falls outside the
// viewport, the four crops disagree with each other, and the colour spreads
// across the whole band instead of pooling. THAT is the "zoom" — it is not
// decoration, it is what makes the gradient read as a field.
//
// `animated: false` (both headers pass it) freezes `tick` at 0, so the four sin
// pairs collapse to their PHASE terms — written out below with the same
// expressions the .slint uses, so they stay checkable line by line.
import QtQuick
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    // The band must not bleed: every route-A copy is larger than it.
    clip: true

    /// Artwork-derived tint, "#rrggbb" (header_tint). Empty = nothing paints.
    property string tint: ""
    /// The appearance pref AND the "no app-wide dynamic background" gate:
    /// `AppearanceState.album-header-gradient && !ShellState.app-background-active`
    /// (AlbumPageView.slint:168). The dynamic background and this atmosphere
    /// clash, so only one of them is ever on.
    property bool active: false
    /// ImmersiveAtmosphere's flat black sheet — the album and artist headers
    /// both mount it with `dim: 0.24` (AlbumPageView.slint:230,
    /// ArtistPageView.slint:222).
    property real dim: 0.24
    /// The two opacity knobs both headers pass (AlbumPageView.slint:231-232,
    /// ArtistPageView.slint:223-224). ImmersiveAtmosphere's OWN defaults are
    /// 0.95 / 0.48 — the headers override them, so these are the header values,
    /// not the component's.
    property real baseOpacity: 0.92
    property real warpOpacity: 0.38

    // `visible` gates the paint, but bindings still evaluate underneath it —
    // so the stops read this, never the raw string: coercing "" to a colour
    // logs a warning on every re-evaluation.
    readonly property color tintColor: root.tint === "" ? "transparent" : root.tint

    /// The tint at a given alpha. A GradientStop needs a concrete colour, and
    /// `Qt.alpha()` does not exist — the channels have to be rebuilt.
    function tintAlpha(a) {
        var c = root.tintColor
        return Qt.rgba(c.r, c.g, c.b, c.a * a)
    }
    /// Twin of tintAlpha for the page-surface fade underneath.
    function surfaceAlpha(a) {
        var c = theme.surfaceMain
        return Qt.rgba(c.r, c.g, c.b, a)
    }

    visible: root.active && (root.tint !== "" || root.hasAtmosphere)
    // Never eat clicks meant for the header controls painted on top.
    enabled: false

    /// ROUTE A — the blurred artwork field Slint actually renders
    /// (`ImmersiveAtmosphere`, AlbumPageView.slint:222). It needs no shader:
    /// the atmosphere is a 128x128 image produced in Rust
    /// (atmosphere_qt.rs, the immersive.rs pipeline ported stage for stage),
    /// so it works on the software path like any other picture. Empty means
    /// the cover has not resolved yet and route B stands in.
    property string atmosphere: ""
    readonly property bool hasAtmosphere: root.atmosphere !== ""

    // --- the four phase constants, one per copy --------------------------
    // ImmersiveAtmosphere drives each copy from `tick = animation-tick()/1ms`,
    // and `animated: false` pins tick at 0. What survives is the phase offset
    // inside each sine, so these are the .slint expressions with `tick = 0`
    // substituted and nothing else changed. `Math.sin` takes radians and
    // Slint's `* 1rad` does the same, so the arguments are identical.
    // They are constants: QML evaluates each of these exactly once.
    readonly property real _tau: 6.283185
    // :22-24  sx = sin(0) + 0.35*sin(1.9τ)   sy = sin(1.1τ)
    readonly property real _sx1: 0.35 * Math.sin(1.9 * root._tau)
    readonly property real _sy1: Math.sin(1.1 * root._tau)
    // :34-36  sx = sin(0.4τ) + 0.42*sin(2.6τ)   sy = sin(1.7τ)
    readonly property real _sx2: Math.sin(0.4 * root._tau) + 0.42 * Math.sin(2.6 * root._tau)
    readonly property real _sy2: Math.sin(1.7 * root._tau)
    // :46-48  sx = sin(1.6τ)   sy = sin(0.2τ) + 0.28*sin(2.8τ)
    readonly property real _sx3: Math.sin(1.6 * root._tau)
    readonly property real _sy3: Math.sin(0.2 * root._tau) + 0.28 * Math.sin(2.8 * root._tau)
    // :58-59  sx = sin(2.4τ)   sy = sin(1.4τ)
    readonly property real _sx4: Math.sin(2.4 * root._tau)
    readonly property real _sy4: Math.sin(1.4 * root._tau)

    // ImmersiveAtmosphere.slint:18 — the component's own `background: #0a0a0b`.
    // Route A is OPAQUE DARK by construction; the white header text and the
    // overlay-palette circles are legible because of THIS, not because of the
    // tint. Route B never had it (Slint paints the flat gradient without the
    // component), so it stays gated on the atmosphere.
    Rectangle {
        anchors.fill: parent
        visible: root.hasAtmosphere
        color: "#0a0a0b"
    }

    // The four artwork copies, :21-67, one row per copy. `growW`/`growH` are
    // the +Npx the .slint adds to the band's size; `baseX`/`baseY` are its
    // constant negative offsets; `offX`/`offY` are the already-resolved
    // `sx * Npx` / `sy * Npx` terms, SIGNS INCLUDED — copy 2 subtracts where
    // copy 1 adds (`x: -310px - sx*156px`), and copy 4 subtracts on x only.
    // That disagreement between the crops is the point: four identical
    // offsets would just stack into one brighter copy.
    //
    // A Repeater, not four hand-written Images and not an inline `component`:
    // an inline component does NOT see this document's ids, so an `AtmoLayer`
    // reading `root.atmosphere` would silently resolve to nothing (the trap
    // controls/QbzSkeleton.qml documents). A Repeater's delegate DOES live in
    // document scope.
    readonly property var _atmoLayers: [
        { growW: 520, growH: 420, baseX: -260, baseY: -210,
          offX:  root._sx1 * 132, offY:  root._sy1 * 58,  o: root.baseOpacity },
        { growW: 620, growH: 500, baseX: -310, baseY: -250,
          offX: -root._sx2 * 156, offY: -root._sy2 * 74,  o: root.warpOpacity },
        { growW: 700, growH: 540, baseX: -350, baseY: -270,
          offX:  root._sx3 * 118, offY:  root._sy3 * 92,  o: 0.24 },
        { growW: 560, growH: 620, baseX: -280, baseY: -310,
          offX: -root._sx4 * 102, offY:  root._sy4 * 128, o: 0.18 }
    ]

    Repeater {
        model: root._atmoLayers
        delegate: Image {
            required property var modelData
            source: root.atmosphere
            visible: root.hasAtmosphere
            asynchronous: true
            smooth: true
            // Slint `image-fit: cover`. The source is square and this rect is
            // far wider than tall, so cover scales to WIDTH and crops the
            // height — that magnification is what spreads the colour across
            // the band instead of pooling it in the middle.
            fillMode: Image.PreserveAspectCrop
            // All four share ONE decode: same url, same pixmap-cache entry.
            width: root.width + modelData.growW
            height: root.height + modelData.growH
            x: modelData.baseX + modelData.offX
            y: modelData.baseY + modelData.offY
            opacity: modelData.o
        }
    }

    // Band 1 — ROUTE B, the flat tint. This is Slint's FALLBACK, painted only
    // while no atmosphere exists; shipping it as the design is what made the
    // header read as a plain ugly gradient. Solid to the 82% mark, the last
    // 18% is band 4's job.
    Rectangle {
        anchors.fill: parent
        visible: !root.hasAtmosphere
        gradient: Gradient {
            // The tint used to hold FULL strength to 0.82 and then die inside
            // the last 18%, which draws a visible horizontal seam across the
            // page — the taller the band, the worse, because the falloff stays
            // the same fraction while the solid part grows. The stops below
            // spread the decay over the bottom ~60% on an ease-out curve, so
            // there is no single row of pixels where the gradient "starts".
            // Alpha is stepped explicitly because a GradientStop interpolates
            // linearly between whatever two colours it is given.
            GradientStop { position: 0.00; color: root.tintColor }
            GradientStop { position: 0.16; color: root.tintColor }
            GradientStop { position: 0.38; color: root.tintAlpha(0.94) }
            GradientStop { position: 0.52; color: root.tintAlpha(0.80) }
            GradientStop { position: 0.65; color: root.tintAlpha(0.60) }
            GradientStop { position: 0.77; color: root.tintAlpha(0.38) }
            GradientStop { position: 0.88; color: root.tintAlpha(0.18) }
            GradientStop { position: 0.95; color: root.tintAlpha(0.07) }
            GradientStop { position: 1.00; color: "transparent" }
        }
    }

    // Band 2 — ImmersiveAtmosphere.slint:77-79, `#000000.with-alpha(dim)`.
    Rectangle {
        anchors.fill: parent
        color: Qt.rgba(0, 0, 0, root.dim)
    }

    // Band 3 — ImmersiveAtmosphere.slint:80-82 vignette, verbatim stops.
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#66000000" }
            GradientStop { position: 0.35; color: "transparent" }
            GradientStop { position: 1.0; color: "#80000000" }
        }
    }

    // Band 4 — the blend that must COMPLETE on the header/content divider.
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            // Mirrors the tint's curve. Held at zero to 0.82 this overlay put
            // its own seam in the same place, doubling the edge the tint was
            // already drawing.
            GradientStop { position: 0.00; color: "transparent" }
            GradientStop { position: 0.45; color: "transparent" }
            GradientStop { position: 0.62; color: root.surfaceAlpha(0.12) }
            GradientStop { position: 0.75; color: root.surfaceAlpha(0.34) }
            GradientStop { position: 0.86; color: root.surfaceAlpha(0.60) }
            GradientStop { position: 0.94; color: root.surfaceAlpha(0.84) }
            GradientStop { position: 1.00; color: theme.surfaceMain }
        }
    }
}
