// QualityBadgeFull — 1:1 port of
// crates/qbz-ui/ui/primitives/QualityBadgeFull.slint.
//
// The contained pill: format icon + the tier label stacked over the exact
// bit-depth / sample-rate line. This is what the New and Classic now-playing
// bars mount (inside the song card's audio stamp) and what album/track rows
// use to advertise stored quality. The Small and Large bars mount
// shell/AudioStamp.qml instead — a different component entirely.
//
// Numbers, read off the .slint (sf = scaleFactor):
//   height     (compact 29 | 36) * sf
//   min width  (compact 108 | 136) * sf, or content width when `bare`
//   radius 4, surface-hover fill, 1px border-subtle (all dropped when `bare`)
//   padding    (compact 8 | 10) * sf     spacing (compact 5 | 6) * sf
//   icon       hires (compact 19 | 24) * sf   cd/mp3/lossless (compact 13 | 16) * sf
//   tier text  (compact 7 | 8) * sf, weight 100, letter-spacing 0.5, secondary
//   detail     (compact 8 | 9) * sf, weight 100, muted

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    /// "hires" | "cd" | "mp3" | "lossless" | "".
    property string tier: ""
    /// Exact quality, e.g. "24-bit / 96 kHz".
    property string detail: ""
    /// Show the format mark. The track-row variant turns it off.
    property bool showIcon: true
    /// No chip background or border, text centred — blends into a track row.
    property bool bare: false
    /// ~20%-smaller variant for the now-playing stamp.
    property bool compact: false
    /// Legibility mode for hosts sitting ON the ambient/immersive backdrop
    /// (2026-08-31 visual cleanup): the chip swaps the theme surface for its
    /// own translucent dark scrim — covering only the badge, keeping the
    /// background visible — and fixed light text, so it reads over ANY cover.
    property bool overAmbient: false
    /// Extra proportional scale on top of `compact` (1.0 = none).
    property real scaleFactor: 1.0
    /// Cap for the text column, in px. 0 = uncapped, which is what every
    /// existing call site gets — this exists so the ONE caller that used to
    /// wrap the badge in a `clip: true` (rows/TrackRow.qml's quality cell) can
    /// bound the badge instead. A clip there was a batch root per visible row,
    /// and Qt cannot merge alpha geometry across clip roots.
    property real maxTextWidth: 0

    readonly property string tierLabel: tier === "hires" ? "HI-RES"
        : (tier === "dsd" ? "DSD"
        : (tier === "mp3" ? "MP3"
        : (tier === "lossless" ? "LOSSLESS" : "CD")))

    readonly property bool isHiRes: tier === "hires"
    readonly property bool isDsd: tier === "dsd"
    /// Brand marks (Hi-Res, DSD) are drawn larger than the glyph chips.
    readonly property bool isBrandMark: isHiRes || isDsd
    readonly property real sf: scaleFactor
    readonly property real padH: bare ? 0 : (compact ? 8 : 10) * sf
    readonly property real gap: bare ? 0 : (compact ? 5 : 6) * sf
    readonly property real iconSide: isBrandMark ? (compact ? 19 : 24) * sf
                                                 : (compact ? 13 : 16) * sf
    // The row's own width (no paddings) — the icon slot plus the text column.
    readonly property real innerWidth: (showIcon ? iconSide + gap : 0) + textCol.width

    visible: tier !== ""
    implicitHeight: (compact ? 29 : 36) * sf
    implicitWidth: bare ? innerWidth
        : Math.max(innerWidth + 2 * padH, (compact ? 108 : 136) * sf)
    width: implicitWidth
    height: implicitHeight

    Rectangle {
        anchors.fill: parent
        radius: 4
        color: root.bare ? "transparent"
            : (root.overAmbient ? "#8c000000" : theme.surfaceHover)
        border.width: root.bare ? 0 : 1
        border.color: root.overAmbient ? "#33ffffff" : theme.borderSubtle
    }

    Row {
        id: content
        // `alignment: start` past the left padding; `alignment: center` when
        // bare (no padding to start from).
        x: root.bare ? Math.round((root.width - root.innerWidth) / 2) : root.padH
        height: parent.height
        spacing: root.showIcon ? root.gap : 0

        // Format mark. The Hi-Res brand mark keeps its own colours; the CD /
        // MP3 / LOSSLESS marks render in the themed muted tier.
        Item {
            visible: root.showIcon
            width: root.showIcon ? root.iconSide : 0
            height: parent.height

            Image {
                visible: root.isHiRes
                anchors.centerIn: parent
                source: "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/hi-res.svg"
                width: root.iconSide
                height: root.iconSide
                fillMode: Image.PreserveAspectFit
                sourceSize: Qt.size(Math.round(root.iconSide * 2), Math.round(root.iconSide * 2))
            }
            // The DSD mark is 2:1, monochrome, themed by bake (QualityBadge.qml).
            Image {
                visible: root.isDsd
                anchors.centerIn: parent
                source: theme.isDark
                    ? "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/dsd-light.svg"
                    : "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/dsd-dark.svg"
                width: root.iconSide
                height: Math.round(root.iconSide / 2)
                fillMode: Image.PreserveAspectFit
                sourceSize: Qt.size(Math.round(root.iconSide * 2), Math.round(root.iconSide))
            }
            // Un-hydrated lossless reuses the neutral disc mark; the detail
            // line carries "FLAC".
            QbzIcon {
                visible: !root.isBrandMark
                anchors.centerIn: parent
                name: root.tier === "mp3" ? "mp3" : "cd"
                tintName: root.overAmbient ? "white" : "muted"
                width: root.iconSide
                height: root.iconSide
            }
        }

        Column {
            id: textCol
            anchors.verticalCenter: parent.verticalCenter
            spacing: 1
            // Explicit (not implicit) so the children may bind to it: a Text
            // with `width: parent.width` inside an implicitly-sized Column is
            // a binding loop. Both Texts are unwrapped, so `implicitWidth` is
            // the natural content width and does not depend on `width`.
            // `implicitWidth` stays the natural content width even with the
            // elides below (an unwrapped Text does not shrink its implicit
            // size), so the no-binding-loop argument above still holds.
            width: root.maxTextWidth > 0
                ? Math.min(root.maxTextWidth,
                           Math.max(tierText.implicitWidth, detailText.implicitWidth))
                : Math.max(tierText.implicitWidth, detailText.implicitWidth)

            Text {
                id: tierText
                width: parent.width
                text: root.tierLabel
                color: root.overAmbient ? "#e6ffffff" : theme.textSecondary
                font.pixelSize: Math.max(1, Math.round((root.compact ? 7 : 8) * root.sf))
                font.weight: Font.Thin
                font.letterSpacing: 0.5
                horizontalAlignment: root.bare ? Text.AlignHCenter : Text.AlignLeft
                elide: Text.ElideRight
            }
            Text {
                id: detailText
                width: parent.width
                text: root.detail
                color: root.overAmbient ? "#b3ffffff" : theme.textMuted
                font.pixelSize: Math.max(1, Math.round((root.compact ? 8 : 9) * root.sf))
                font.weight: Font.Thin
                horizontalAlignment: root.bare ? Text.AlignHCenter : Text.AlignLeft
                elide: Text.ElideRight
            }
        }
    }
}
