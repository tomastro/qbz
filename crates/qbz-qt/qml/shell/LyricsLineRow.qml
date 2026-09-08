// One lyric line — QML port of the `LyricsLineRow` component inside
// crates/qbz-ui/ui/shell/LyricsLinesView.slint.
//
// Three stacked pieces, in Slint's order:
//   1. INACTIVE / unsynced: one wrapping Text on the dimming ladder.
//   2. ACTIVE: per-VISUAL-LINE karaoke highlight — the line is segmented at
//      the view's content width (Qt font metrics, so the segments match what
//      the Text actually draws), and each visual row renders a dim base plus
//      a bright overlay clipped to that row's LOCAL share of the global
//      progress. Row 1 therefore fills to 100% BEFORE row 2 starts, instead
//      of every wrapped row lighting at once.
//   3. TRANSLATION (Qobuz v10): static text under the original, ~72% size and
//      dimmer, never uppercased — it follows the line but the karaoke fill
//      stays keyed on the ORIGINAL timings and text.
//
// PERFORMANCE CONTRACT: the row never binds the 30Hz `lineProgress` unless it
// is the active row. The karaoke Repeater's model is `isActive ? segments :
// []`, so on every other row zero delegates exist and zero bindings are
// re-evaluated per tick. Segmentation itself runs only when the line becomes
// active or when the width / font / size / uppercase inputs change — never
// per tick, never per frame.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Column {
    id: row

    // ---- data ---------------------------------------------------------
    // Renamed from `index` so a Repeater delegate can carry the required
    // `index` context property without shadowing it.
    property int lineIndex: -1
    property string lineText: ""
    property string translationText: ""
    // The sync engine (LyricsSyncEngine) — read ONLY from inside the active
    // branch, so inactive rows never subscribe to its 30Hz outputs.
    property var sync: null

    // ---- state (pushed by the view; plain values, no geometry) ---------
    property bool synced: false
    property int activeIndex: -1
    readonly property bool isActive: row.synced && row.lineIndex === row.activeIndex
    readonly property bool isPast: row.synced && row.activeIndex >= 0
        && row.lineIndex < row.activeIndex
    readonly property int distance: Math.abs(row.lineIndex - row.activeIndex)

    // ---- display prefs -------------------------------------------------
    property real contentWidth: 0
    property int sizeInactive: 13
    property int sizeActive: 15
    // 0 off · 1 soft · 2 strong
    property int dimmingMode: 2
    property bool uppercase: false
    property string fontFamily: ""
    property color activeColor: theme.accent
    // The UNSUNG part of the active karaoke line. The default follows the
    // theme; immersive hosts override it with a decisively dimmer white —
    // theme.textSecondary is near-white there, so the timed fill was barely
    // distinguishable from the sung overlay (2026-08-31 visual cleanup).
    property color inactiveColor: theme.textSecondary
    // Lite fill: light the active line whole, no per-frame clip sweep.
    property bool liteFill: false
    property bool showTranslation: false
    // Full-screen one-line lyrics centre every visual wrap segment while the
    // split list keeps its established left alignment.
    property bool centered: false
    // The full-screen one-line host enables this lightweight native text
    // shadow. It avoids an extra blurred layer while keeping every glyph
    // readable over arbitrary album artwork.
    property bool textShadow: false
    property color textShadowColor: "#b0000000"

    QbzTheme { id: theme }

    // "" = the Qt/system default — Qt resets an empty family to the
    // application font, exactly Slint's empty font-family = "System" option.
    readonly property string effFamily: row.fontFamily

    width: row.contentWidth
    // Gap between the original line and its translation (no effect while the
    // translation is hidden — a single visible child has no spacing).
    spacing: 3

    // Slint transforms the string for display; the engine still matches on
    // the raw text.
    readonly property string displayText: row.uppercase ? row.lineText.toUpperCase() : row.lineText

    // Dimming ladder (LyricsLinesView.slint `line-opacity`): off -> 1.0 all;
    // soft -> 0.6 inactive; strong (default) is ASYMMETRIC — incoming lines
    // fade much harder than already-sung ones, for a stronger timing cue.
    // Unsynced lyrics render the whole list at 0.85. With no active line the
    // ladder has no reference point: full brightness.
    function lineOpacity() {
        if (!row.synced)
            return 0.85
        if (row.isActive)
            return 1.0
        if (row.dimmingMode === 0 || row.activeIndex < 0)
            return 1.0
        if (row.dimmingMode === 1)
            return 0.6
        if (row.isPast) {
            if (row.distance === 1) return 0.5
            if (row.distance === 2) return 0.35
            if (row.distance === 3) return 0.25
            return 0.18
        }
        if (row.distance === 1) return 0.4
        if (row.distance === 2) return 0.2
        if (row.distance === 3) return 0.08
        return 0.03
    }

    // ---- INACTIVE / unsynced ------------------------------------------
    Text {
        visible: !row.isActive
        width: row.contentWidth
        text: row.displayText
        wrapMode: Text.WordWrap
        horizontalAlignment: row.centered ? Text.AlignHCenter : Text.AlignLeft
        opacity: row.lineOpacity()
        // 1:1 with Slint's 220ms ease-out: upcoming lines fade IN as they
        // brighten toward active, passed lines fade OUT to their dim tier,
        // instead of snapping between dimming steps.
        Behavior on opacity { NumberAnimation { duration: 220; easing.type: Easing.OutQuad } }
        color: row.isPast ? theme.textMuted : theme.textSecondary
        font.family: row.effFamily
        font.pixelSize: row.sizeInactive
        font.weight: theme.weightMedium
        font.letterSpacing: 0.2
        style: row.textShadow ? Text.Raised : Text.Normal
        styleColor: row.textShadowColor
    }

    // ---- ACTIVE: per-visual-line karaoke -------------------------------
    // Segments: [{ text, start, w }] — `start` is the cumulative width
    // fraction of all PRIOR segments, `w` this segment's share of the line.
    property var segments: []

    TextMetrics {
        id: metrics
        font.family: row.effFamily
        font.pixelSize: row.sizeActive
        font.weight: theme.weightBold
        font.letterSpacing: 0.2
    }

    // Greedy word wrap against the SAME metrics the active line is drawn
    // with, so the segment boundaries match the rendered wrap. Runs on
    // activation and on any input change — never per tick.
    function computeSegments() {
        if (!row.isActive) {
            if (row.segments.length > 0)
                row.segments = []
            return
        }
        var text = row.displayText
        var budget = row.contentWidth
        if (text === "" || budget <= 0) {
            row.segments = [{ "text": text, "start": 0.0, "w": 1.0 }]
            return
        }
        var words = text.split(" ")
        var visual = []
        var cur = ""
        for (var i = 0; i < words.length; i++) {
            var cand = cur === "" ? words[i] : cur + " " + words[i]
            metrics.text = cand
            if (metrics.advanceWidth > budget && cur !== "") {
                visual.push(cur)
                cur = words[i]
            } else {
                cur = cand
            }
        }
        if (cur !== "")
            visual.push(cur)

        var widths = []
        var total = 0
        for (var j = 0; j < visual.length; j++) {
            metrics.text = visual[j]
            widths.push(metrics.advanceWidth)
            total += metrics.advanceWidth
        }
        var out = []
        var acc = 0
        for (var k = 0; k < visual.length; k++) {
            var share = total > 0 ? widths[k] / total : 0
            out.push({ "text": visual[k], "start": acc, "w": share })
            acc += share
        }
        row.segments = out
    }

    onIsActiveChanged: row.computeSegments()
    onDisplayTextChanged: row.computeSegments()
    onContentWidthChanged: row.computeSegments()
    onSizeActiveChanged: row.computeSegments()
    onFontFamilyChanged: row.computeSegments()

    Column {
        visible: row.isActive
        width: row.contentWidth
        spacing: 0

        Repeater {
            // Empty model on every inactive row -> no delegates -> the 30Hz
            // progress is bound exactly once, on the active row.
            model: row.isActive ? row.segments : []

            delegate: Item {
                id: seg
                required property var modelData
                x: row.centered ? (row.contentWidth - width) * 0.5 : 0
                width: segBase.width
                height: segBase.height

                Text {
                    id: segBase
                    text: seg.modelData.text
                    wrapMode: Text.NoWrap
                    color: row.inactiveColor
                    font.family: row.effFamily
                    font.pixelSize: row.sizeActive
                    font.weight: theme.weightBold
                    font.letterSpacing: 0.2
                    style: row.textShadow ? Text.Raised : Text.Normal
                    styleColor: row.textShadowColor
                }

                // Bright (sung) overlay, clipped to this segment's LOCAL fill:
                //   local = clamp((progress - start) / w, 0, 1)
                Item {
                    id: fillClip
                    x: 0
                    y: 0
                    height: segBase.height
                    clip: true
                    // Lite fill (pref): the active line is fully lit and
                    // STATIC — no per-frame clip animation, so the surface
                    // stops repainting while a line is held.
                    width: row.liteFill
                        ? segBase.width
                        : segBase.width * Math.max(0, Math.min(1,
                            seg.modelData.w > 0
                                ? ((row.sync ? row.sync.lineProgress : 0) - seg.modelData.start) / seg.modelData.w
                                : 0))
                    Behavior on width {
                        enabled: !row.liteFill
                        NumberAnimation {
                            duration: row.sync ? row.sync.fillAnimMs : 0
                            easing.type: Easing.Linear
                        }
                    }
                    Text {
                        x: 0
                        y: 0
                        width: segBase.width
                        text: seg.modelData.text
                        wrapMode: Text.NoWrap
                        color: row.activeColor
                        font.family: row.effFamily
                        font.pixelSize: row.sizeActive
                        font.weight: theme.weightBold
                        font.letterSpacing: 0.2
                        style: row.textShadow ? Text.Raised : Text.Normal
                        styleColor: row.textShadowColor
                    }
                }
            }
        }
    }

    // ---- TRANSLATION ----------------------------------------------------
    Text {
        visible: row.showTranslation && row.translationText !== ""
        width: row.contentWidth
        text: row.translationText
        wrapMode: Text.WordWrap
        horizontalAlignment: row.centered ? Text.AlignHCenter : Text.AlignLeft
        opacity: row.lineOpacity()
        Behavior on opacity { NumberAnimation { duration: 220; easing.type: Easing.OutQuad } }
        // Slint: text-muted when past, text-secondary @ 0.7 otherwise.
        color: row.isPast
            ? theme.textMuted
            : Qt.rgba(theme.textSecondary.r, theme.textSecondary.g,
                      theme.textSecondary.b, 0.7)
        font.family: row.effFamily
        font.pixelSize: Math.round((row.isActive ? row.sizeActive : row.sizeInactive) * 0.72)
        font.weight: theme.weightRegular
        font.letterSpacing: 0.2
        style: row.textShadow ? Text.Raised : Text.Normal
        styleColor: row.textShadowColor
    }
}
