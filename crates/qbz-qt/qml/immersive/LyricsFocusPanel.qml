// LyricsFocusPanel — FOCUS mode 4, the one-line lyrics variant (§6.6 of
// the 2026-08-02 immersive-port contract), port of
// crates/qbz-ui/ui/immersive/ImmersiveLyricsFocusPanel.slint:50-151.
//
// One centered CURRENT line, cross-faded on every line change. The host still
// advances one line at a time, but the active line now reuses the split-view
// typography and per-line time fill. Data: the QbzLyrics doc (lines/status/synced,
// passed in by the mount) + the SHARED immersive LyricsSyncEngine instance
// from ImmersiveView (`sync`) — the Slint reads the one global LyricsState
// active-index; Qt has one engine per surface group, and the focus panel +
// the split lyrics share the immersive engine (trap 22: its gateOpen covers
// BOTH lyric surfaces and nothing else).
//
// Entry animation (:57-74,:116-125): fadeInUp-LITE (translate-up 24px +
// opacity; the blur is skipped per the recon Slint note) replayed WITHOUT
// remounting: a watch on sync.activeIndex flips `entering` (SNAP to the
// start pose — the Behaviors are DISABLED while entering so the pose jump
// does not animate), a 16ms one-shot releases it and the 500ms
// cubic-bezier(0.22,1,0.36,1) runs forward.
//
// The line uses split-lyrics white plus a restrained native shadow so the
// timed fill remains legible over every immersive background.

import QtQuick
import com.blitzfc.qbz
import "../shell"

Item {
    id: root

    // ---- data (supplied by the ImmersiveView mount) ----------------------
    // The QbzLyrics document fields the panel reads.
    property var lines: []
    property bool synced: false
    property int status: 0
    // The SHARED immersive LyricsSyncEngine instance (see the header).
    property var sync: null
    readonly property int activeIndex: root.sync ? root.sync.activeIndex : -1

    // Current line text (empty if no active line yet, :34-36).
    readonly property string currentLine:
        (root.activeIndex >= 0 && root.activeIndex < root.lines.length
         && root.lines[root.activeIndex].text !== undefined)
            ? root.lines[root.activeIndex].text : ""
    readonly property bool hasCurrent: root.currentLine !== ""
    // Synced docs with no current line yet (before the first lyric) show ♪
    // (:40-42).
    readonly property bool waiting: root.synced && root.status === 2
        && root.lines.length > 0 && !root.hasCurrent
    // Truly empty: ready/no-lyrics with zero lines, or idle (:44-46).
    readonly property bool noLyrics: root.status === 3
        || (root.status === 2 && root.lines.length === 0)

    // Keep the split-view typeface and weight, with a modest size lift for
    // the single-line stage. The bounded content width below remains the
    // wrapping authority, so longer lines gain rows rather than clipping.
    readonly property real lineSize: Math.round(Math.max(30,
        Math.min(root.width * 0.026, 48)))

    readonly property string fontDir: "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/fonts/"
    readonly property string fontSource: {
        if (QbzLyrics.fontIndex === 1) return root.fontDir + "LINESeedJP-Regular.ttf"
        if (QbzLyrics.fontIndex === 2) return root.fontDir + "Montserrat-VariableFont_wght.ttf"
        if (QbzLyrics.fontIndex === 3) return root.fontDir + "NotoSans-VariableFont_wdth,wght.ttf"
        if (QbzLyrics.fontIndex === 4) return root.fontDir + "SourceSans3-VariableFont_wght.ttf"
        return ""
    }
    Component {
        id: fontLoaderComponent
        FontLoader { source: root.fontSource }
    }
    Loader {
        id: selectedFontLoader
        active: root.fontSource !== ""
        sourceComponent: fontLoaderComponent
    }
    readonly property string fontFamily:
        selectedFontLoader.item
        && selectedFontLoader.item.status === FontLoader.Ready
            ? selectedFontLoader.item.name : ""

    // --- Entry-animation retrigger (:57-74) --------------------------------
    readonly property int watchedIndex: root.activeIndex
    property bool entering: false
    onWatchedIndexChanged: {
        // Snap to the start pose, then release on the next tick so the
        // Behaviors run forward (toggling within one evaluation would not
        // produce a transition — the Behaviors are disabled while entering).
        root.entering = true
        enterTimer.restart()
    }
    Timer {
        id: enterTimer
        interval: 16
        repeat: false
        onTriggered: root.entering = false
    }

    // The line block, centered as a group in the viewport; padding mirrors
    // the Svelte 80px 60px 140px (top / sides / bottom-player-clearance,
    // :83-86).
    Item {
        anchors.fill: parent
        anchors.topMargin: 80
        anchors.leftMargin: 60
        anchors.rightMargin: 60
        anchors.bottomMargin: 140

        // Current line — one line at a time, with the exact split-view
        // karaoke renderer and a restrained native shadow.
        LyricsLineRow {
            visible: root.hasCurrent
            anchors.centerIn: parent
            contentWidth: Math.min(parent.width, root.width * 0.9)
            lineText: root.currentLine
            lineIndex: root.activeIndex
            sync: root.sync
            synced: root.synced
            activeIndex: root.activeIndex
            sizeInactive: root.lineSize
            sizeActive: root.lineSize
            uppercase: QbzLyrics.uppercase
            fontFamily: root.fontFamily
            activeColor: "#ffffff"
            // 35% white: the theme's textSecondary was barely dimmer than
            // the sung overlay, so the karaoke fill read as static text.
            inactiveColor: "#59ffffff"
            liteFill: false
            centered: true
            textShadow: true
            // The translate-up is applied via anchors.verticalCenterOffset so
            // the centering anchor survives the offset.
            anchors.verticalCenterOffset: root.entering ? 24 : 0
            opacity: root.entering ? 0.0 : 1.0
            Behavior on anchors.verticalCenterOffset {
                // Disabled while entering -> the start pose SNAPS; the
                // release (entering -> false) animates forward (:118-121).
                enabled: !root.entering
                NumberAnimation {
                    duration: 500
                    easing.type: Easing.Bezier
                    easing.bezierCurve: [0.22, 1.0, 0.36, 1.0]
                }
            }
            Behavior on opacity {
                enabled: !root.entering
                NumberAnimation {
                    duration: 500
                    easing.type: Easing.Bezier
                    easing.bezierCurve: [0.22, 1.0, 0.36, 1.0]
                }
            }
        }

        // Synced-but-before-first-line: the ♪ glyph (:130-138).
        Text {
            visible: !root.hasCurrent && root.waiting
            anchors.centerIn: parent
            text: "♪"
            color: "#ffffff"
            opacity: 0.3
            font.pixelSize: 48
        }

        // No lyrics at all — localized empty state (:142-150). Slint
        // #ffffff80 (RRGGBBAA) -> Qt #AARRGGBB.
        Text {
            visible: !root.hasCurrent && !root.waiting && root.noLyrics
            anchors.centerIn: parent
            width: parent.width
            text: QbzSession.tr("No lyrics", QbzSession.trRev)
            color: "#80ffffff"
            font.pixelSize: 20
            font.italic: true
            horizontalAlignment: Text.AlignHCenter
        }
    }
}
