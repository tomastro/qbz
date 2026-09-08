// KioskSkeleton — the kiosk shell's transition-state layer: ONE component that
// answers loading, error and empty, and disappears entirely when the route is
// populated.
//
// ── WHY IT IS NOT QbzSkeleton ─────────────────────────────────────────────
// controls/QbzSkeleton.qml is the desktop placeholder and it BREATHES: every
// bar binds its opacity to an animated `breathe` real driven by a 900 ms
// NumberAnimation, with an animated-instance cap of 48. On the desktop that is
// paid for and measured. On a kiosk it is not affordable and not wanted:
//
//   - contract §5.3 — "skeletons no montan imágenes reales ni modelos
//     completos y no tienen animación local continua";
//   - contract §7.4 — with reduce-motion, which is the KIOSK DEFAULT, they
//     work static;
//   - the GPU doctrine — Qt Quick has no partial redraws, so ANY dirty item
//     presents the whole window at ~1.2% GPU flat. A breathing placeholder is
//     a continuous full-window present for the entire duration of a load, i.e.
//     it spends GPU precisely while the CPU is busiest.
//
// QbzSkeleton is SHARED and launch-frozen (contract §2.1), so it is not edited
// to get a static mode. This is the kiosk-side wrapper the contract prefers
// over a conditional through a shared component's normal path.
//
// EVERYTHING HERE IS STATIC. No Timer, no NumberAnimation, no Behavior, no
// FrameAnimation, no pulse subscription. The blocks sit at `tone`, which is
// QbzSkeleton's own frozen mid value (QbzSkeleton.qml:241, the value it snaps
// to when an instance is not pulsing), so a kiosk placeholder reads as the
// same material as a settled desktop one.
//
// It also mounts NO artwork and NO model. The shapes are Rectangles over a
// BOUNDED integer count, never over the data (contract §3.3 forbids an
// unbounded Repeater over albums/artists/tracks — this Repeater's model is
// `Math.min(maxCells, cols * rows)`, an int derived from geometry alone).
//
// ── THE STATE MACHINE ─────────────────────────────────────────────────────
// Exactly one arm is mounted at a time, and `presented` names it:
//
//   loading  →  "skeleton"   the cheap chrome for `kind`
//   error    →  "error"      the document's own message
//   empty    →  "empty"      `emptyText`
//   else     →  ""           NOTHING is instantiated; the host's real content
//                            is the only thing on screen
//
// LOADING WINS OVER ERROR. A host that re-requests after a failure sets
// `loading` true again, and showing the stale error under a live retry is what
// makes a route look dead. ERROR WINS OVER EMPTY, because a document that
// failed has no idea whether it is empty.
//
// The final arm is the reason this is a Loader and not four `visible:` gates:
// `visible: false` is not lazy mounting (contract §3.3), and a populated grid
// must not be paying for a skeleton subtree underneath it.
//
// ── TEXT ──────────────────────────────────────────────────────────────────
// `emptyText` defaults to the EXISTING msgid "No results", which is present in
// all eight catalogues. No new msgid is introduced by this file, and the msgid
// IS the English string. `error` is passed through verbatim: it arrives from
// the document that produced it, already localised on the Rust side.
//
// `QbzSession.tr(…, QbzSession.trRev)` — never a bare `tr()`, which renders
// once and then lies through a language switch. The `typeof` guard mirrors
// theme/RoundedImage.qml:236: a registered singleton is always defined in the
// built binary, and the guard is what keeps this file loadable in an isolated
// qml6 harness.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    // ── THE STATE API ──────────────────────────────────────────────────────
    /// Which cheap chrome to draw while loading:
    ///   "grid"     card cells — album/artist/folder grids
    ///   "list"     rows — track lists, queue, history
    ///   "detail"   a header block (cover + title bars) over a short row list
    ///   "settings" full-width setting rows, no artwork cell
    property string kind: "grid"
    /// A fetch/mount is in flight. Wins over `error` and `empty`.
    property bool loading: false
    /// The document's error message, ALREADY TRANSLATED. "" = no error.
    property string error: ""
    /// The document resolved and has no rows.
    property bool empty: false
    /// What to say when `empty`. Defaults to the existing msgid "No results";
    /// a host with a more specific existing msgid ("No albums in your local
    /// library yet.") passes it in already translated.
    property string emptyText: root._tr("No results")

    /// Which arm is mounted: "skeleton" | "error" | "empty" | "".
    readonly property string presented: root.loading ? "skeleton"
        : root.error !== "" ? "error"
        : root.empty ? "empty"
        : ""

    // ── GEOMETRY (kind-specific; all optional) ─────────────────────────────
    /// Grid: the same three numbers KioskAlbumGrid takes, so a skeleton laid
    /// over a grid lands on the grid's own pitch.
    property int columns: 6
    property real gap: 16
    property real pad: 16
    /// Extra height under the art square, matching KioskCard's title+artist
    /// band (KioskCard.qml: art + 7 + title + 7 + artist ≈ 46).
    property real cardBand: 46
    /// Artist grids draw round cells.
    property bool roundCells: false
    /// List: KioskTrackRow's touch height and its art cell.
    property real rowHeight: 64
    property real rowSpacing: 0
    property real rowArtSize: 46
    /// Detail: the header cover edge.
    property real coverSize: 132
    /// HARD CAP on placeholder cells, whatever the geometry says. A skeleton
    /// that scales with the viewport is still bounded; this is the backstop.
    property int maxCells: 24

    /// The frozen tone. QbzSkeleton's own non-pulsing value.
    property real tone: 0.55

    QbzTheme { id: theme }

    function _tr(s) {
        return typeof QbzSession !== "undefined" ? QbzSession.tr(s, QbzSession.trRev) : s
    }

    // Nothing to present = nothing instantiated. Not `visible: false`.
    Loader {
        anchors.fill: parent
        active: root.presented !== ""
        sourceComponent: root.presented === "skeleton"
            ? (root.kind === "list" ? listShape
                : root.kind === "detail" ? detailShape
                : root.kind === "settings" ? settingsShape
                : gridShape)
            : messageShape
    }

    // ---- the empty / error arm -------------------------------------------
    Component {
        id: messageShape

        KioskEmptyState {
            anchors.fill: parent
            isError: root.presented === "error"
            text: root.presented === "error" ? root.error : root.emptyText
        }
    }

    // ---- the skeleton arms -----------------------------------------------
    // Plain Components, not inline `component` blocks: a Component shares the
    // file scope, so `root` and `theme` resolve inside it.

    // ---- "grid" ----------------------------------------------------------
    // Same pitch algebra as KioskAlbumGrid: the card width falls out of the
    // item width, the columns and the gap, so the placeholder cells sit where
    // the real cards will.
    Component {
        id: gridShape

        Item {
            id: grid

            readonly property real cardW: root.columns > 0
                ? Math.max(0, (root.width - 2 * root.pad - (root.columns - 1) * root.gap) / root.columns)
                : 0
            readonly property real cardH: grid.cardW + root.cardBand
            readonly property int rows: grid.cardH > 0
                ? Math.max(1, Math.ceil((root.height - root.pad) / (grid.cardH + root.gap)))
                : 1
            readonly property int cells: Math.max(0,
                Math.min(root.maxCells, root.columns * grid.rows))

            Repeater {
                model: grid.cells

                delegate: Item {
                    id: cell

                    required property int index

                    x: root.pad + (cell.index % root.columns) * (grid.cardW + root.gap)
                    y: root.pad + Math.floor(cell.index / root.columns) * (grid.cardH + root.gap)
                    width: grid.cardW
                    height: grid.cardH

                    Rectangle {
                        width: grid.cardW
                        height: grid.cardW
                        radius: root.roundCells ? grid.cardW / 2 : theme.radiusSm
                        color: theme.surfaceElevated
                        opacity: root.tone
                    }
                    Rectangle {
                        x: root.roundCells ? (grid.cardW - width) / 2 : 0
                        y: grid.cardW + 8
                        width: grid.cardW * 0.7
                        height: 13
                        radius: theme.radiusSm
                        color: theme.surfaceElevated
                        opacity: root.tone
                    }
                    Rectangle {
                        x: root.roundCells ? (grid.cardW - width) / 2 : 0
                        y: grid.cardW + 28
                        width: grid.cardW * 0.45
                        height: 11
                        radius: theme.radiusSm
                        color: theme.surfaceElevated
                        opacity: root.tone
                    }
                }
            }
        }
    }

    // ---- "list" ----------------------------------------------------------
    Component {
        id: listShape

        Item {
            id: list

            readonly property int rows: Math.max(0, Math.min(root.maxCells,
                Math.ceil(root.height / Math.max(1, root.rowHeight + root.rowSpacing))))

            Repeater {
                model: list.rows

                delegate: Item {
                    id: listRow

                    required property int index

                    x: root.pad
                    y: listRow.index * (root.rowHeight + root.rowSpacing)
                    width: Math.max(0, root.width - 2 * root.pad)
                    height: root.rowHeight

                    Rectangle {
                        y: (root.rowHeight - root.rowArtSize) / 2
                        width: root.rowArtSize
                        height: root.rowArtSize
                        radius: 5
                        color: theme.surfaceElevated
                        opacity: root.tone
                    }
                    Rectangle {
                        x: root.rowArtSize + 12
                        y: (root.rowHeight - 32) / 2
                        width: listRow.width * 0.34
                        height: 13
                        radius: theme.radiusSm
                        color: theme.surfaceElevated
                        opacity: root.tone
                    }
                    Rectangle {
                        x: root.rowArtSize + 12
                        y: (root.rowHeight - 32) / 2 + 20
                        width: listRow.width * 0.2
                        height: 11
                        radius: theme.radiusSm
                        color: theme.surfaceElevated
                        opacity: root.tone
                    }
                }
            }
        }
    }

    // ---- "detail" --------------------------------------------------------
    // A header block (cover + eyebrow/title/meta bars) over a few list rows —
    // the shape of KioskAlbum / KioskArtist while their document lands.
    Component {
        id: detailShape

        Item {
            Rectangle {
                x: root.pad
                y: root.pad
                width: root.coverSize
                height: root.coverSize
                radius: theme.radiusSm
                color: theme.surfaceElevated
                opacity: root.tone
            }
            Rectangle {
                x: root.pad + root.coverSize + 20
                y: root.pad + 8
                width: Math.min(280, Math.max(0, root.width - root.pad * 2 - root.coverSize - 20))
                height: 22
                radius: theme.radiusSm
                color: theme.surfaceElevated
                opacity: root.tone
            }
            Rectangle {
                x: root.pad + root.coverSize + 20
                y: root.pad + 42
                width: Math.min(180, Math.max(0, root.width - root.pad * 2 - root.coverSize - 20))
                height: 14
                radius: theme.radiusSm
                color: theme.surfaceElevated
                opacity: root.tone
            }
            Rectangle {
                x: root.pad + root.coverSize + 20
                y: root.pad + 66
                width: Math.min(140, Math.max(0, root.width - root.pad * 2 - root.coverSize - 20))
                height: 12
                radius: theme.radiusSm
                color: theme.surfaceElevated
                opacity: root.tone
            }

            Repeater {
                model: Math.max(0, Math.min(root.maxCells, Math.ceil(
                    (root.height - root.pad * 2 - root.coverSize - 16)
                    / Math.max(1, root.rowHeight + root.rowSpacing))))

                delegate: Rectangle {
                    required property int index

                    x: root.pad
                    y: root.pad + root.coverSize + 16
                        + index * (root.rowHeight + root.rowSpacing)
                    width: Math.max(0, root.width - 2 * root.pad)
                    height: Math.max(0, root.rowHeight - 10)
                    radius: theme.radiusSm
                    color: theme.surfaceElevated
                    opacity: root.tone
                }
            }
        }
    }

    // ---- "settings" ------------------------------------------------------
    // Full-width rows with no artwork cell: a label bar and a control bar.
    Component {
        id: settingsShape

        Item {
            Repeater {
                model: Math.max(0, Math.min(root.maxCells,
                    Math.ceil(root.height / Math.max(1, root.rowHeight + 8))))

                delegate: Item {
                    id: settingRow

                    required property int index

                    x: root.pad
                    y: settingRow.index * (root.rowHeight + 8)
                    width: Math.max(0, root.width - 2 * root.pad)
                    height: root.rowHeight

                    Rectangle {
                        y: (root.rowHeight - 32) / 2
                        width: settingRow.width * 0.42
                        height: 14
                        radius: theme.radiusSm
                        color: theme.surfaceElevated
                        opacity: root.tone
                    }
                    Rectangle {
                        y: (root.rowHeight - 32) / 2 + 20
                        width: settingRow.width * 0.28
                        height: 11
                        radius: theme.radiusSm
                        color: theme.surfaceElevated
                        opacity: root.tone
                    }
                    Rectangle {
                        x: Math.max(0, settingRow.width - 96)
                        y: (root.rowHeight - 28) / 2
                        width: 96
                        height: 28
                        radius: theme.radiusSm
                        color: theme.surfaceElevated
                        opacity: root.tone
                    }
                }
            }
        }
    }
}
