// QualityBadge — 1:1 port of crates/qbz-ui/ui/primitives/QualityBadge.slint.
//
// The ICON-ONLY badge: a compact square/rect mark that buys layout space on
// cards and list rows, with the exact quality ("24-bit / 96 kHz") revealed in
// a hover tooltip. Every number below is read off the .slint:
//
//   width  = hires 42 | lossless 36 | (cd, mp3) 30      height = 30
//   hires    -> the brand mark, 42x28, centred, NOT tinted
//   cd/mp3   -> 30x30 chip: radius 3, surface-elevated, 1px border-subtle,
//               16x16 glyph tinted text-muted
//   lossless -> 36x30 chip with the bare filetype text (9px, medium, muted)
//   tooltip  -> right-anchored 6px above the badge, 24px tall, 9px side
//               padding, 11px medium text-secondary on surface-elevated
//
// The previous file was a port of the Tauri QualityBadge.svelte with four
// modes (full/compact/bare/iconOnly) and three "state" borders. Slint has no
// such component: the stacked tier/detail form is QualityBadgeFull.qml and the
// now-playing 2-row form is shell/AudioStamp.qml. This is only the mark.
//
// COMPAT SURFACE (unchanged so QualityMini.qml and its call sites keep
// working): `tierOverride` (aliased as `tier` by QualityMini), `bitDepth`,
// `samplingRate`, `format`, and an inert `mode`.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    // ---- Slint inputs -----------------------------------------------------
    /// "hires" | "dsd" | "cd" | "mp3" | "lossless" | "" — empty hides the
    /// badge. `dsd` is ours (2026-09-02): a purchased or local 1-bit master
    /// wears the DSD mark, never the Hi-Res one — Qobuz streams nothing above
    /// 24/192, so "HI-RES 24-bit / 352.8 kHz" on a DSD album described a
    /// stream that does not exist.
    /// Named `tierOverride` (not `tier`) because QualityMini declares
    /// `property alias tier: ...tierOverride`; see `resolvedTier` below.
    property string tierOverride: ""
    /// Exact quality string for the hover tooltip, e.g. "24-bit / 96 kHz".
    property string label: ""
    /// For the "lossless" tier (un-hydrated filetype, e.g. an un-hydrated Plex
    /// FLAC): the bare filetype to print, e.g. "FLAC".
    property string detail: ""

    // ---- Compat inputs (rows that only carry raw numbers) -----------------
    property int bitDepth: 0
    property real samplingRate: 0
    property string format: ""
    /// Inert. QualityMini sets it to "iconOnly"; this component only ever
    /// renders the icon-only form now.
    property string mode: "iconOnly"

    // Tier, normalised to the FOUR names the Slint component draws. The Rust
    // side still emits "max" for >96 kHz hi-res (local_library_qt::tier_of)
    // and the old JS derivation produced "lossy" — both would have rendered a
    // blank 30px box, so they are folded into hires / mp3 here.
    //
    // Called `resolvedTier`, NOT `tier`: QualityMini declares
    // `property alias tier: ...tierOverride`, and a property redeclared in a
    // derived type SHADOWS the base one for every lookup on that instance —
    // including the bindings in this file. Naming it `tier` would silently
    // feed the raw, un-normalised value back into the drawing code for every
    // QualityMini (a local "max" album would render a blank chip).
    readonly property string resolvedTier: {
        var t = tierOverride !== "" ? tierOverride : derivedTier
        if (t === "max") return "hires"
        if (t === "lossy") return "mp3"
        return t
    }

    readonly property string derivedTier: {
        var fmt = (format || "").toLowerCase()
        // MP3 is checked FIRST — it must never resolve as "CD".
        if (fmt.indexOf("mp3") >= 0) return "mp3"
        if (bitDepth >= 24) return "hires"
        if (bitDepth === 16) return "cd"
        if (samplingRate >= 44.1) return "cd"
        return ""
    }

    readonly property bool isHiRes: resolvedTier === "hires"
    readonly property bool isDsd: resolvedTier === "dsd"

    // Tooltip copy. Slint passes the model's own quality-detail as `label` and
    // shows NOTHING when it is empty. The Qt card/row models do not carry a
    // detail field yet (see the report), so a row that hands over raw numbers
    // still gets a truthful string; nothing is fabricated when neither exists.
    readonly property string tipText: {
        if (label !== "") return label
        if (bitDepth > 0 && samplingRate > 0) return bitDepth + "-bit / " + samplingRate + " kHz"
        if (samplingRate > 0) return samplingRate + " kHz"
        return ""
    }

    visible: resolvedTier !== ""
    implicitWidth: (resolvedTier === "hires" || resolvedTier === "dsd") ? 42
        : (resolvedTier === "lossless" ? 36 : 30)
    implicitHeight: 30
    width: implicitWidth
    height: implicitHeight

    // --- hires: the brand mark, un-tinted (it carries its own colours) -----
    Image {
        visible: root.resolvedTier === "hires"
        source: "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/hi-res.svg"
        width: 42
        height: 28
        x: Math.round((root.width - width) / 2)
        y: Math.round((root.height - height) / 2)
        fillMode: Image.PreserveAspectFit
        sourceSize: Qt.size(84, 56)
    }

    // --- dsd: the DSD mark, monochrome, so it follows the theme ------------
    // Two bakes (light glyph for dark themes, dark glyph for light ones): the
    // official logo is a single flat colour and QML Image cannot tint an SVG.
    Image {
        visible: root.isDsd
        source: theme.isDark
            ? "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/dsd-light.svg"
            : "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/dsd-dark.svg"
        width: 42
        height: 21
        x: Math.round((root.width - width) / 2)
        y: Math.round((root.height - height) / 2)
        fillMode: Image.PreserveAspectFit
        sourceSize: Qt.size(84, 42)
    }

    // --- cd / mp3 / lossless: the framed chip ------------------------------
    Rectangle {
        id: chip
        visible: root.resolvedTier === "cd" || root.resolvedTier === "mp3" || root.resolvedTier === "lossless"
        width: root.resolvedTier === "lossless" ? 36 : 30
        height: 30
        radius: 3
        color: theme.surfaceElevated
        border.width: 1
        border.color: theme.borderSubtle

        // Muted-tier mark — themed (a fixed mid-grey vanished on light
        // themes). QbzIcon serves the pre-baked muted variant; hi-res above
        // keeps its brand colours.
        QbzIcon {
            visible: root.resolvedTier === "cd" || root.resolvedTier === "mp3"
            name: root.resolvedTier === "mp3" ? "mp3" : "cd"
            tintName: "muted"
            width: 16
            height: 16
            x: Math.round((parent.width - width) / 2)
            y: Math.round((parent.height - height) / 2)
        }

        // Un-hydrated lossless: bit-depth/rate unknown, filetype known — a
        // neutral text chip ("FLAC") instead of disappearing.
        Text {
            visible: root.resolvedTier === "lossless"
            text: root.detail
            color: theme.textMuted
            font.pixelSize: 9
            font.weight: theme.weightMedium
            x: Math.round((parent.width - width) / 2)
            y: Math.round((parent.height - height) / 2)
        }
    }

    // --- Hover tooltip -----------------------------------------------------
    // Right-anchored 6px above the badge so it extends LEFTWARD into the card
    // and is never clipped by the next one. Rendered as a Controls ToolTip
    // (a popup) rather than a child Rectangle: several call sites sit inside
    // `clip: true` containers, where Slint's negative-y child would vanish.
    ToolTip {
        id: tip
        visible: hover.containsMouse && root.tipText !== ""
        text: root.tipText
        delay: 0
        timeout: -1
        padding: 0
        x: root.width - width
        y: -height - 6
        implicitHeight: 24
        contentItem: Text {
            text: tip.text
            color: theme.textSecondary
            font.pixelSize: 11
            font.weight: theme.weightMedium
            verticalAlignment: Text.AlignVCenter
            leftPadding: 9
            rightPadding: 9
        }
        background: Rectangle {
            color: theme.surfaceElevated
            radius: theme.radiusSm
            border.width: 1
            border.color: theme.borderSubtle
        }
    }

    MouseArea {
        id: hover
        anchors.fill: parent
        hoverEnabled: true
        // Slint: mouse-cursor: default. Its TouchArea absorbs the press too,
        // so the default acceptedButtons is kept (1:1).
        cursorShape: Qt.ArrowCursor
    }
}
