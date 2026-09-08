// SongCardStamp — the IN-CARD audio stamp of crates/qbz-ui/ui/shell/
// SongCard.slint (its `if root.has-stamp` column plus the DotLed it declares).
//
// This is what the NEW (0) and CLASSIC (1) now-playing bars mount, pinned to
// the song card's right edge:
//
//   [ QualityBadgeFull: icon + TIER over "24-bit / 96 kHz" ]   38px tall
//                          (4px)
//   [ (o) PIPEWIRE   (o) BITPERF ]                             15px tall
//
// The SMALL (2) and LARGE (3) bars mount shell/AudioStamp.qml instead — the
// inline 2-row form with the 7px text LEDs. They are different components in
// Slint and stay different here; do not merge them.
//
// Geometry read off SongCard.slint with `compact-stamp: false` and
// `narrow: false` (what both mounts pass): stamp-w 128px (dot-leds arm),
// badge box 38px, LED row 15px, 4px between them.
//
// While the stream is downgraded the badge advertises the DELIVERED
// tier/detail with an amber overlaid arrow, and the catalog max moves into the
// tooltip's "Source" line next to the cause.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../controls"
import "../theme"

Column {
    id: root

    QbzTheme { id: theme }

    spacing: 4
    width: 128
    // AppShell's single topmost QbzTooltip. Desktop NPB modes pass it through
    // SongCard so long device labels are measured by the shared bubble and
    // cannot be clipped by the player bar.
    property Item tooltipHost: null

    // SongCard.slint's show-delivered / quality-cause-line /
    // quality-stamp-tooltip, verbatim.
    readonly property bool showDelivered: QbzPlayer.npQualityDowngraded
        && QbzPlayer.npQualityTrueDetail !== ""
    readonly property string qualityCauseLine: {
        var c = QbzPlayer.npQualityLimitCause
        if (c === 1) return QbzSession.tr("Limited by your streaming quality setting", QbzSession.trRev)
        if (c === 2) return QbzSession.tr("Limited by your output device's limit", QbzSession.trRev)
        if (c === 3) return QbzSession.tr("Limited by the quality cap for {}", QbzSession.trRev)
            .replace("{}", QbzPlayer.npCastTarget)
        if (c === 4) return QbzSession.tr("Qobuz has no higher quality for this track", QbzSession.trRev)
        return ""
    }
    readonly property string qualityStampTooltip: {
        if (!showDelivered) return QbzPlayer.npQualityDetail
        var t = QbzSession.tr("Source: {}", QbzSession.trRev).replace("{}", QbzPlayer.npQualityDetail)
            + "\n"
            + QbzSession.tr("Output: {}", QbzSession.trRev).replace("{}", QbzPlayer.npQualityTrueDetail)
        return qualityCauseLine === "" ? t : t + "\n" + qualityCauseLine
    }

    // The tooltip bubble of shell/TooltipOverlay.slint, 1:1: surface-elevated,
    // radius 4, NO border (the port's earlier bubble had a 1px border-subtle
    // and radiusSm = 8), plus the downward caret. The bubble's bottom edge
    // sits 9px above the anchor and the 5px caret pokes below it, so every
    // ToolTip using this background is positioned at y = -height - 4 with a
    // bottomPadding of 10 (5px bubble padding + the 5px caret zone).
    //
    // Slint also drop-shadows the bubble (blur 10, #00000099); that needs a
    // shader layer effect and is deliberately dropped.
    // SUPERSEDED (2026-07-29): the reason used to be "renders nothing on the
    // software path". Effects need shaders, and this port runs on the GPU
    // (OpenGL RHI, measured); that note came from an offscreen session, which
    // forces the software renderer by definition — theme/RoundedImage.qml now
    // detects the software path with `GraphicsInfo.api` instead of assuming it.
    // Adding the shadow is a visual change owing its own parity pass, so the
    // bubble stays flat here.
    component TipBubble: Item {
        Rectangle {
            anchors.fill: parent
            anchors.bottomMargin: 5
            color: theme.surfaceElevated
            radius: 4
        }
        // A 7.07px square rotated 45 deg is a 10px-wide diamond; centred on
        // the bubble's bottom edge, its lower half is the 10x5 caret
        // (TooltipOverlay.slint:56 draws a 10x6 Path). QtQuick.Shapes is not
        // imported anywhere in this port.
        Rectangle {
            width: 7.07
            height: 7.07
            rotation: 45
            antialiasing: true
            color: theme.surfaceElevated
            x: Math.round((parent.width - width) / 2)
            y: parent.height - 5 - height / 2
        }
    }

    // Dot LED (circle-dot on / circle off) + constant-colour label —
    // SongCard.slint's DotLed (line 57): 9px mark, 4px gap, 8px/600 label at
    // letter-spacing 0.3 that NEVER changes colour (it stays text-muted; only
    // the dot signals state).
    //
    // The mark is DRAWN, not an SVG: the port has no circle / circle-dot
    // assets, and the software path cannot recolour at render time, so every
    // LED colour would need its own baked variant.
    //
    // The figure is measured off the two lucide SVGs Slint tints, scaled to
    // the 9px box the .slint asks for (scale = 9/24 = 0.375):
    //   circle.svg      <circle r="10" stroke-width="2">  -> painted band
    //                   r 9..11 => outer diameter 22 * 0.375 = 8.25px,
    //                   stroke   2 * 0.375 = 0.75px
    //   circle-dot.svg  adds <circle r="2.5" fill>        -> centre dot
    //                   diameter 5 * 0.375 = 1.875px
    // The previous 9px/1px ring with a 4px dot read as a solid bullseye next
    // to Slint's thin ring + pinpoint — that was the visible mismatch.
    component DotLed: Item {
        id: led
        property string label: ""
        property bool on: false
        property color litColor: "#22c55e"
        property string tooltip: ""
        implicitWidth: ledRow.implicitWidth
        implicitHeight: ledRow.implicitHeight
        width: implicitWidth
        height: implicitHeight

        Row {
            id: ledRow
            spacing: 4
            // The 9px icon BOX (what QbzIcon occupies in the .slint) — the
            // 4px gap must measure from it, not from the smaller ring.
            Item {
                width: 9
                height: 9
                anchors.verticalCenter: parent.verticalCenter
                Rectangle {
                    width: 8.25
                    height: 8.25
                    radius: width / 2
                    anchors.centerIn: parent
                    antialiasing: true
                    color: "transparent"
                    border.width: 0.75
                    border.color: led.on ? led.litColor : theme.textMuted
                    Rectangle {
                        visible: led.on
                        width: 1.875
                        height: 1.875
                        radius: width / 2
                        anchors.centerIn: parent
                        antialiasing: true
                        color: led.litColor
                    }
                }
            }
            Text {
                text: led.label
                color: theme.textMuted
                font.pixelSize: 8
                font.weight: theme.weightSemibold
                font.letterSpacing: 0.3
                anchors.verticalCenter: parent.verticalCenter
            }
        }

        MouseArea {
            id: ledHover
            anchors.fill: parent
            hoverEnabled: true
            onContainsMouseChanged: {
                if (root.tooltipHost && led.tooltip !== "")
                    root.tooltipHost.hover(containsMouse, led,
                                           "npb-led-" + led.label, led.tooltip)
            }
        }
        // Bubble geometry: see Tip below — same numbers, read off
        // shell/TooltipOverlay.slint.
        ToolTip {
            id: ledTip
            visible: !root.tooltipHost && ledHover.containsMouse && led.tooltip !== ""
            text: led.tooltip
            delay: 0
            timeout: -1
            // Geometry pinned to the text (see controls/QualityInline.qml).
            padding: 0
            leftPadding: 0
            rightPadding: 0
            topPadding: 0
            bottomPadding: 0
            leftInset: 0
            rightInset: 0
            topInset: 0
            bottomInset: 0
            margins: 0
            width: contentItem ? contentItem.implicitWidth : 0
            height: contentItem ? contentItem.implicitHeight : 0
            x: Math.round((led.width - width) / 2)
            y: -height - 4
            contentItem: Text {
                text: ledTip.text
                color: theme.textPrimary
                font.pixelSize: 11
                font.weight: theme.weightMedium
                leftPadding: 9
                rightPadding: 9
                topPadding: 5
                bottomPadding: 10
            }
            background: TipBubble {}
        }
    }

    // --- the quality badge + the downgrade arrow overlay --------------------
    Item {
        width: parent.width
        height: 38

        QualityBadgeFull {
            // Explicit width/height (SongCard.slint sets x/y/width/height on
            // the badge), NOT anchors.fill: the component binds its own
            // width/height to its implicit size, and a call-site assignment
            // cleanly replaces that binding where anchors would fight it.
            x: 0
            y: 0
            width: parent.width
            height: parent.height
            compact: false
            showIcon: true
            // While downgraded, advertise the DELIVERED tier/detail (the true
            // stream) rather than the catalog max.
            tier: root.showDelivered ? QbzPlayer.npQualityEffectiveTier
                                     : QbzPlayer.npQualityTier
            detail: root.showDelivered ? QbzPlayer.npQualityTrueDetail
                                       : QbzPlayer.npQualityDetail
        }
        // Downgrade arrow (glyph, not a pill — ADR-008), amber so it reads as
        // "adjusted", not an error. Overlaid top-right so it never disturbs
        // the badge's width maths; the tooltip carries the cause.
        Text {
            visible: QbzPlayer.npQualityDowngraded
            text: "↓"
            font.pixelSize: 9
            font.weight: Font.Bold
            color: "#eab308"
            x: parent.width - width - 2
            y: 1
        }
        MouseArea {
            id: stampHover
            anchors.fill: parent
            hoverEnabled: true
        }
        ToolTip {
            id: stampTip
            visible: stampHover.containsMouse && root.qualityStampTooltip !== ""
            text: root.qualityStampTooltip
            delay: 0
            timeout: -1
            // Geometry pinned to the text (see controls/QualityInline.qml).
            padding: 0
            leftPadding: 0
            rightPadding: 0
            topPadding: 0
            bottomPadding: 0
            leftInset: 0
            rightInset: 0
            topInset: 0
            bottomInset: 0
            margins: 0
            width: contentItem ? contentItem.implicitWidth : 0
            height: contentItem ? contentItem.implicitHeight : 0
            x: Math.round((root.width - width) / 2)
            y: -height - 4
            contentItem: Text {
                text: stampTip.text
                color: theme.textPrimary
                font.pixelSize: 11
                font.weight: theme.weightMedium
                leftPadding: 9
                rightPadding: 9
                topPadding: 5
                bottomPadding: 10
            }
            background: TipBubble {}
        }
    }

    // --- the LED row (three mutually exclusive variants) -------------------
    Item {
        width: parent.width
        height: 15

        // A peer/cast device owns playback -> show the render TARGET instead
        // of the (now meaningless) local output LEDs.
        Row {
            visible: QbzPlayer.npIsRemote && QbzPlayer.npCastTarget !== ""
            width: parent.width
            height: parent.height
            spacing: 4
            QbzIcon {
                name: "monitor-speaker"
                // Baked #e0b341 variant (assets/icons/amber/).
                tintName: "amber"
                width: 13
                height: 13
                anchors.verticalCenter: parent.verticalCenter
            }
            Text {
                width: parent.width - 17
                text: QbzPlayer.npCastTarget
                color: "#e0b341"
                font.pixelSize: 9
                font.weight: theme.weightSemibold
                elide: Text.ElideRight
                anchors.verticalCenter: parent.verticalCenter
            }
        }

        // Local-network cast (Chromecast / DLNA), no QConnect peer.
        DotLed {
            visible: QbzPlayer.npCastActive && !QbzPlayer.npIsRemote
            label: QbzPlayer.npCastProtocol === "dlna" ? "DLNA" : "CAST"
            on: true
            litColor: "#a855f7"
            tooltip: QbzSession.tr("Casting", QbzSession.trRev)
            anchors.verticalCenter: parent.verticalCenter
        }

        // Local output -> backend + mode dot LEDs, 8px apart.
        Row {
            visible: !QbzPlayer.npIsRemote && !QbzPlayer.npCastActive
            height: parent.height
            spacing: 8
            DotLed {
                label: QbzPlayer.npOutputBackendLabel
                on: QbzPlayer.npOutputBackendActive
                litColor: "#5b8def"
                tooltip: QbzPlayer.npOutputBackendTooltip
                anchors.verticalCenter: parent.verticalCenter
            }
            DotLed {
                label: QbzPlayer.npOutputModeLabel
                on: QbzPlayer.npOutputModeActive
                litColor: "#22c55e"
                tooltip: QbzSession.tr("Output mode", QbzSession.trRev)
                anchors.verticalCenter: parent.verticalCenter
            }
        }
    }
}
