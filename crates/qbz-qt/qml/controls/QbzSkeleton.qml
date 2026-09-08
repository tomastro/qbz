// QbzSkeleton — THE shared loading placeholder, app-wide.
//
// Reference: crates/qbz-ui/ui/discover/HomeSkeleton.slint. The Slint HAS a
// skeleton treatment, and this is a 1:1 port of its primitive: a grey
// `surface-elevated` block on Radius.sm whose OPACITY breathes between 0.4
// and 0.85 on a 900ms shared phase. There is no travelling gradient in the
// Slint and there is none here — see the COST note below.
//
// What is a deliberate ADDITION beyond the Slint (which mounts a bare 36px
// LoadingSpinner in FavoritesView, SearchResultsView, LocalLibraryView and
// LocalAlbumView, and no per-item treatment anywhere):
//   - the "row" / "text" / "header" shapes (the Slint only has card + bar),
//   - the per-item "art" / "circle" tiles a host mounts over a card whose
//     cover has not landed yet,
//   - the "cardGrid" / "rowList" COMPOSITES (one instance = a whole grid or
//     a whole list of placeholders — see COST),
//   - the animated-instance cap, the visibility gate and the settle timeout.
//
// ── COST ───────────────────────────────────────────────────────────────────
// One instance = ONE animator, not one per bar: every bar binds its opacity
// to the single `breathe` real, so a 3-bar card animates one property — and
// a "cardGrid" of 48 cells (144 bars) still animates exactly ONE. That is
// why a host should prefer ONE composite over N single instances.
// An instance whose `cellIndex` is past `animatedCap`, or whose view is
// hidden, or whose window is minimized/hidden, or that has SETTLED, holds NO
// animator at all and renders as a static grey block (its Behavior is
// disabled, so nothing repaints).
// Worst case a host may mount: `animatedCap` (48) animated instances, i.e.
// 48 concurrent 900ms NumberAnimations. A composite spends 1 of those 48 for
// a full viewport, so the realistic worst case per view is far lower — see
// each view's own note.
//
// ── GATING RULE ────────────────────────────────────────────────────────────
// Freeze on NOT VISIBLE — the item being hidden or the window being
// minimized/hidden. NEVER on lost focus: a tiling desktop keeps windows
// visible and unfocused, and freezing those would stop the pulse under
// normal use. (Same rule as shell/AmbientField.qml.)
//
// ── DRIVING THE PHASE ──────────────────────────────────────────────────────
// Preferred: the HOST owns ONE 900ms Timer toggling a bool and every
// skeleton binds `phase:` to it — N placeholders, 1 timer. `selfDrive: true`
// mounts a private timer for one-off mounts.
//
// ── COVER HANDOVER (`pending` / `coverReady` / `coverSource`) ──────────────
// THE BUG THIS EXISTS FOR: every surface used to gate its cover placeholder
// on `artPath !== ""`. That predicate goes false the moment the PATH lands —
// but the art is not on screen then. theme/RoundedImage.qml is a Canvas: it
// still has to load the file through its probe and rasterize it. So the
// placeholder vanished and the cell sat EMPTY until the canvas painted.
//
// The honest gate is "has the image handed over", and there are two arms:
//   - `coverReady` — bind it to `RoundedImage.ready` when the host can see
//     the image item (the local rows, the detail headers). EXACT: the
//     placeholder is still fully opaque on the frame the art appears.
//   - `coverSource` — the art path, for hosts whose image is sealed inside a
//     component they do not own (AlbumCard, PlaylistCard, SlimCard, …). This
//     instance then loads the SAME path through its own hidden probe; because
//     QQuickPixmapCache is shared and process-wide, that is the SAME cache
//     entry the card is loading, so it costs no second decode and no second
//     copy in memory — it only tells us WHEN the decode finished. The card's
//     canvas paints on the next frame, which the 180ms fade below covers.
// `pending` is the third input: "this cell wants a cover at all". A row with
// no artwork slot passes false and gets no placeholder.
//
// Call sites that still set `visible:` by hand keep their own (path-based)
// behaviour — the new default binding only applies where `visible` is left
// alone. That is deliberate: the gate is opted into file by file.
//
// ── CLEANUP (settleMs / settleHold) ────────────────────────────────────────
// A placeholder gated only on "the payload has not landed" can shimmer
// FOREVER when the payload never lands. That is not hypothetical: local
// artwork resolution (src/local_artwork.rs, `resolve_window_blocking`)
// DROPS keys with no cover — "the QML map only ever grows with real hits" —
// so a local album with no embedded art never produces a map entry. The same
// holds for a Qobuz cover whose download fails: the document is republished
// without an `artPath`.
// `settleMs > 0` gives the instance a bounded life: it fades out once and
// stops animating, revealing whatever the real control draws when it has no
// art (AlbumCard's own empty tile, ArtistCard's designed round portrait).
// Per-item clearing still wins whenever the item DOES arrive.
// `settleHold` suspends that countdown while the host knows a resolution pass
// is still in flight, so a cover that takes longer than settleMs to decode
// (a COLD local library takes seconds — see LocalLibraryView.artSettleMs)
// does not settle to an empty tile a beat before it lands. The bound still
// holds: the host drops the hold when its pass goes quiet.

import QtQuick
import QtQuick.Window
import "../theme"

Item {
    id: root

    // Shape:
    //   "card"       — grid cell: art square + title bar + subtitle bar
    //                  (HomeSkeleton.slint's SkeletonRow card).
    //   "circleCard" — artist grid cell: ROUND art + two centred bars.
    //   "row"        — list row: square art + two bars.
    //   "art"        — bare square art tile (the per-item overlay).
    //   "block"      — one bar (section titles). Same shape as "art".
    //   "circle"     — bare round art tile (ArtistCard/LabelCard's portrait).
    //   "text"       — `lines` stacked text bars, tapering.
    //   "header"     — detail-page header: cover + eyebrow/title/meta bars
    //                  + a row of round action buttons.
    //   "cardGrid"   — COMPOSITE: fills this item with a grid of cards.
    //   "rowList"    — COMPOSITE: fills this item with a column of rows.
    property string variant: "card"

    // Host-driven breathe phase (ignored when selfDrive is true).
    property bool phase: false
    property bool selfDrive: false

    // Per-instance opt-out, and the animated-instance cap: a host passes the
    // placeholder's position (0-based, viewport-relative where the list is
    // virtualized) and everything past the cap renders static.
    property bool animated: true
    property int cellIndex: 0
    readonly property int animatedCap: 48

    property real blockRadius: 8

    // --- shape knobs ------------------------------------------------------
    // "text"
    property int lines: 2
    property real lineHeight: 13
    property real lineSpacing: 8
    // "header"
    property real coverSize: 150
    property real coverGap: 24
    property bool headerRound: false
    property bool compactHeader: false
    property int actionCount: 0
    property real actionSize: 32
    // "cardGrid" — cell pitch vs the card drawn inside it.
    property real cellW: 220
    property real cellH: 266
    property real cardW: 200
    property real cardH: 246
    property bool roundCells: false
    property int maxCells: 48
    // "rowList"
    property real rowH: 50
    property real rowGap: 6
    // "row" / "rowList": some lists have no artwork cell, and the cell is
    // usually SMALLER than the row (TrackRow: 36px art in a 50px row).
    property bool rowArt: true
    property real rowArtSize: 0     // 0 = square, the full row height
    property bool rowArtRound: false // artist rails: the avatar is a disc

    // "card"/"circleCard" is the 200px card footprint: 200 art + 8 + 14 + 8
    // + 12 (the Slint numbers). The other shapes are sized by the host.
    implicitWidth: (root.variant === "card" || root.variant === "circleCard") ? 200 : 0
    implicitHeight: (root.variant === "card" || root.variant === "circleCard")
        ? (root.width + 42)
        : root.variant === "text"
            ? (root.lines * root.lineHeight + Math.max(0, root.lines - 1) * root.lineSpacing)
            : root.variant === "header" ? root.coverSize : 0

    QbzTheme { id: theme }

    readonly property bool windowShowing: root.Window.window
        ? (root.Window.window.visibility !== Window.Minimized
           && root.Window.window.visibility !== Window.Hidden)
        : true

    // ---- cover handover (see the block comment) ---------------------------
    /// "This cell wants a cover." false retires the placeholder at once.
    property bool pending: true
    /// Host-driven readiness — bind to `RoundedImage.ready`.
    property bool coverReady: false
    /// Self-probing readiness — the art path, when the image is sealed inside
    /// a component the host does not own. "" disarms the probe entirely.
    property string coverSource: ""
    Image {
        id: coverProbe
        source: root.coverSource
        visible: false
        asynchronous: true
        cache: true
        // NO sourceSize: it is part of the pixmap-cache key, so requesting a
        // different decode size here would fork a SECOND decode of the same
        // file instead of riding the one the card already asked for.
    }
    /// The art is on screen (or, on the probe arm, decoded and one frame from
    /// it). This — not the path — is what retires the placeholder.
    readonly property bool handedOver: root.coverReady
        || (root.coverSource !== "" && coverProbe.status === Image.Ready)

    // ---- cleanup / settle -------------------------------------------------
    // 0 = never settle (the Home/Library mounts, whose documents always
    // republish). > 0 = fade out and stop after that many ms of shimmering.
    property int settleMs: 0
    /// While true the countdown is suspended: the host's resolution pass is
    /// still in flight, so "nothing has landed yet" is not yet evidence that
    /// nothing ever will.
    property bool settleHold: false
    property bool settled: false
    Timer {
        interval: Math.max(1, root.settleMs)
        repeat: false
        running: root.settleMs > 0 && !root.settled && !root.settleHold
            && !root.handedOver
            && root.visible && root.windowShowing
        onTriggered: root.settled = true
    }
    // A recycled delegate re-shows the placeholder for a DIFFERENT item, so
    // the countdown restarts with it.
    onVisibleChanged: if (root.visible) root.settled = false
    // A new pass arming the hold un-settles an instance that already gave up:
    // its cover may be in THIS pass.
    onSettleHoldChanged: if (root.settleHold) root.settled = false
    // One 180ms fade, once — at settle, at handover, or when the cell stops
    // wanting a cover. The fade is what removes the empty frame on handover:
    // the canvas paints on the next frame (~16ms in), while this is still at
    // ~91% opacity, so the placeholder dissolves INTO the art.
    readonly property bool retired: root.settled || root.handedOver || !root.pending
    opacity: root.retired ? 0.0 : 1.0
    Behavior on opacity { NumberAnimation { duration: 180 } }
    // Default only — a call site that assigns `visible` replaces this and
    // keeps its own gate (see the block comment).
    visible: root.opacity > 0.004

    readonly property bool pulsing: root.animated
        && root.cellIndex < root.animatedCap
        && !root.retired
        && root.visible && root.windowShowing

    property bool _localPhase: false
    readonly property bool _phase: root.selfDrive ? root._localPhase : root.phase

    Timer {
        interval: 900
        repeat: true
        running: root.selfDrive && root.pulsing
        onTriggered: root._localPhase = !root._localPhase
    }

    // THE animated value of this skeleton (see COST). Frozen at the mid tone
    // whenever the instance is not pulsing — the Behavior is disabled then,
    // so the value snaps and no animator is left running.
    property real breathe: root.pulsing ? (root._phase ? 0.85 : 0.4) : 0.55
    Behavior on breathe {
        enabled: root.pulsing
        NumberAnimation { duration: 900; easing.type: Easing.InOutQuad }
    }

    // Cell geometry shared by "card" / "circleCard" (sized to this item) and
    // by every cell of "cardGrid" (sized to cardW).
    readonly property real _cellW: root.variant === "cardGrid" ? root.cardW : root.width
    readonly property bool _cellRound: root.variant === "circleCard"
        || (root.variant === "cardGrid" && root.roundCells)
    readonly property real _rowH: root.variant === "rowList" ? root.rowH : root.height

    Loader {
        anchors.fill: parent
        // A retired instance frees its rectangles instead of keeping a dead
        // scene subtree per cell — a settled 48-cell grid is 144 Rectangles.
        // Keyed off `opacity`, not `visible`, so a call site that overrides
        // `visible` cannot strand the shape alive mid-fade.
        active: root.opacity > 0.004
        sourceComponent: (root.variant === "card" || root.variant === "circleCard") ? cellShape
            : root.variant === "row" ? rowShape
            : root.variant === "circle" ? circleShape
            : root.variant === "text" ? textShape
            : root.variant === "header" ? headerShape
            : root.variant === "cardGrid" ? cardGridShape
            : root.variant === "rowList" ? rowListShape
            : blockShape
    }

    // ---- shapes (plain Components: they share the file scope, so `root`
    // and `theme` resolve — inline `component` blocks do not) --------------
    Component {
        id: blockShape
        Rectangle {
            color: theme.surfaceElevated
            radius: root.blockRadius
            opacity: root.breathe
        }
    }
    Component {
        id: circleShape
        Rectangle {
            color: theme.surfaceElevated
            radius: Math.min(width, height) / 2
            opacity: root.breathe
        }
    }

    // ONE card cell — art (square or round) + title bar + subtitle bar.
    // Absolute child positions, never a Column: a Column inside a Repeater
    // whose width comes from its children is a binding-loop invitation.
    Component {
        id: cellShape
        Item {
            width: root._cellW
            height: root._cellW + 42
            Rectangle {
                width: root._cellW
                height: root._cellW
                radius: root._cellRound ? root._cellW / 2 : root.blockRadius
                color: theme.surfaceElevated
                opacity: root.breathe
            }
            Rectangle {
                y: root._cellW + 8
                x: root._cellRound ? (root._cellW - width) / 2 : 0
                width: root._cellW * 0.7
                height: 14
                radius: root.blockRadius
                color: theme.surfaceElevated
                opacity: root.breathe
            }
            Rectangle {
                y: root._cellW + 30
                x: root._cellRound ? (root._cellW - width) / 2 : 0
                width: root._cellW * 0.45
                height: 12
                radius: root.blockRadius
                color: theme.surfaceElevated
                opacity: root.breathe
            }
        }
    }

    // ONE list row — art square (optional) + two bars.
    Component {
        id: rowShape
        Item {
            id: rowRoot
            width: root.width
            height: root._rowH
            readonly property real art: root.rowArtSize > 0 ? root.rowArtSize : root._rowH
            readonly property real textX: root.rowArt ? rowRoot.art + 12 : 0
            Rectangle {
                visible: root.rowArt
                y: (root._rowH - rowRoot.art) / 2
                width: rowRoot.art
                height: rowRoot.art
                radius: root.rowArtRound ? rowRoot.art / 2 : 6
                color: theme.surfaceElevated
                opacity: root.breathe
            }
            Rectangle {
                x: rowRoot.textX
                y: (root._rowH - 30) / 2
                width: root.width * 0.34
                height: 13
                radius: root.blockRadius
                color: theme.surfaceElevated
                opacity: root.breathe
            }
            Rectangle {
                x: rowRoot.textX
                y: (root._rowH - 30) / 2 + 19
                width: root.width * 0.2
                height: 11
                radius: root.blockRadius
                color: theme.surfaceElevated
                opacity: root.breathe
            }
        }
    }

    // `lines` stacked text bars: full width, then 80%, last one 45%.
    Component {
        id: textShape
        Item {
            Repeater {
                model: Math.max(1, root.lines)
                delegate: Rectangle {
                    required property int index
                    y: index * (root.lineHeight + root.lineSpacing)
                    width: root.width * (index === 0 ? 1.0
                        : index === Math.max(1, root.lines) - 1 ? 0.45 : 0.8)
                    height: root.lineHeight
                    radius: root.blockRadius
                    color: theme.surfaceElevated
                    opacity: root.breathe
                }
            }
        }
    }

    // Detail-page header: cover + eyebrow / title / two meta bars + a row of
    // round action buttons (32px, the QbzCircleAction footprint).
    Component {
        id: headerShape
        Item {
            Rectangle {
                width: root.coverSize
                height: root.coverSize
                radius: root.headerRound ? root.coverSize / 2 : 12
                color: theme.surfaceElevated
                opacity: root.breathe
            }
            Item {
                x: root.coverSize + root.coverGap
                y: 4
                width: Math.max(0, root.width - root.coverSize - root.coverGap)
                height: root.coverSize
                Rectangle {
                    y: root.compactHeader ? 4 : 0
                    width: root.compactHeader ? Math.min(parent.width, 420) : 88
                    height: root.compactHeader ? 24 : 11
                    radius: root.blockRadius
                    color: theme.surfaceElevated
                    opacity: root.breathe
                }
                Rectangle {
                    visible: !root.compactHeader
                    y: root.compactHeader ? 34 : 23
                    width: root.compactHeader ? Math.min(parent.width, 260)
                        : Math.min(parent.width, 420)
                    height: root.compactHeader ? 16 : 28
                    radius: root.blockRadius
                    color: theme.surfaceElevated
                    opacity: root.breathe
                }
                Rectangle {
                    y: root.compactHeader ? 40 : 63
                    width: Math.min(parent.width, 300)
                    height: 14
                    radius: root.blockRadius
                    color: theme.surfaceElevated
                    opacity: root.breathe
                }
                Rectangle {
                    visible: !root.compactHeader
                    y: 85
                    width: Math.min(parent.width, 200)
                    height: 13
                    radius: root.blockRadius
                    color: theme.surfaceElevated
                    opacity: root.breathe
                }
                Repeater {
                    model: root.actionCount
                    delegate: Rectangle {
                        required property int index
                        x: index * (root.actionSize + 12)
                        y: root.compactHeader ? 70 : 110
                        width: root.actionSize
                        height: root.actionSize
                        radius: root.actionSize / 2
                        color: theme.surfaceElevated
                        opacity: root.breathe
                    }
                }
            }
        }
    }

    // COMPOSITE: a whole grid of card cells on ONE animator. `cellShape`
    // sizes itself (cardW x cardW+42), so the Grid only supplies the pitch.
    Component {
        id: cardGridShape
        Item {
            id: gridRoot
            readonly property int cols: Math.max(1, Math.floor(root.width / root.cellW))
            readonly property int rows: Math.max(1, Math.ceil(root.height / root.cellH))
            Grid {
                columns: gridRoot.cols
                columnSpacing: Math.max(0, root.cellW - root.cardW)
                rowSpacing: Math.max(0, root.cellH - root.cardH)
                Repeater {
                    model: Math.min(root.maxCells, gridRoot.cols * gridRoot.rows)
                    delegate: cellShape
                }
            }
        }
    }

    // COMPOSITE: a whole list of rows on ONE animator.
    Component {
        id: rowListShape
        Item {
            Column {
                spacing: root.rowGap
                Repeater {
                    model: Math.min(root.maxCells,
                        Math.max(1, Math.ceil(root.height / (root.rowH + root.rowGap))))
                    delegate: rowShape
                }
            }
        }
    }
}
