// Shared lyrics lines renderer — QML port of
// crates/qbz-ui/ui/shell/LyricsLinesView.slint.
//
// Everything arrives through properties; nothing here references the sidebar
// or window geometry, so the miniplayer and Immersive hosts mount it with
// their own padding, gap and type scale.
//
// What it owns:
//  - the scrolling body: top/bottom spacers (so the first/last SYNCED line
//    can center), one LyricsLineRow per line, the shared scrollbar;
//  - AUTO-CENTER: on every active-line change the row's LAYOUT-relative
//    geometry is read one frame later (a 16ms one-shot, the same trick the
//    Slint view uses for its recenter epoch — the row's font-size flip
//    resizes it, so measuring in the same pass would read the OLD height)
//    and the viewport is moved so the row sits in the middle. SMOOTH for
//    moves of <= 2 lines, INSTANT for bigger jumps and the initial sync,
//    1:1 with Slint. The animation drives `contentY` only on the auto-follow
//    path, so user wheel input is never animated.
//  - the embedded lyric font families (lazy: one FontLoader exists only while
//    a non-System option is selected).
//
// Unsynced/plain documents render as a static top-aligned list: no spacers,
// no active line, no scroll-follow.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: view

    // ---- data ----------------------------------------------------------
    // Array of { timeMs, endMs, text, translation }.
    property var lines: []
    property bool synced: false
    property bool showTranslation: false
    // The LyricsSyncEngine instance driving the highlight.
    property var sync: null
    readonly property int activeIndex: view.sync ? view.sync.activeIndex : -1

    // ---- layout knobs (a mini/immersive host overrides these) -----------
    property int sidePadding: 6
    property int lineGap: 12

    // ---- display prefs --------------------------------------------------
    property int sizeInactive: 13
    property int sizeActive: 15
    property int dimmingMode: 2
    property bool autoFollow: true
    property bool uppercase: false
    /// Opt-in centered lines (kiosk Now Playing). Desktop leaves it false, so
    /// its left-aligned lyrics are unchanged. LyricsLineRow already implements
    /// the centered layout AND the centered word-sync highlight offset.
    property bool centered: false
    // 0 System · 1 LINE Seed JP · 2 Montserrat · 3 Noto Sans · 4 Source Sans 3
    property int fontIndex: 0
    property color activeColor: theme.accent
    // Unsung part of the active karaoke line (see LyricsLineRow.qml).
    property color inactiveColor: theme.textSecondary
    property bool liteFill: false

    // Seek request from a line click (the host decides whether to honour it).
    signal seekRequested(int timeMs)

    QbzTheme { id: theme }

    readonly property real contentWidth: Math.max(0, width - 2 * sidePadding)

    // ---- embedded lyric font (lazy) --------------------------------------
    readonly property string fontDir: "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/fonts/"
    readonly property string fontSource: {
        if (view.fontIndex === 1) return view.fontDir + "LINESeedJP-Regular.ttf"
        if (view.fontIndex === 2) return view.fontDir + "Montserrat-VariableFont_wght.ttf"
        if (view.fontIndex === 3) return view.fontDir + "NotoSans-VariableFont_wdth,wght.ttf"
        if (view.fontIndex === 4) return view.fontDir + "SourceSans3-VariableFont_wght.ttf"
        return ""
    }
    Component {
        id: fontLoaderComponent
        FontLoader { source: view.fontSource }
    }
    Loader {
        id: selectedFontLoader
        active: view.fontSource !== ""
        sourceComponent: fontLoaderComponent
    }

    // "" = the real Qt/system default = Slint's "System" option. A family
    // that has not finished loading falls back to the default rather than
    // rendering in a wrong face.
    readonly property string fontFamily:
        selectedFontLoader.item
        && selectedFontLoader.item.status === FontLoader.Ready
            ? selectedFontLoader.item.name : ""

    // ---- auto-center scroll ----------------------------------------------
    // Last index centered on; -1 = nothing yet -> the next center is INSTANT.
    property int lastCentered: -1
    readonly property real spacerHeight: Math.max(0, flick.height / 2 - sizeActive * 1.5)

    onActiveIndexChanged: {
        if (view.activeIndex < 0)
            view.lastCentered = -1
        centerTimer.restart()
    }
    onLinesChanged: {
        view.lastCentered = -1
        flick.contentY = 0
    }

    Timer {
        id: centerTimer
        interval: 16
        repeat: false
        onTriggered: view.centerActive()
    }

    function centerActive() {
        if (!view.autoFollow || !view.synced)
            return
        var idx = view.activeIndex
        if (idx < 0)
            return
        var item = rows.itemAt(idx)
        if (!item)
            return
        var max = Math.max(0, flick.contentHeight - flick.height)
        var target = Math.max(0, Math.min(
            item.y + item.height / 2 - flick.height / 2, max))
        // Smooth for adjacent moves (<= 2 lines), INSTANT for bigger jumps
        // and the initial sync.
        var smooth = view.lastCentered >= 0 && Math.abs(idx - view.lastCentered) <= 2
        view.lastCentered = idx
        scrollAnim.stop()
        if (smooth) {
            scrollAnim.to = target
            scrollAnim.start()
        } else {
            flick.contentY = target
        }
    }

    NumberAnimation {
        id: scrollAnim
        target: flick
        property: "contentY"
        duration: 260
        easing.type: Easing.InOutQuad
    }

    Flickable {
        id: flick
        anchors.fill: parent
        clip: true
        contentWidth: width
        contentHeight: body.implicitHeight
        boundsBehavior: Flickable.StopAtBounds

        Column {
            id: body
            width: flick.width
            leftPadding: view.sidePadding
            rightPadding: view.sidePadding
            spacing: view.lineGap

            // Top spacer — synced docs only (so line 0 can center).
            Item {
                width: 1
                height: view.synced ? view.spacerHeight : 10
            }

            Repeater {
                id: rows
                model: view.lines

                // The delegate root is a plain Item so the click target can
                // anchors.fill it — anchoring a child INSIDE the row's Column
                // would fight the positioner.
                delegate: Item {
                    id: rowSlot
                    required property var modelData
                    required property int index

                    width: view.contentWidth
                    height: lineRow.height

                    LyricsLineRow {
                        id: lineRow
                        contentWidth: view.contentWidth
                        lineText: rowSlot.modelData.text !== undefined ? rowSlot.modelData.text : ""
                        translationText: rowSlot.modelData.translation !== undefined
                            ? rowSlot.modelData.translation : ""
                        lineIndex: rowSlot.index
                        sync: view.sync
                        synced: view.synced
                        activeIndex: view.activeIndex
                        sizeInactive: view.sizeInactive
                        sizeActive: view.sizeActive
                        dimmingMode: view.dimmingMode
                        uppercase: view.uppercase
                        fontFamily: view.fontFamily
                        activeColor: view.activeColor
                        inactiveColor: view.inactiveColor
                        liteFill: view.liteFill
                        showTranslation: view.showTranslation
                        centered: view.centered
                    }

                    // Click-to-seek on stamped lines. NOT in the Slint
                    // reference (which has neither click-to-seek nor a
                    // user-scroll pause) — kept from the existing Qt panel
                    // rather than removing working behaviour; see the report.
                    MouseArea {
                        anchors.fill: parent
                        enabled: rowSlot.modelData.timeMs !== null
                            && rowSlot.modelData.timeMs !== undefined
                        cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                        onClicked: view.seekRequested(rowSlot.modelData.timeMs)
                    }
                }
            }

            // Bottom spacer — synced docs only (so the last line can center).
            Item {
                width: 1
                height: view.synced ? view.spacerHeight : 10
            }
        }
    }

    QbzScrollBar {
        target: flick
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
    }
}
