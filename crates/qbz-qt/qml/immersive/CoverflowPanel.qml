// CoverflowPanel — FOCUS mode 2, the cheap classic-iPhone coverflow (§6.2 of
// the 2026-08-02 immersive-port contract), port of
// crates/qbz-ui/ui/immersive/CoverflowPanel.slint:125-285.
//
// ARCHITECTURE (Slint header :4-28, binding):
//   1. PERSISTENT covers. QbzQueue.coverflowJson is the WHOLE flat queue
//      [history oldest..newest, NOW, upcoming...], rebuilt by Rust ONLY on
//      id-sequence change (§4.4) — a track advance moves only `index`, so it
//      NEVER rebuilds the Repeaters and NEVER re-decodes a visible cover.
//   2. ANIMATED SCROLL. One float `scroll` bound to the doc's int `index`
//      with a 320ms ease-in-out Behavior (:152-153). When Rust moves the
//      index, `scroll` EASES to it and the fan repaints only during the
//      320ms, then idles. NO continuous tick, NO Timer.
//   3. TRANSFORM-ONLY layout. Each card derives offset = i - scroll;
//      x = center + offset*pitch; size = baseArt * scale(|offset|); a
//      per-depth black overlay dims on an INNER Rectangle (never root
//      opacity). |offset| > 3.2 drops the card (:75). FLAT scaled cards —
//      no rotateY/skew/perspective (owner accepted flat, :24).
//   4. NAVIGATION. Left = history -> QbzQueue.queuePlayHistory(center-1-i)
//      (:219-221); right = upcoming -> QbzQueue.queuePlayUpcomingFlat(
//      orig-center-1) (:249-251, §4.4 — the queue-wide flat invokable, NEVER
//      the page-local queuePlayUpcoming, trap 23). The center card is
//      non-interactive, full-res cover (:259-273).
//
// GEOMETRY (:144-147): baseArt = max(160, min(vh-372, vw*0.52, 560))
// (reserve 372 = pad-top 52 + info ~160 + spacing 28 + player clearance
// 132); pitch = baseArt*0.42. Card scale max(0.5, 1-0.18*|off|) (:66), dim
// min(0.55, |off|*0.18) (:69), y = (stageH-slot)/2 - 12 (:82), radius 8,
// near-center shadow 28/#00000066/8 (:88-90).
//
// Z-ORDER: LEFT Repeater iterates the FORWARD model (near-left declared
// last -> paints on top); RIGHT iterates the REVERSED model (near-upcoming
// declared last, :225-233); the CENTER card is a separate topmost instance
// (a Repeater can't reorder by data, :255-258). The paint-side windows
// ((-3.5,-0.5) / (0.5,3.5)) hide each Repeater's center-index GHOST copy —
// the dedicated center card is the ONLY center cover (:209-213).
//
// COVERS: the doc carries artUrl only (§4.4); resolution rides the shared
// artwork pipeline — QbzShell.sidebarArtworkWindow(urls) dispatch +
// QbzLibrary.libraryArtworkReady -> coverMap, the QueuePanel.qml:154-196
// idiom. The center card prefers QbzPlayer.npArtworkPath (the unified
// now-playing cover, sharper), falling back to the model row's cover while
// it resolves (:158-161,:266-271).

import QtQuick
import QtQuick.Effects
import QtQuick.Window
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    // --- Data: the FLAT coverflow document (§4.4) ---------------------------
    // Guarded try/catch (trap 15); the bridge seeds a full-shape default
    // ({"index":0,"tracks":[]}, queue_bridge.rs:143) so the fallback is
    // paranoia, not the steady state.
    property var cfDoc: {
        try {
            var d = JSON.parse(QbzQueue.coverflowJson)
            if (d && d.tracks)
                return d
        } catch (e) {
        }
        return { "index": 0, "tracks": [] }
    }
    readonly property int centerIdx: root.cfDoc.index
    readonly property int cfLen: root.cfDoc.tracks.length
    // Reversed copy for the RIGHT Repeater (:234).
    readonly property var revTracks: root.cfDoc.tracks.slice().reverse()

    /// CREATION band. The fan PAINTS |offset| < 3.5 (~7 covers); this is the
    /// wider band in which a CoverCard is allowed to EXIST.
    ///
    /// Why it exists: the model is the whole flat queue, and a CoverCard is an
    /// Image + a masked layer + text + a hit area. On a real session with 1935
    /// persisted queue rows the two Repeaters below were building ~4000 of them
    /// on the UI thread, which froze the app for ~28 s at launch — the panel
    /// did not even have to be open, because an unmounted-but-declared overlay
    /// is still BUILT (AppShell mounts ImmersiveView unconditionally).
    ///
    /// The reference does create one element per row (CoverflowPanel.slint
    /// hides |offset| > 3 rather than skipping it), and that is fine THERE: a
    /// Slint element is cheap where a QML delegate is not. Copying its shape
    /// literally is what cost the 28 s.
    ///
    /// Wider than the paint band on purpose: `scroll` eases over 320 ms on a
    /// track change, so the cards it is about to reveal must already exist or
    /// they would pop in mid-animation.
    function inBand(off) {
        return off > -6.5 && off < 6.5
    }

    // --- Geometry (:144-147) ------------------------------------------------
    readonly property real baseArt: Math.max(160,
        Math.min(Math.min(root.height - 372, root.width * 0.52), 560))
    readonly property real pitch: root.baseArt * 0.42

    // D2 (contract 04 §4): the CENTER card is a large slot (up to 560 CSS
    // px) — report its size so the large-art feed re-resolves per bucket;
    // the card below binds large-feed-first with the row cover as last
    // fallback. Bucketed in Rust; invisible panel writes nothing (pulse law).
    function requestArtSize() {
        if (root.visible)
            QbzPlayer.requestNpArtworkSize(Math.round(root.baseArt))
    }
    onBaseArtChanged: requestArtSize()
    onVisibleChanged: requestArtSize()

    // --- Animated scroll — the ONLY motion (:152-153) -----------------------
    property real scroll: root.cfDoc.index
    Behavior on scroll { NumberAnimation { duration: 320; easing.type: Easing.InOutQuad } }

    // --- Cover resolution (shared artwork pipeline) -------------------------
    property var coverMap: ({})
    Connections {
        target: QbzLibrary
        function onLibraryArtworkReady(key, path) {
            var m = root.coverMap
            m[key] = path
            root.coverMap = Object.assign({}, m)
        }
    }
    Component.onCompleted: {
        root.dispatchCovers()
        requestArtSize() // D2 initial large-art request
    }
    onCfDocChanged: root.dispatchCovers()
    function dispatchCovers() {
        var tracks = root.cfDoc.tracks
        var urls = []
        for (var i = 0; i < tracks.length; i++)
            if (tracks[i].artUrl)
                urls.push(tracks[i].artUrl)
        if (urls.length > 0)
            QbzShell.sidebarArtworkWindow(JSON.stringify(urls))
    }
    function coverFor(row) {
        return (row && row.artUrl) ? (root.coverMap[row.artUrl] || "") : ""
    }

    // No-shader renderer detection, verbatim from SpectrumBand.qml (the
    // near-center drop shadow is a MultiEffect blur; no shaders -> plain
    // offset rect).
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null

    // --- One flat coverflow card (:46-123) ----------------------------------
    // ALL geometry derives from `offset` + the stage props — when `scroll`
    // eases, every card eases to its new transform with NO model change and
    // NO source reassignment. `dim` is on an INNER Rectangle, never root
    // opacity (:109-116).
    component CoverCard: Item {
        id: card
        property string artSource: ""
        property bool hasArt: false
        property real offset: 0          // signed distance from center (i - scroll)
        property real baseArt: 0         // depth-0 (center) edge length
        property real pitch: 0           // x spacing between adjacent depths
        property real stageCx: 0         // window-x of the center
        property real stageHeight: 0
        property bool interactive: false
        // Whether THIS card paints its slot (:55-60). The fan is split into
        // a LEFT Repeater, a RIGHT Repeater and the dedicated center card;
        // each Repeater card sets this true only inside its own side's
        // offset window so a slot is never double-drawn (GHOST FIX).
        property bool paintSide: true
        signal navigate()

        readonly property real ao: Math.abs(card.offset)
        // Step-down scale: center 1.0 -> ~0.5 at |offset| >= ~2.8 (:66).
        readonly property real scale: Math.max(0.5, 1.0 - 0.18 * card.ao)
        readonly property real slot: card.baseArt * card.scale
        // Per-depth black overlay: 0 at center, up to 0.55 outward (:69).
        readonly property real dim: Math.min(0.55, card.ao * 0.18)

        // Only ~7 cards near the center paint (:75).
        visible: card.ao <= 3.2 && card.paintSide
        width: card.slot
        height: card.slot
        // Centered on (stageCx + offset*pitch); small upward bias to leave
        // room for the song card below (:81-82).
        x: card.stageCx + card.offset * card.pitch - card.slot / 2
        y: (card.stageHeight - card.slot) / 2 - 12

        // Static drop shadow on (near-)center cards only — 28/#00000066/8,
        // NEVER animated (:88-90).
        Rectangle {
            visible: card.ao < 0.5
            anchors.fill: parent
            anchors.topMargin: 8
            radius: 8
            color: "#66000000"
            layer.enabled: !root._noShaders
            layer.effect: MultiEffect {
                blurEnabled: true
                blurMax: 32
                blur: 0.875 // 28/32
            }
        }
        // Neutral placeholder while the cover is unresolved (:93-99).
        Rectangle {
            visible: !card.hasArt
            anchors.fill: parent
            radius: 8
            color: "#14ffffff"
        }
        RoundedImage {
            visible: card.hasArt
            anchors.fill: parent
            radius: 8
            source: card.artSource
        }
        // Per-depth attenuation on an INNER Rectangle (:110-116).
        Rectangle {
            anchors.fill: parent
            radius: 8
            color: "#000000"
            opacity: card.dim
        }
        // Side cards navigate; the center is non-interactive (:119-122).
        MouseArea {
            anchors.fill: parent
            enabled: card.interactive && card.ao > 0.5
            cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: card.navigate()
        }
    }

    // --- Balanced composition: fan stage + track-info in ONE centered
    // column (:163-178): pad-top 52, pad-bottom 132 (player clearance),
    // spacing 28, center-aligned at any window size.
    Item {
        anchors.fill: parent
        anchors.topMargin: 52
        anchors.bottomMargin: 132
        Column {
            anchors.centerIn: parent
            width: root.width
            spacing: 28

            // --- Cover fan stage (:181-192) --------------------------------
            Item {
                id: fanStage
                width: root.width
                // Fixed height holds the center cover footprint + headroom.
                height: root.baseArt + 56
                readonly property real cx: root.width / 2

                // LEFT (history) Repeater — forward model, ascending index,
                // so the near-left cover is declared last and paints ON TOP
                // (:194-199). Paints only offset in (-3.5, -0.5).
                Repeater {
                    model: root.cfDoc.tracks
                    // The delegate is a ZERO-SIZE, NON-CLIPPING slot: the card
                    // inside places itself absolutely off `stageCx`, so the
                    // wrapper must stay at (0,0) and must not clip or the fan
                    // would collapse. Declaration order still decides z, so
                    // the near-left cover is still declared last.
                    delegate: Item {
                        id: lSlot
                        required property int index
                        required property var modelData
                        readonly property real off: lSlot.index - root.scroll
                        Loader {
                            active: root.inBand(lSlot.off)
                            sourceComponent: CoverCard {
                                offset: lSlot.off
                                baseArt: root.baseArt
                                pitch: root.pitch
                                stageCx: fanStage.cx
                                stageHeight: fanStage.height
                                artSource: root.coverFor(lSlot.modelData)
                                hasArt: artSource !== ""
                                // Left side only; hides the center-index card
                                // (GHOST FIX) and the whole right half (:209-213).
                                paintSide: lSlot.off < -0.5 && lSlot.off > -3.5
                                interactive: true
                                // i < centerIdx -> a history cover. The flat
                                // model is oldest-first while queuePlayHistory
                                // is most-recent-first: hist = center-1-i.
                                onNavigate: {
                                    if (lSlot.index < root.centerIdx)
                                        QbzQueue.queuePlayHistory(root.centerIdx - 1 - lSlot.index)
                                }
                            }
                        }
                    }
                }

                // RIGHT (upcoming) Repeater — REVERSED model: rev index j is
                // original flat index orig = cfLen-1-j, so the loop walks
                // far-upcoming -> near-upcoming and the near-upcoming cover
                // is declared last (paints ON TOP, :225-233). Paints only
                // offset in (0.5, 3.5).
                Repeater {
                    model: root.revTracks
                    // Same zero-size, non-clipping slot as the left side.
                    delegate: Item {
                        id: rSlot
                        required property int index
                        required property var modelData
                        readonly property int orig: root.cfLen - 1 - rSlot.index
                        readonly property real off: rSlot.orig - root.scroll
                        Loader {
                            active: root.inBand(rSlot.off)
                            sourceComponent: CoverCard {
                                offset: rSlot.off
                                baseArt: root.baseArt
                                pitch: root.pitch
                                stageCx: fanStage.cx
                                stageHeight: fanStage.height
                                artSource: root.coverFor(rSlot.modelData)
                                hasArt: artSource !== ""
                                paintSide: rSlot.off > 0.5 && rSlot.off < 3.5
                                interactive: true
                                // orig > centerIdx -> an upcoming cover:
                                // up_index = orig - center - 1 (:246-251), the
                                // QUEUE-WIDE flat invokable (trap 23).
                                onNavigate: {
                                    if (rSlot.orig > root.centerIdx)
                                        QbzQueue.queuePlayUpcomingFlat(rSlot.orig - root.centerIdx - 1)
                                }
                            }
                        }
                    }
                }

                // CENTER card — drawn LAST so it paints over its neighbors
                // (:255-273). Bound to the current index's offset (eases to
                // 0 with `scroll`). Prefers the size-resolved large
                // now-playing cover (D2), then the small feed, then the
                // model row's cover while they resolve. Non-interactive.
                CoverCard {
                    readonly property string rowCover: root.centerIdx >= 0
                        && root.centerIdx < root.cfLen
                        ? root.coverFor(root.cfDoc.tracks[root.centerIdx]) : ""
                    offset: root.centerIdx - root.scroll
                    baseArt: root.baseArt
                    pitch: root.pitch
                    stageCx: fanStage.cx
                    stageHeight: fanStage.height
                    artSource: QbzPlayer.npArtworkPathLarge !== ""
                        ? QbzPlayer.npArtworkPathLarge
                        : (QbzPlayer.npArtworkPath !== ""
                            ? QbzPlayer.npArtworkPath : rowCover)
                    hasArt: artSource !== ""
                    interactive: false
                }
            }

            // --- Track info (shared, homologated across the panels) --------
            // Sits directly below the fan in the SAME centered column so the
            // fan + metadata read as one balanced group (:276-283).
            ImmersiveTrackMeta {
                width: root.width
            }
        }
    }
}
